import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { test } from 'node:test';
import {
  api,
  bytes,
  connect,
  createDocument,
  getDocument,
  login,
  origin,
  releaseSessions,
  setCommand,
  undoCellCommand,
  uuid7,
} from '../helpers/client.mjs';
import { freePort, killServer, startServer } from './private-postgres.mjs';

test(
  'atomic sort and its exact Undo Redo preimages survive hard restarts',
  { timeout: 120000 },
  async () => {
    assert.ok(process.env.DATABASE_URL);
    assert.ok(process.env.PXEL_KEYRING_FILE);
    const root = resolve('.local');
    mkdirSync(root, { recursive: true });
    const fixture = mkdtempSync(join(root, 'sort-restart-'));
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
      let current = snapshot;
      client = await connect(base, cookie);
      for (const [row, value] of [9, 3, 1, 2].entries()) {
        assert.equal(
          (await client.send(setCommand(client, current, value, { row, sequence: row + 1 }))).case,
          'committed',
        );
        current = await getDocument(base, cookie, snapshot.document_id);
      }
      const before = current,
        sheet = before.sheets[0],
        operationId = uuid7();
      const sort = {
        operationId: bytes(operationId),
        documentId: bytes(before.document_id),
        actorId: client.actorId,
        baseVersion: BigInt(before.version),
        clientSequence: 5n,
        sortRange: {
          schemaVersion: 1,
          source: {
            sheetId: bytes(sheet.id),
            rowId: bytes(sheet.rows[0]),
            columnId: bytes(sheet.columns[0]),
          },
          rows: 4,
          columns: 1,
          headerRows: 1,
          keys: [{ columnId: bytes(sheet.columns[0]), descending: false }],
        },
      };
      assert.equal((await client.send(sort)).case, 'committed');
      const sorted = await getDocument(base, cookie, before.document_id);
      const values = (book) =>
        sheet.rows
          .slice(0, 4)
          .map(
            (row) =>
              book.sheets[0].cells.find(
                (cell) => cell.row_id === row && cell.column_id === sheet.columns[0],
              )?.value.value,
          );
      assert.deepEqual(values(sorted), [9, 1, 2, 3]);
      assert.equal(BigInt(sorted.version), BigInt(before.version) + 1n);
      client.socket.terminate();
      client = undefined;
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'sorted.log'));
      const recovered = await getDocument(base, cookie, before.document_id);
      assert.equal(recovered.state_hash, sorted.state_hash);
      assert.deepEqual(recovered.calculation_context, before.calculation_context);
      client = await connect(base, cookie);
      const undo = undoCellCommand(client, recovered, bytes(operationId));
      assert.equal((await client.send(undo)).case, 'committed');
      const undone = await getDocument(base, cookie, before.document_id);
      assert.deepEqual(undone.sheets[0].cells, before.sheets[0].cells);
      client.socket.terminate();
      client = undefined;
      await killServer(server);
      server = undefined;
      server = await startServer(environment, base, join(fixture, 'undone.log'));
      current = await getDocument(base, cookie, before.document_id);
      assert.equal(current.state_hash, undone.state_hash);
      client = await connect(base, cookie);
      assert.equal(
        (await client.send(undoCellCommand(client, current, undo.operationId))).case,
        'committed',
      );
      assert.deepEqual(values(await getDocument(base, cookie, before.document_id)), [9, 1, 2, 3]);
      client.socket.close();
      client = undefined;
      assert.equal(
        (await api(base, cookie, `/api/documents/${before.document_id}/undo-history`)).status,
        200,
      );
      await releaseSessions(base);
    } finally {
      if (client) client.socket.terminate();
      if (server) await killServer(server);
      assert.ok(fixture.startsWith(root + sep));
      rmSync(fixture, { recursive: true, force: true });
    }
  },
);
