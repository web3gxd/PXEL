import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import WebSocket from 'ws';
import { protectLocalPath } from '../../scripts/keyring.mjs';
import {
  api,
  connect,
  createDocument,
  getDocument,
  origin,
  sendPresence,
  setCommand,
  uuid7,
} from '../helpers/client.mjs';
import { exe, freePort, killServer, run, startServer } from './private-postgres.mjs';

test(
  'local accounts enforce first setup, password changes, master access and HTTP/WS revocation',
  { timeout: 180000 },
  async () => {
    assert.ok(process.env.PXEL_PG_BIN);
    mkdirSync('.local', { recursive: true });
    const root = mkdtempSync(resolve('.local/local-auth-'));
    protectLocalPath(root, true);
    const databasePassword = randomBytes(24).toString('hex');
    const bootstrapToken = randomBytes(32).toString('hex');
    const masterPassword = 'Master_' + randomBytes(18).toString('hex');
    const issuedPassword = 'Issued_' + randomBytes(18).toString('hex');
    const userPassword = 'Changed_' + randomBytes(18).toString('hex');
    const resetPassword = 'Reset_' + randomBytes(18).toString('hex');
    const documentMarker = 'Local_auth_document_' + randomBytes(18).toString('hex');
    const passwordFile = join(root, 'password');
    const bootstrapFile = join(root, 'bootstrap-token');
    const keyring = join(root, 'keyring.json');
    for (const [path, value] of [
      [passwordFile, databasePassword],
      [bootstrapFile, bootstrapToken],
      [
        keyring,
        JSON.stringify({
          schema_version: 1,
          active_kek_id: 'local-auth-test',
          keys: [{ id: 'local-auth-test', key_hex: randomBytes(32).toString('hex') }],
        }),
      ],
    ]) {
      writeFileSync(path, value, { mode: 0o600 });
      protectLocalPath(path);
    }
    const binary = join(root, process.platform === 'win32' ? 'server.exe' : 'server');
    copyFileSync(
      resolve(
        process.env.PXEL_TEST_SERVER_BIN ??
          join(
            process.env.CARGO_TARGET_DIR ?? 'target',
            'debug',
            process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
          ),
      ),
      binary,
    );
    const pgPort = await freePort(),
      appPort = await freePort();
    const base = `http://127.0.0.1:${appPort}`,
      data = join(root, 'data');
    const env = {
      ...process.env,
      DATABASE_URL: `postgresql://pxel:${databasePassword}@127.0.0.1:${pgPort}/pxel`,
      PGPASSWORD: databasePassword,
      PXEL_DEV_AUTH: '0',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_ALLOWED_ORIGINS: origin,
      PXEL_KEYRING_FILE: keyring,
      PXEL_BOOTSTRAP_TOKEN_FILE: bootstrapFile,
      PXEL_BLOB_ROOT: join(root, 'blobs'),
      PXEL_TEST_SERVER_BIN: binary,
    };
    delete env.PXEL_KEYRING_JSON;
    let server,
      pgRunning = false,
      client;
    const call = (path, body, cookie, requestOrigin = origin) =>
      fetch(base + path, {
        method: 'POST',
        headers: {
          Origin: requestOrigin,
          'Content-Type': 'application/json',
          ...(cookie ? { Cookie: cookie } : {}),
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(15000),
      });
    const status = async (cookie) => {
      const response = await fetch(base + '/api/auth/status', {
        headers: cookie ? { Cookie: cookie } : {},
        signal: AbortSignal.timeout(5000),
      });
      assert.equal(response.status, 200);
      return response.json();
    };
    const session = async (response, expectedMode) => {
      assert.equal(response.status, 200);
      const cookie = response.headers.get('set-cookie');
      for (const required of [
        'HttpOnly',
        'Secure',
        'SameSite=Strict',
        'Path=/api',
        'Max-Age=28800',
      ])
        assert.ok(cookie.includes(required));
      const body = await response.json();
      assert.equal(body.mode, expectedMode);
      return { cookie: cookie.split(';')[0], account: body.account };
    };
    const login = (username, password) =>
      call('/api/auth/login', { schema_version: 1, username, password });
    const action = (user, action, password, cookie) =>
      call(
        '/api/admin/accounts/' + user,
        { schema_version: 1, action, ...(password ? { password } : {}) },
        cookie,
      );
    const rejectWs = (cookie) =>
      new Promise((resolve, reject) => {
        let responded = false;
        const socket = new WebSocket(base.replace(/^http/, 'ws') + '/api/ws', {
          headers: { Origin: origin, ...(cookie ? { Cookie: cookie } : {}) },
          handshakeTimeout: 5000,
        });
        socket.once('unexpected-response', (_, response) => {
          responded = true;
          try {
            assert.equal(response.statusCode, 401);
            resolve();
          } catch (error) {
            reject(error);
          }
          response.resume();
          socket.terminate();
        });
        socket.once('open', () => {
          socket.terminate();
          reject(new Error('unauthorized websocket opened'));
        });
        socket.on('error', (error) => {
          if (!responded) reject(error);
        });
      });
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
      run(
        exe('pg_ctl'),
        [
          '-D',
          data,
          '-l',
          join(root, 'postgres.log'),
          '-o',
          `-h 127.0.0.1 -p ${pgPort} -c synchronous_commit=on -c fsync=on -c full_page_writes=on`,
          '-w',
          'start',
        ],
        env,
      );
      pgRunning = true;
      run(exe('createdb'), ['-h', '127.0.0.1', '-p', String(pgPort), '-U', 'pxel', 'pxel'], env);
      server = await startServer(env, base, join(root, 'server.log'));
      assert.equal((await status()).mode, 'setup');
      assert.equal((await api(base, '', '/api/documents')).status, 401);
      await rejectWs();
      const setup = {
        schema_version: 1,
        username: 'master',
        password: masterPassword,
        bootstrap_token: bootstrapToken,
      };
      assert.equal(
        (await call('/api/auth/setup', setup, undefined, 'https://foreign.invalid')).status,
        403,
      );
      assert.equal((await call('/api/auth/setup', { ...setup, schema_version: 99 })).status, 400);
      assert.equal(
        (await call('/api/auth/setup', { ...setup, bootstrap_token: 'incorrect' })).status,
        401,
      );
      assert.equal((await call('/api/auth/login', { ...setup, extra: true })).status, 400);
      const results = await Promise.all([
        call('/api/auth/setup', setup),
        call('/api/auth/setup', setup),
      ]);
      assert.deepEqual(results.map((r) => r.status).sort(), [200, 409]);
      const master = await session(
        results.find((r) => r.status === 200),
        'authenticated',
      );
      assert.equal(master.account.is_master, true);
      assert.equal((await status()).mode, 'login');
      assert.equal((await call('/api/auth/setup', setup)).status, 409);
      assert.equal(
        (
          await call(
            '/api/admin/accounts',
            { schema_version: 1, username: 'alice', password: issuedPassword },
            master.cookie,
          )
        ).status,
        204,
      );
      const aliceFirst = await session(await login('alice', issuedPassword), 'password_change');
      assert.equal(aliceFirst.account.is_master, false);
      assert.equal((await api(base, aliceFirst.cookie, '/api/documents')).status, 401);
      assert.equal(
        (
          await api(base, aliceFirst.cookie, '/api/documents', 'POST', {
            document_id: uuid7(),
            operation_id: uuid7(),
          })
        ).status,
        401,
      );
      assert.equal((await api(base, aliceFirst.cookie, '/api/admin/accounts')).status, 401);
      await rejectWs(aliceFirst.cookie);
      assert.equal(
        (
          await call(
            '/api/auth/password',
            { schema_version: 1, current_password: issuedPassword, new_password: issuedPassword },
            aliceFirst.cookie,
          )
        ).status,
        400,
      );
      assert.equal(
        (
          await call(
            '/api/auth/password',
            {
              schema_version: 1,
              current_password: 'Wrong_' + issuedPassword,
              new_password: userPassword,
            },
            aliceFirst.cookie,
          )
        ).status,
        401,
      );
      let alice = await session(
        await call(
          '/api/auth/password',
          { schema_version: 1, current_password: issuedPassword, new_password: userPassword },
          aliceFirst.cookie,
        ),
        'authenticated',
      );
      assert.equal((await status(aliceFirst.cookie)).mode, 'login');
      assert.equal((await login('alice', issuedPassword)).status, 401);
      assert.equal((await api(base, alice.cookie, '/api/admin/accounts')).status, 401);
      assert.equal(
        (
          await call(
            '/api/admin/accounts',
            { schema_version: 1, username: 'rogue', password: issuedPassword },
            alice.cookie,
          )
        ).status,
        401,
      );
      const own = await createDocument(base, alice.cookie);
      const mastered = await getDocument(base, master.cookie, own.intent.document_id);
      assert.equal(mastered.capabilities.can_share, true, 'master must receive owner capabilities');
      assert.equal(
        (
          await api(
            base,
            alice.cookie,
            '/api/documents/' + own.intent.document_id + '/permissions',
            'POST',
            { action: 'revoke', change_id: uuid7(), target_user_id: master.account.user_id },
          )
        ).status,
        403,
      );
      client = await connect(base, master.cookie);
      assert.equal(
        (await client.send(setCommand(client, mastered, documentMarker))).case,
        'committed',
      );
      client.socket.terminate();
      client = undefined;
      const mastersBook = await createDocument(base, master.cookie);
      assert.equal(
        (await api(base, alice.cookie, '/api/documents/' + mastersBook.intent.document_id)).status,
        403,
      );
      assert.equal(
        (await api(base, alice.cookie + '; ' + alice.cookie, '/api/documents')).status,
        401,
      );
      for (const operation of ['disable', 'reset_password'])
        assert.equal(
          (
            await action(
              master.account.user_id,
              operation,
              operation === 'reset_password' ? resetPassword : undefined,
              master.cookie,
            )
          ).status,
          403,
        );
      client = await connect(base, alice.cookie);
      let closed = once(client.socket, 'close', { signal: AbortSignal.timeout(20000) });
      assert.equal(
        (await action(alice.account.user_id, 'reset_password', resetPassword, master.cookie))
          .status,
        204,
      );
      sendPresence(client, own.snapshot);
      await closed;
      client = undefined;
      assert.equal((await api(base, alice.cookie, '/api/documents')).status, 401);
      assert.equal((await login('alice', userPassword)).status, 401);
      const reset = await session(await login('alice', resetPassword), 'password_change');
      alice = await session(
        await call(
          '/api/auth/password',
          { schema_version: 1, current_password: resetPassword, new_password: userPassword },
          reset.cookie,
        ),
        'authenticated',
      );
      client = await connect(base, alice.cookie);
      closed = once(client.socket, 'close', { signal: AbortSignal.timeout(20000) });
      assert.equal(
        (await action(alice.account.user_id, 'disable', undefined, master.cookie)).status,
        204,
      );
      sendPresence(client, own.snapshot);
      await closed;
      client = undefined;
      assert.equal((await login('alice', userPassword)).status, 401);
      assert.equal((await api(base, alice.cookie, '/api/documents')).status, 401);
      assert.equal(
        (await action(alice.account.user_id, 'enable', undefined, master.cookie)).status,
        204,
      );
      assert.equal((await api(base, alice.cookie, '/api/documents')).status, 401);
      alice = await session(await login('alice', userPassword), 'authenticated');
      await getDocument(base, alice.cookie, own.intent.document_id);
      client = await connect(base, alice.cookie);
      closed = once(client.socket, 'close', { signal: AbortSignal.timeout(20000) });
      assert.equal((await call('/api/auth/logout', {}, alice.cookie)).status, 204);
      sendPresence(client, own.snapshot);
      await closed;
      client = undefined;
      assert.equal((await api(base, alice.cookie, '/api/documents')).status, 401);
      for (let attempt = 0; attempt < 5; attempt++)
        assert.equal((await login('alice', issuedPassword)).status, 401);
      assert.equal(
        (await login('alice', userPassword)).status,
        401,
        'fifth failure must lock out even a correct password',
      );
      await killServer(server);
      server = await startServer(env, base, join(root, 'server.log'));
      assert.equal((await status()).mode, 'login');
      assert.equal(
        (await login('alice', userPassword)).status,
        401,
        'lockout must survive restart',
      );
      assert.equal((await status(master.cookie)).mode, 'authenticated');
      await getDocument(base, master.cookie, own.intent.document_id);
      const log = readFileSync(join(root, 'server.log'), 'utf8');
      for (const secret of [
        databasePassword,
        bootstrapToken,
        masterPassword,
        issuedPassword,
        userPassword,
        resetPassword,
        documentMarker,
        master.cookie.split('=')[1],
        alice.cookie.split('=')[1],
      ])
        assert.equal(
          log.includes(secret),
          false,
          'credentials and session tokens must stay out of logs',
        );
    } finally {
      client?.socket.terminate();
      await killServer(server);
      if (pgRunning) run(exe('pg_ctl'), ['-D', data, '-m', 'fast', '-w', 'stop'], env);
    }
  },
);
