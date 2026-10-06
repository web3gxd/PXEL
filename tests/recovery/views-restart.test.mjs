import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import {
  api,
  bytes,
  connect,
  createDocument,
  login,
  origin,
  releaseSessions,
  uuid7,
} from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';

test(
  'frozen-row durable replay and encrypted private filters survive a hard restart',
  { timeout: 120000 },
  async () => {
    assert.ok(process.env.DATABASE_URL);
    assert.ok(process.env.PXEL_KEYRING_FILE);
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'views-restart-'));
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
    let server, client;
    try {
      server = await startServer(environment, base, join(fixture, 'before.log'));
      const cookie = await login(base),
        { snapshot } = await createDocument(base, cookie);
      const path = `/api/documents/${snapshot.document_id}`,
        sheet = snapshot.sheets[0];
      client = await connect(base, cookie);
      const result = await client.send({
        operationId: bytes(uuid7()),
        documentId: bytes(snapshot.document_id),
        actorId: client.actorId,
        baseVersion: BigInt(snapshot.version),
        clientSequence: 1n,
        setFrozenRows: { sheetId: bytes(sheet.id), rows: 2 },
      });
      assert.equal(result.case, 'committed', `rejection code ${result.value?.code}`);
      client.socket.close();
      client = undefined;
      const frozen = await (await api(base, cookie, path)).json();
      assert.equal(frozen.sheets[0].frozen_rows, 2);
      assert.equal(frozen.version, '2');
      const settings = {
        hardcode_dismissals: ['ab'.repeat(32)],
        filters: [
          {
            id: uuid7(),
            sheet_id: sheet.id,
            first_row_id: sheet.rows[0],
            last_row_id: sheet.rows[3],
            first_column_id: sheet.columns[0],
            last_column_id: sheet.columns[1],
            header_row_id: sheet.rows[0],
            conditions: [{ column_id: sheet.columns[0], predicate: { kind: 'nonblank' } }],
          },
        ],
      };
      const save = await api(base, cookie, `${path}/personal-view`, 'PUT', {
        schema_version: 1,
        expected_revision: 0,
        document_version: 2,
        settings,
      });
      assert.equal(save.status, 200);
      const before = await (await api(base, cookie, path)).json();
      assert.equal(before.state_hash, frozen.state_hash);
      assert.equal(before.version, frozen.version);
      assert.deepEqual(before.calculation_context, frozen.calculation_context);
      const stale = await api(base, cookie, `${path}/personal-view`, 'PUT', {
        schema_version: 1,
        expected_revision: 0,
        document_version: 2,
        settings,
      });
      assert.equal(stale.status, 409);
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'after.log'));
      const restored = await (await api(base, cookie, path)).json();
      assert.equal(restored.version, frozen.version);
      assert.equal(restored.state_hash, frozen.state_hash);
      assert.equal(restored.sheets[0].frozen_rows, 2);
      const view = await (await api(base, cookie, `${path}/personal-view`)).json();
      assert.equal(view.revision, 1);
      assert.deepEqual(view.settings, settings);
      const historyResponse = await api(base, cookie, `${path}/undo-history`);
      assert.equal(historyResponse.status, 200);
      await releaseSessions(base);
    } finally {
      if (client) client.socket.close();
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
