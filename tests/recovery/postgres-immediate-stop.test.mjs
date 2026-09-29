import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import {
  closeSync,
  mkdtempSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:net';
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

const localRoot = resolve('.local');
const exe = (name) =>
  join(process.env.PXEL_PG_BIN ?? '', name + (process.platform === 'win32' ? '.exe' : ''));

function run(command, args, env = process.env, capture = false) {
  // pg_ctl's server process can inherit captured pipe handles, leaving spawnSync
  // waiting for EOF forever even after pg_ctl itself exits.
  const result = spawnSync(command, args, {
    env,
    encoding: 'utf8',
    windowsHide: true,
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'ignore',
    timeout: 30000,
  });
  if (result.error || result.status !== 0)
    throw new Error(
      `${command} failed: ${result.error?.code ?? result.stderr?.trim() ?? result.status}`,
    );
  return capture ? result.stdout.trim() : '';
}

async function freePort() {
  const server = createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  server.close();
  await once(server, 'close');
  return port;
}

async function startServer(env, base, logFile) {
  const binary = resolve('target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''));
  const log = openSync(logFile, 'a');
  const child = spawn(binary, [], { env, stdio: ['ignore', log, log], windowsHide: true });
  closeSync(log);
  let startError;
  child.on('error', (error) => {
    startError = error;
  });
  for (let attempt = 0; attempt < 150; attempt++) {
    if (startError || child.exitCode !== null || child.signalCode !== null)
      throw startError ?? new Error('server exited before ready; inspect private server.log');
    try {
      if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return child;
    } catch {}
    await delay(100);
  }
  child.kill('SIGKILL');
  throw new Error('server readiness timeout');
}

async function killServer(child) {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exit = once(child, 'exit');
    child.kill('SIGKILL');
    await exit;
  }
}

test(
  'committed tail survives immediate PostgreSQL stop and replays after ACTIVE snapshot',
  { timeout: 120000 },
  async () => {
    if (!process.env.PXEL_PG_BIN)
      throw new Error('PXEL_PG_BIN must point to private PostgreSQL binaries');
    mkdirSync(localRoot, { recursive: true });
    const root = mkdtempSync(join(localRoot, 'pg-crash-'));
    const data = join(root, 'data');
    const passwordFile = join(root, 'password');
    const password = randomBytes(24).toString('hex');
    writeFileSync(passwordFile, password, { mode: 0o600 });
    const pgPort = await freePort();
    const appPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`;
    const databaseUrl = `postgresql://pxel:${password}@127.0.0.1:${pgPort}/pxel`;
    const env = {
      ...process.env,
      DATABASE_URL: databaseUrl,
      PGPASSWORD: password,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_BLOB_ROOT: join(root, 'blobs'),
      PXEL_SNAPSHOT_INTERVAL: '2',
    };
    const pgCtl = exe('pg_ctl');
    const pgArgs = ['-D', data];
    let child;
    let postgresRunning = false;
    try {
      run(
        exe('initdb'),
        [
          '-D',
          data,
          '-U',
          'pxel',
          '--encoding=UTF8',
          '--locale=C',
          '--auth-host=scram-sha-256',
          '--auth-local=scram-sha-256',
          `--pwfile=${passwordFile}`,
        ],
        env,
      );
      const startPg = () => {
        run(
          pgCtl,
          [
            ...pgArgs,
            '-l',
            join(root, 'postgres.log'),
            '-o',
            `-h 127.0.0.1 -p ${pgPort} -c synchronous_commit=on -c fsync=on -c full_page_writes=on`,
            '-w',
            'start',
          ],
          env,
        );
        postgresRunning = true;
      };
      startPg();
      run(exe('createdb'), ['-h', '127.0.0.1', '-p', String(pgPort), '-U', 'pxel', 'pxel'], env);
      child = await startServer(env, base, join(root, 'server.log'));
      const cookie = await login(base);
      const { snapshot } = await createDocument(base, cookie);
      const writer = await connect(base, cookie);
      let receipt;
      try {
        receipt = await writer.send(setCommand(writer, snapshot, 'snapshot source'));
        assert.equal(receipt.case, 'committed');
        assert.equal(receipt.value.committedVersion, 2n);
        const atTwo = await getDocument(base, cookie, snapshot.document_id);
        const source = await writer.send(
          setCommand(writer, atTwo, 'snapshot source 2', { sequence: 2 }),
        );
        assert.equal(source.case, 'committed');
        assert.equal(source.value.committedVersion, 3n);
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
                String(pgPort),
                '-U',
                'pxel',
                '-d',
                'pxel',
                '-tAc',
                activeQuery,
              ],
              env,
              true,
            ) === '1'
          ) {
            active = true;
            break;
          }
          await delay(100);
        }
        assert.ok(active, 'version 3 snapshot did not become ACTIVE');
        const atSnapshot = await getDocument(base, cookie, snapshot.document_id);
        const tail = await writer.send(
          setCommand(writer, atSnapshot, 'committed tail', { sequence: 3 }),
        );
        assert.equal(tail.case, 'committed');
        assert.equal(tail.value.committedVersion, 4n);
        const expectedHash = Buffer.from(tail.value.confirmedStateHash).toString('hex');
        run(pgCtl, [...pgArgs, '-m', 'immediate', '-w', 'stop'], env);
        postgresRunning = false;
        writer.socket.terminate();
        await killServer(child);
        child = undefined;
        startPg();
        child = await startServer(env, base, join(root, 'server.log'));
        const restored = await getDocument(base, cookie, snapshot.document_id);
        assert.equal(restored.version, '4');
        assert.equal(restored.state_hash, expectedHash);
        assert.equal(restored.sheets[0].cells[0].value.value, 'committed tail');
        const recoveries = readFileSync(join(root, 'server.log'), 'utf8')
          .split('\n')
          .filter(Boolean)
          .map((line) => JSON.parse(line))
          .filter((line) => line.fields?.event === 'document_recovery');
        assert.ok(
          recoveries.some(
            (line) => line.fields.snapshot_version === 3 && line.fields.replayed_events === 1,
          ),
          'actor did not report snapshot v3 plus one tail event',
        );
        const historical = await fetch(base + `/api/documents/${snapshot.document_id}/versions/3`, {
          headers: { Origin: 'http://127.0.0.1:5173', Cookie: cookie },
        });
        assert.equal(historical.status, 200);
        assert.equal((await historical.json()).state_hash, atSnapshot.state_hash);
      } finally {
        writer.socket.terminate();
      }
    } finally {
      try {
        if (postgresRunning && child && child.exitCode === null && child.signalCode === null)
          await releaseSessions(base);
      } finally {
        try {
          await killServer(child);
        } finally {
          if (postgresRunning) run(pgCtl, [...pgArgs, '-m', 'immediate', '-w', 'stop'], env);
          const safeRoot = resolve(root);
          if (!safeRoot.startsWith(localRoot + sep))
            throw new Error('unsafe recovery cleanup target');
          if (process.env.PXEL_TEST_KEEP_RECOVERY !== '1')
            rmSync(safeRoot, { recursive: true, force: true });
        }
      }
    }
  },
);
