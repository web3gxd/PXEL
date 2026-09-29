import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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

test(
  'verified physical backup and snapshot objects restore a consistent document',
  { timeout: 180000 },
  async () => {
    if (!process.env.PXEL_PG_BIN)
      throw new Error('PXEL_PG_BIN must point to private PostgreSQL binaries');
    mkdirSync(localRoot, { recursive: true });
    const root = mkdtempSync(join(localRoot, 'pg-restore-'));
    const sourceData = join(root, 'source-db');
    const backupData = join(root, 'backup-db');
    const sourceBlobs = join(root, 'source-blobs');
    const backupBlobs = join(root, 'backup-blobs');
    const passwordFile = join(root, 'password');
    const password = randomBytes(24).toString('hex');
    writeFileSync(passwordFile, password, { mode: 0o600 });
    const sourcePort = await freePort();
    const restorePort = await freePort();
    const appPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`;
    const databaseUrl = (port) => `postgresql://pxel:${password}@127.0.0.1:${port}/pxel`;
    const commonEnv = {
      ...process.env,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_SNAPSHOT_INTERVAL: '2',
    };
    const sourceEnv = {
      ...commonEnv,
      DATABASE_URL: databaseUrl(sourcePort),
      PXEL_BLOB_ROOT: sourceBlobs,
    };
    const restoredEnv = {
      ...commonEnv,
      DATABASE_URL: databaseUrl(restorePort),
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
      startPg(sourceData, sourcePort, 'source-postgres.log', sourceEnv);
      sourceRunning = true;
      run(
        exe('createdb'),
        ['-h', '127.0.0.1', '-p', String(sourcePort), '-U', 'pxel', 'pxel'],
        sourceEnv,
      );
      server = await startServer(sourceEnv, base, join(root, 'source-server.log'));
      const cookie = await login(base);
      const { snapshot } = await createDocument(base, cookie);
      const writer = await connect(base, cookie);
      let backupHash;
      let sourceVersionThreeHash;
      try {
        assert.equal((await writer.send(setCommand(writer, snapshot, 'one'))).case, 'committed');
        const atTwo = await getDocument(base, cookie, snapshot.document_id);
        assert.equal(
          (await writer.send(setCommand(writer, atTwo, 'two', { sequence: 2 }))).case,
          'committed',
        );
        const activeQuery = `SELECT count(*) FROM document_snapshots WHERE document_id='${snapshot.document_id}' AND snapshot_version=3 AND state='ACTIVE'`;
        let active = false;
        for (let attempt = 0; attempt < 100; attempt++) {
          if (
            run(
              exe('psql'),
              [
                '-h',
                '127.0.0.1',
                '-p',
                String(sourcePort),
                '-U',
                'pxel',
                '-d',
                'pxel',
                '-tAc',
                activeQuery,
              ],
              sourceEnv,
              true,
            ) === '1'
          ) {
            active = true;
            break;
          }
          await delay(100);
        }
        assert.ok(active, 'snapshot v3 did not activate before backup');
        const atThree = await getDocument(base, cookie, snapshot.document_id);
        sourceVersionThreeHash = atThree.state_hash;
        const committed = await writer.send(
          setCommand(writer, atThree, 'backup point', { sequence: 3 }),
        );
        assert.equal(committed.case, 'committed');
        assert.equal(committed.value.committedVersion, 4n);
        backupHash = Buffer.from(committed.value.confirmedStateHash).toString('hex');
      } finally {
        writer.socket.terminate();
      }
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

      // The source advances after the backup. Restoring must stay at v4.
      server = await startServer(sourceEnv, base, join(root, 'source-server.log'));
      const laterCookie = await login(base);
      const laterWriter = await connect(base, laterCookie);
      try {
        const atFour = await getDocument(base, laterCookie, snapshot.document_id);
        assert.equal(atFour.version, '4');
        assert.equal(
          (await laterWriter.send(setCommand(laterWriter, atFour, 'later', { sequence: 1 }))).case,
          'committed',
        );
        assert.equal((await getDocument(base, laterCookie, snapshot.document_id)).version, '5');
      } finally {
        laterWriter.socket.terminate();
        await releaseSessions(base);
      }
      await killServer(server);
      server = undefined;
      stopPg(sourceData, sourceEnv);
      sourceRunning = false;

      startPg(backupData, restorePort, 'restored-postgres.log', restoredEnv);
      restoreRunning = true;
      server = await startServer(restoredEnv, base, join(root, 'restored-server.log'));
      const restoredCookie = await login(base);
      const restored = await getDocument(base, restoredCookie, snapshot.document_id);
      assert.equal(restored.version, '4');
      assert.equal(restored.state_hash, backupHash);
      assert.equal(restored.sheets[0].cells[0].value.value, 'backup point');
      const recoveries = readFileSync(join(root, 'restored-server.log'), 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line))
        .filter((line) => line.fields?.event === 'document_recovery');
      assert.ok(
        recoveries.some(
          (line) => line.fields.snapshot_version === 3 && line.fields.replayed_events === 1,
        ),
        'restored actor did not use snapshot v3 plus v4 tail',
      );
      const historical = await fetch(base + `/api/documents/${snapshot.document_id}/versions/3`, {
        headers: { Origin: 'http://127.0.0.1:5173', Cookie: restoredCookie },
      });
      assert.equal(historical.status, 200);
      assert.equal((await historical.json()).state_hash, sourceVersionThreeHash);
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
          if (!safeRoot.startsWith(localRoot + sep))
            throw new Error('unsafe restore cleanup target');
          if (process.env.PXEL_TEST_KEEP_RECOVERY !== '1')
            rmSync(safeRoot, { recursive: true, force: true });
        }
      }
    }
  },
);
