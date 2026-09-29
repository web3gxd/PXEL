import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import {
  appendFileSync,
  cpSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  connect,
  createDocument,
  getDocument,
  login,
  releaseSessions,
  setCommand,
} from '../helpers/client.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

const localRoot = resolve('.local');
const configPath = (path) =>
  (process.platform === 'win32' ? path.replaceAll('\\', '\\\\') : path).replaceAll("'", "''");

function psql(port, sql, env) {
  return run(
    exe('psql'),
    ['-h', '127.0.0.1', '-p', String(port), '-U', 'pxel', '-d', 'pxel', '-tAc', sql],
    env,
    true,
  );
}

test(
  'PITR stops at a named restore point before the next committed operation',
  { timeout: 180000 },
  async () => {
    if (!process.env.PXEL_PG_BIN)
      throw new Error('PXEL_PG_BIN must point to private PostgreSQL binaries');
    mkdirSync(localRoot, { recursive: true });
    const root = mkdtempSync(join(localRoot, 'pg-pitr-'));
    const sourceData = join(root, 'source-db');
    const backupData = join(root, 'backup-db');
    const sourceBlobs = join(root, 'source-blobs');
    const backupBlobs = join(root, 'backup-blobs');
    const walArchive = join(root, 'wal-archive');
    const passwordFile = join(root, 'password');
    mkdirSync(walArchive);
    const password = randomBytes(24).toString('hex');
    writeFileSync(passwordFile, password, { mode: 0o600 });
    const sourcePort = await freePort();
    const restorePort = await freePort();
    const appPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`;
    const commonEnv = {
      ...process.env,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_SNAPSHOT_INTERVAL: '2',
    };
    const sourceEnv = {
      ...commonEnv,
      DATABASE_URL: `postgresql://pxel:${password}@127.0.0.1:${sourcePort}/pxel`,
      PXEL_BLOB_ROOT: sourceBlobs,
    };
    const restoredEnv = {
      ...commonEnv,
      DATABASE_URL: `postgresql://pxel:${password}@127.0.0.1:${restorePort}/pxel`,
      PXEL_BLOB_ROOT: backupBlobs,
    };
    const pgCtl = exe('pg_ctl');
    const pgArgs = (data) => ['-D', data];
    function startPg(data, port, logName, env) {
      run(
        pgCtl,
        [
          ...pgArgs(data),
          '-l',
          join(root, logName),
          '-o',
          `-h 127.0.0.1 -p ${port} -c synchronous_commit=on -c fsync=on -c full_page_writes=on`,
          '-w',
          'start',
        ],
        env,
      );
    }
    function stopPg(data, env) {
      try {
        run(pgCtl, [...pgArgs(data), 'status'], env);
      } catch {
        return;
      }
      run(pgCtl, [...pgArgs(data), '-m', 'immediate', '-w', 'stop'], env);
    }
    let server;
    let sourceRunning = false;
    let restoreRunning = false;
    try {
      run(
        exe('initdb'),
        [
          '-D',
          sourceData,
          '-U',
          'pxel',
          '--encoding=UTF8',
          '--locale=C',
          '--auth-host=scram-sha-256',
          '--auth-local=scram-sha-256',
          `--pwfile=${passwordFile}`,
        ],
        sourceEnv,
      );
      const archiveCommand =
        process.platform === 'win32'
          ? `copy /Y "%p" "${configPath(join(walArchive, '%f'))}"`
          : `cp "%p" "${configPath(join(walArchive, '%f'))}"`;
      appendFileSync(
        join(sourceData, 'postgresql.conf'),
        `\narchive_mode = on\narchive_command = '${archiveCommand}'\n`,
      );
      startPg(sourceData, sourcePort, 'source-postgres.log', sourceEnv);
      sourceRunning = true;
      run(
        exe('createdb'),
        ['-h', '127.0.0.1', '-p', String(sourcePort), '-U', 'pxel', 'pxel'],
        sourceEnv,
      );
      server = await startServer(sourceEnv, base, join(root, 'source-server.log'));
      const firstCookie = await login(base);
      const { snapshot } = await createDocument(base, firstCookie);
      const firstWriter = await connect(base, firstCookie);
      try {
        assert.equal(
          (await firstWriter.send(setCommand(firstWriter, snapshot, 'one'))).case,
          'committed',
        );
        const atTwo = await getDocument(base, firstCookie, snapshot.document_id);
        assert.equal(
          (await firstWriter.send(setCommand(firstWriter, atTwo, 'two', { sequence: 2 }))).case,
          'committed',
        );
      } finally {
        firstWriter.socket.terminate();
      }
      const activeQuery = `SELECT count(*) FROM document_snapshots WHERE document_id='${snapshot.document_id}' AND snapshot_version=3 AND state='ACTIVE'`;
      let active = false;
      for (let attempt = 0; attempt < 100; attempt++) {
        if (psql(sourcePort, activeQuery, sourceEnv) === '1') {
          active = true;
          break;
        }
        await delay(100);
      }
      assert.ok(active, 'snapshot v3 did not activate');
      const atThree = await getDocument(base, firstCookie, snapshot.document_id);
      await releaseSessions(base);
      await killServer(server);
      server = undefined;
      cpSync(sourceBlobs, backupBlobs, { recursive: true, errorOnExist: true, force: false });
      run(
        exe('pg_basebackup'),
        [
          '-h',
          '127.0.0.1',
          '-p',
          String(sourcePort),
          '-U',
          'pxel',
          '-D',
          backupData,
          '-Fp',
          '-X',
          'stream',
          '-c',
          'fast',
        ],
        sourceEnv,
        false,
        120000,
      );
      run(exe('pg_verifybackup'), ['-q', backupData], sourceEnv, false, 120000);

      server = await startServer(sourceEnv, base, join(root, 'source-server.log'));
      const sourceCookie = await login(base);
      const writer = await connect(base, sourceCookie);
      let targetHash;
      try {
        const versionFour = await writer.send(
          setCommand(writer, atThree, 'restore target', { sequence: 1 }),
        );
        assert.equal(versionFour.case, 'committed');
        assert.equal(versionFour.value.committedVersion, 4n);
        targetHash = Buffer.from(versionFour.value.confirmedStateHash).toString('hex');
        const marker = psql(sourcePort, "SELECT pg_create_restore_point('pxel_v4')", sourceEnv);
        assert.match(marker, /^[0-9A-F]+\/[0-9A-F]+$/);
        const markerFile = psql(
          sourcePort,
          `SELECT pg_walfile_name('${marker}'::pg_lsn)`,
          sourceEnv,
        );
        assert.match(markerFile, /^[0-9A-F]{24}$/);
        const atFour = await getDocument(base, sourceCookie, snapshot.document_id);
        const versionFive = await writer.send(setCommand(writer, atFour, 'later', { sequence: 2 }));
        assert.equal(versionFive.case, 'committed');
        assert.equal(versionFive.value.committedVersion, 5n);
        psql(sourcePort, 'SELECT pg_switch_wal()', sourceEnv);
        let archived = false;
        for (let attempt = 0; attempt < 100; attempt++) {
          if (existsSync(join(walArchive, markerFile))) {
            archived = true;
            break;
          }
          await delay(100);
        }
        assert.ok(archived, 'restore point WAL segment was not archived');
      } finally {
        writer.socket.terminate();
        await releaseSessions(base);
      }
      await killServer(server);
      server = undefined;
      stopPg(sourceData, sourceEnv);
      sourceRunning = false;

      const restoreCommand =
        process.platform === 'win32'
          ? `copy /Y "${configPath(join(walArchive, '%f'))}" "%p"`
          : `cp "${configPath(join(walArchive, '%f'))}" "%p"`;
      appendFileSync(
        join(backupData, 'postgresql.conf'),
        `\nrestore_command = '${restoreCommand}'\nrecovery_target_name = 'pxel_v4'\nrecovery_target_action = 'promote'\nrecovery_target_timeline = 'current'\n`,
      );
      writeFileSync(join(backupData, 'recovery.signal'), '');
      startPg(backupData, restorePort, 'restored-postgres.log', restoredEnv);
      restoreRunning = true;
      let promoted = false;
      for (let attempt = 0; attempt < 100; attempt++) {
        try {
          if (psql(restorePort, 'SELECT pg_is_in_recovery()', restoredEnv) === 'f') {
            promoted = true;
            break;
          }
        } catch {}
        await delay(100);
      }
      assert.ok(promoted, 'PITR did not promote at the requested restore point');
      server = await startServer(restoredEnv, base, join(root, 'restored-server.log'));
      const restoredCookie = await login(base);
      const restored = await getDocument(base, restoredCookie, snapshot.document_id);
      assert.equal(restored.version, '4');
      assert.equal(restored.state_hash, targetHash);
      assert.equal(restored.sheets[0].cells[0].value.value, 'restore target');
      const recoveries = readFileSync(join(root, 'restored-server.log'), 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line))
        .filter((line) => line.fields?.event === 'document_recovery');
      assert.ok(
        recoveries.some(
          (line) => line.fields.snapshot_version === 3 && line.fields.replayed_events === 1,
        ),
        'PITR actor did not use snapshot v3 plus v4 tail',
      );
      const historical = await fetch(base + `/api/documents/${snapshot.document_id}/versions/3`, {
        headers: { Origin: 'http://127.0.0.1:5173', Cookie: restoredCookie },
      });
      assert.equal(historical.status, 200);
      assert.equal((await historical.json()).state_hash, atThree.state_hash);
      await releaseSessions(base);
    } finally {
      try {
        if (server && server.exitCode === null && server.signalCode === null)
          await releaseSessions(base);
      } finally {
        try {
          await killServer(server);
        } finally {
          if (sourceRunning) stopPg(sourceData, sourceEnv);
          if (restoreRunning) stopPg(backupData, restoredEnv);
          const safeRoot = resolve(root);
          if (!safeRoot.startsWith(localRoot + sep)) throw new Error('unsafe PITR cleanup target');
          if (process.env.PXEL_TEST_KEEP_RECOVERY !== '1')
            rmSync(safeRoot, { recursive: true, force: true });
        }
      }
    }
  },
);
