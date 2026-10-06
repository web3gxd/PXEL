import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
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
  formulaCommand as setFormulaCommand,
  insertRowsCommand,
  uuid7,
} from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';
test(
  'owner action journal and fixed calculated history diff survive restart without mutations',
  { timeout: 120000 },
  async () => {
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'history-restart-')),
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
      const cookie = await login(base),
        { snapshot } = await createDocument(base, cookie),
        path = `/api/documents/${snapshot.document_id}`;
      client = await connect(base, cookie);
      assert.equal((await client.send(setCommand(client, snapshot, 7))).case, 'committed');
      let current = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(
        (await client.send(setFormulaCommand(client, current, '=A1*2', { column: 1, sequence: 2 })))
          .case,
        'committed',
      );
      const before = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(
        (await client.send(setCommand(client, before, 8, { sequence: 3 }))).case,
        'committed',
      );
      current = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(
        (
          await client.send(
            insertRowsCommand(client, current, current.sheets[0].rows[0], [uuid7()], {
              sequence: 4,
            }),
          )
        ).case,
        'committed',
      );
      const head = await getDocument(base, cookie, snapshot.document_id);
      client.socket.terminate();
      client = undefined;
      const paths = [
        `${path}/history?schema_version=1&through_version=${head.version}`,
        `${path}/history/${before.version}`,
        `${path}/diff?schema_version=1&from_version=${before.version}&to_version=${head.version}`,
        path + '/undo-history',
      ];
      const expected = [];
      for (const url of paths) {
        const r = await api(base, cookie, url);
        assert.equal(r.status, 200);
        expected.push(await r.json());
      }
      assert.equal(expected[1].projection.version, before.version);
      assert.equal(expected[1].calculated[0].value.value, 14);
      assert.equal(expected[2].structural_context, true);
      assert.ok(expected[2].changes.some((c) => c.category === 'formula' && c.structural_context));
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      for (const [i, url] of paths.entries()) {
        const r = await api(base, cookie, url);
        assert.equal(r.status, 200);
        assert.deepEqual(await r.json(), expected[i]);
      }
      const after = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(after.version, head.version);
      assert.equal(after.state_hash, head.state_hash);
      assert.deepEqual(after.calculation_context, head.calculation_context);
      await releaseSessions(base);
    } finally {
      client?.socket.terminate();
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
