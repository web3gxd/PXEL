import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { request as httpRequest } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  api,
  connect,
  createDocument,
  getDocument,
  login,
  origin,
  releaseSessions,
  setCommand,
  formulaCommand,
  uuid7,
} from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';
test(
  'password-file import and settings commit survive restart with fresh identities and safe defaults',
  { timeout: 120000 },
  async () => {
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'pxel-restart-')),
      port = await freePort(),
      base = `http://127.0.0.1:${port}`;
    const environment = {
      ...process.env,
      PXEL_TEST_SERVER_BIN: join(
        resolve(process.env.CARGO_TARGET_DIR ?? 'target'),
        'debug',
        process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
      ),
      PXEL_BIND: `127.0.0.1:${port}`,
      PXEL_DEV_AUTH: '1',
      PXEL_ALLOWED_ORIGINS: origin,
      PXEL_BLOB_ROOT: join(fixture, 'blobs'),
    };
    let server, client;
    try {
      server = await startServer(environment, base, join(fixture, 'before.log'));
      const cookie = await login(base);
      for (const endpoint of ['/api/imports/pxel/preview', '/api/imports/pxel']) {
        for (const [session, requestOrigin, expected] of [
          [undefined, origin, 401],
          [cookie, 'https://foreign.invalid', 403],
        ]) {
          const response = await fetch(base + endpoint, {
            method: 'POST',
            headers: {
              Origin: requestOrigin,
              'Content-Type': 'application/json',
              ...(session ? { Cookie: session } : {}),
            },
            body: '{',
            signal: AbortSignal.timeout(5000),
          });
          assert.equal(response.status, expected, 'authorization must precede JSON parsing');
          const pending = incompleteImport(base, endpoint, session, requestOrigin, 5000);
          try {
            assert.equal(await pending.response, expected, 'rejected bodies must not be read');
          } finally {
            pending.request.destroy();
          }
        }
      }
      const pending = incompleteImport(base, '/api/imports/pxel/preview', cookie, origin, 20000);
      try {
        let busy;
        for (let attempt = 0; attempt < 50; attempt++) {
          busy = await fetch(base + '/api/imports/pxel', {
            method: 'POST',
            headers: { Origin: origin, Cookie: cookie, 'Content-Type': 'application/json' },
            body: '{',
            signal: AbortSignal.timeout(5000),
          });
          if (busy.status === 429) break;
          assert.equal(busy.status, 400);
          await delay(20);
        }
        assert.equal(busy?.status, 429, 'body reads must hold the shared import permit');
        assert.equal(await pending.response, 408, 'incomplete authenticated bodies must time out');
      } finally {
        pending.request.destroy();
      }
      const { snapshot } = await createDocument(base, cookie);
      client = await connect(base, cookie);
      assert.equal((await client.send(setCommand(client, snapshot, 7))).case, 'committed');
      let current = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(
        (await client.send(formulaCommand(client, current, '=A1*2', { column: 1, sequence: 2 })))
          .case,
        'committed',
      );
      const head = await getDocument(base, cookie, snapshot.document_id);
      client.socket.terminate();
      client = undefined;
      const exported = await api(
        base,
        cookie,
        `/api/documents/${head.document_id}/export.pxel`,
        'POST',
        { schema_version: 1, password: 'test password 2026', document_version: head.version },
      );
      assert.equal(exported.status, 200);
      const payload = {
        schema_version: 1,
        password: 'test password 2026',
        file_base64: Buffer.from(await exported.arrayBuffer()).toString('base64'),
        document_id: uuid7(),
        request_id: uuid7(),
      };
      let response = await api(base, cookie, '/api/imports/pxel', 'POST', payload);
      assert.equal(response.status, 201);
      const imported = await response.json(),
        book = await getDocument(base, cookie, imported.document_id);
      assert.notEqual(book.sheets[0].id, head.sheets[0].id);
      assert.ok(
        book.sheets[0].cells.some(
          (c) => c.value.type === 'formula' && c.value.value.current_text === '=(A1*2)',
        ),
      );
      const paths = [
          `/api/documents/${imported.document_id}/personal-view`,
          `/api/documents/${imported.document_id}/privacy`,
          `/api/documents/${imported.document_id}/disclosure-policy`,
        ],
        settings = [];
      for (const path of paths) {
        response = await api(base, cookie, path);
        assert.equal(response.status, 200);
        settings.push(await response.json());
      }
      assert.equal(settings[2].rules.allow_participant_export, false);
      assert.equal(settings[1].rules.enabled, false);
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      const after = await getDocument(base, cookie, imported.document_id);
      assert.equal(after.state_hash, book.state_hash);
      assert.equal(after.version, book.version);
      for (const [i, path] of paths.entries()) {
        response = await api(base, cookie, path);
        assert.deepEqual(await response.json(), settings[i]);
      }
      response = await api(base, cookie, '/api/imports/pxel', 'POST', payload);
      assert.equal(response.status, 201);
      assert.equal((await response.json()).version, imported.version);
      await releaseSessions(base);
    } finally {
      client?.socket.terminate();
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);

function incompleteImport(base, endpoint, cookie, requestOrigin, timeout) {
  let request;
  const response = new Promise((resolveResponse, reject) => {
    request = httpRequest(
      base + endpoint,
      {
        method: 'POST',
        headers: {
          Origin: requestOrigin,
          'Content-Type': 'application/json',
          'Content-Length': String(32 * 1024 * 1024),
          ...(cookie ? { Cookie: cookie } : {}),
        },
        signal: AbortSignal.timeout(timeout),
      },
      (incoming) => {
        incoming.resume();
        resolveResponse(incoming.statusCode);
      },
    );
    request.on('error', reject);
    request.write('{');
  });
  return { request, response };
}
