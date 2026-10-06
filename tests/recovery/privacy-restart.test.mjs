import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { api, createDocument, login, origin, releaseSessions, uuid7 } from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';
test(
  'encrypted privacy policy anonymous links and disclosure stamps survive restart',
  { timeout: 120000 },
  async () => {
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'privacy-restart-'));
    const port = await freePort(),
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
    let server;
    try {
      server = await startServer(environment, base, join(fixture, 'before.log'));
      const cookie = await login(base),
        { snapshot } = await createDocument(base, cookie),
        path = `/api/documents/${snapshot.document_id}`;
      const rules = { enabled: true, types: ['email', 'inn'], exceptions: ['ab'.repeat(32)] };
      const saved = await api(base, cookie, path + '/privacy', 'PUT', {
        schema_version: 1,
        document_version: snapshot.version,
        expected_revision: 0,
        rules,
      });
      assert.equal(saved.status, 200);
      const privacy = await saved.json();
      const created = await api(base, cookie, path + '/shares', 'POST', {
        request_id: uuid7(),
        permission: 'comment',
        ttl: 'hour',
        confirm_indefinite: false,
        pin: '482615',
        anonymized: true,
      });
      assert.equal(created.status, 201);
      const link = await created.json(),
        shared = `/api/shared/${link.share.link_id}`;
      const before = await (await api(base, cookie, path)).json();
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      const after = await (await api(base, cookie, path)).json();
      assert.equal(after.version, before.version);
      assert.equal(after.state_hash, before.state_hash);
      assert.deepEqual(after.calculation_context, before.calculation_context);
      assert.deepEqual(after.capabilities.disclosure_stamp, before.capabilities.disclosure_stamp);
      assert.deepEqual(await (await api(base, cookie, path + '/privacy')).json(), privacy);
      const unlock = await api(base, '', shared + '/session', 'POST', {
        token: link.token,
        secret: '482615',
      });
      assert.equal(unlock.status, 200);
      const guest = unlock.headers.get('set-cookie').split(';')[0];
      const safe = await api(base, guest, shared + '/document');
      assert.equal(safe.status, 200);
      const projection = await safe.json();
      assert.equal(projection.redacted_projection_schema_version, 2);
      assert.equal(projection.privacy_revision, 1);
      assert.equal(projection.capabilities.anonymized, true);
      assert.equal(projection.capabilities.can_edit, false);
      assert.equal(Object.hasOwn(projection, 'calculation_context'), false);
      assert.equal(Object.hasOwn(projection, 'state_hash'), false);
      assert.equal((await api(base, guest, shared + '/undo-history')).status, 403);
      await releaseSessions(base);
    } finally {
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
