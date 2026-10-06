import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { api, createDocument, login, origin, releaseSessions } from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';

test(
  'encrypted disclosure policy and allowed/denied audit survive a hard server restart',
  { timeout: 120000 },
  async () => {
    assert.ok(process.env.DATABASE_URL);
    assert.ok(process.env.PXEL_KEYRING_FILE);
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'disclosure-restart-'));
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
      const initial = snapshot.capabilities.disclosure_stamp;
      assert.equal(
        (
          await api(base, cookie, `${path}/clipboard/authorize`, 'POST', {
            schema_version: 1,
            action: 'copy',
            stamp: initial,
          })
        ).status,
        200,
      );
      const policy = await (await api(base, cookie, `${path}/disclosure-policy`)).json();
      const saved = await api(base, cookie, `${path}/disclosure-policy`, 'PUT', {
        schema_version: 1,
        expected_revision: 0,
        rules: { ...policy.rules, allow_copy: false },
      });
      assert.equal(saved.status, 200);
      const current = await (await api(base, cookie, path)).json();
      assert.equal(current.version, snapshot.version);
      assert.equal(
        (
          await api(base, cookie, `${path}/clipboard/authorize`, 'POST', {
            schema_version: 1,
            action: 'cut',
            stamp: current.capabilities.disclosure_stamp,
          })
        ).status,
        403,
      );
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      const restored = await (await api(base, cookie, path)).json();
      assert.equal(restored.capabilities.can_copy, false);
      assert.equal(restored.version, snapshot.version);
      const audit = await (await api(base, cookie, `${path}/audit`)).json();
      assert.equal(audit.schema_version, 1);
      assert.equal(audit.events.length, 2);
      assert.deepEqual(
        audit.events.map((event) => event.allowed),
        [false, true],
      );
      await releaseSessions(base);
    } finally {
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
