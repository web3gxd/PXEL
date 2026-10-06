import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { api, getDocument, login, origin, releaseSessions, uuid7 } from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';

test(
  'encrypted catalog title favorites layout and atomic templates survive hard restart',
  { timeout: 120000 },
  async () => {
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'library-restart-')),
      port = await freePort(),
      base = `http://127.0.0.1:${port}`;
    const environment = {
      ...process.env,
      PXEL_TEST_SERVER_BIN:
        process.env.PXEL_LIBRARY_SERVER_BIN ??
        join(
          resolve(process.env.CARGO_TARGET_DIR ?? 'target'),
          'debug',
          process.platform === 'win32' ? 'pxel-server.exe' : 'pxel-server',
        ),
      PXEL_BIND: `127.0.0.1:${port}`,
      PXEL_DEV_AUTH: '1',
      PXEL_ALLOWED_ORIGINS: origin,
      PXEL_BLOB_ROOT: join(fixture, 'blobs'),
    };
    let server;
    try {
      server = await startServer(environment, base, join(fixture, 'before.log'));
      const cookie = await login(base, 'library_' + uuid7().replaceAll('-', '').slice(-20));
      const intent = {
        schema_version: 1,
        template: 'budget',
        document_id: uuid7(),
        request_id: uuid7(),
      };
      let response = await api(base, cookie, '/api/library/templates', 'POST', intent);
      assert.equal(response.status, 201);
      const receipt = await response.json(),
        id = receipt.document_id,
        path = `/api/documents/${id}`;
      const head = await getDocument(base, cookie, id);
      assert.equal(head.sheets[0].frozen_rows, 2);
      assert.equal(head.sheets[0].merges.length, 1);
      response = await api(base, cookie, path + '/metadata', 'PUT', {
        schema_version: 1,
        expected_revision: 1,
        title: 'Restart catalog title',
      });
      assert.equal(response.status, 200);
      const metadata = await response.json();
      response = await api(base, cookie, path + '/favorite', 'PUT', {
        schema_version: 1,
        expected_revision: 0,
        favorite: true,
      });
      assert.equal(response.status, 200);
      response = await api(base, cookie, path + '/opened', 'POST', { schema_version: 1 });
      assert.equal(response.status, 200);
      response = await api(base, cookie, '/api/library/layout', 'PUT', {
        schema_version: 1,
        expected_revision: 0,
        layout: 'list',
      });
      assert.equal(response.status, 200);
      const before = await (await api(base, cookie, '/api/documents?schema_version=1')).json();
      assert.equal(before.documents.length, 1);
      assert.equal(before.documents[0].preference.favorite, true);
      assert.ok(before.documents[0].preference.opened_at > 0);
      assert.equal(before.layout.layout, 'list');
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      assert.deepEqual(
        await (await api(base, cookie, '/api/documents?schema_version=1')).json(),
        before,
      );
      assert.deepEqual(await (await api(base, cookie, path + '/metadata')).json(), metadata);
      const after = await getDocument(base, cookie, id);
      assert.equal(after.version, head.version);
      assert.equal(after.state_hash, head.state_hash);
      response = await api(base, cookie, '/api/library/templates', 'POST', intent);
      assert.equal(response.status, 201);
      assert.deepEqual(await response.json(), receipt);
      assert.equal(
        (await (await api(base, cookie, path + '/metadata')).json()).title,
        metadata.title,
      );
      await releaseSessions(base);
    } finally {
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
