import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { api, connect, getDocument, login, setCommand } from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const executable = resolve(`target/debug/pxel-server${process.platform === 'win32' ? '.exe' : ''}`);

function importFixture(path, identity, extraArguments = []) {
  const databaseUrl =
    process.env.DATABASE_URL ?? readFileSync('.local/database-url', 'utf8').trim();
  return spawnSync(
    executable,
    ['xlsx-import', resolve(path), identity.workspace_id, identity.actor_id, ...extraArguments],
    {
      encoding: 'utf8',
      timeout: 30_000,
      env: { ...process.env, DATABASE_URL: databaseUrl, PXEL_DEV_AUTH: '1' },
      windowsHide: true,
    },
  );
}

test('local XLSX import publishes one replayable document atomically', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const result = importFixture('fixtures/xlsx/single_sheet_scalars.xlsx', identity);
  assert.equal(result.status, 0, result.stderr);
  const match = /^Imported document ([0-9a-f-]+) at version 3 \(3 committed events\)\s*$/i.exec(
    result.stdout,
  );
  assert.ok(match, result.stdout);
  const document = await getDocument(base, owner, match[1]);
  assert.equal(document.version, '3');
  assert.equal(document.sheets.length, 1);
  assert.equal(document.sheets[0].name, 'Budget');
  assert.equal(document.sheets[0].cells.length, 4);
  assert.ok(document.sheets[0].cells.some((cell) => cell.value.value === 'hello & world'));
  assert.ok(
    document.sheets[0].cells.some(
      (cell) => cell.value.type === 'formula' && cell.value.value.source === '=A1+B1',
    ),
  );
  assert.equal((await api(base, owner, `/api/documents/${match[1]}/export.xlsx`)).status, 200);
});

test('unsupported multi-sheet XLSX is rejected without publishing a document', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const result = importFixture('fixtures/xlsx/minimal_scalars.xlsx', identity);
  assert.notEqual(result.status, 0);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, /supported import profile/);
  assert.doesNotMatch(result.stderr, /hello|world/);
});

test('merge metadata is reported and cannot be silently dropped during import', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const result = importFixture('fixtures/xlsx/single_sheet_merge.xlsx', identity);
  assert.notEqual(result.status, 0);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, /WorksheetMetadata/);
  assert.match(result.stderr, /would lose unsupported features/);
  assert.doesNotMatch(result.stderr, /hello|world/);
  const accepted = importFixture('fixtures/xlsx/single_sheet_merge.xlsx', identity, [
    '--allow-loss',
  ]);
  assert.equal(accepted.status, 0, accepted.stderr);
  assert.match(accepted.stderr, /WorksheetMetadata/);
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(accepted.stdout)?.[1];
  assert.ok(documentId);
  const document = await getDocument(base, owner, documentId);
  assert.equal(document.sheets[0].cells.length, 4);
});

test('supported XLSX survives import, edit, export and reimport', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const first = importFixture('fixtures/xlsx/single_sheet_scalars.xlsx', identity);
  assert.equal(first.status, 0, first.stderr);
  const firstId = /Imported document ([0-9a-f-]+)/i.exec(first.stdout)?.[1];
  assert.ok(firstId);
  const initial = await getDocument(base, owner, firstId);
  const client = await connect(base, owner);
  try {
    assert.equal((await client.send(setCommand(client, initial, 41))).case, 'committed');
  } finally {
    client.socket.terminate();
  }
  const edited = await getDocument(base, owner, firstId);
  assert.equal(edited.version, '4');
  const exported = await api(base, owner, `/api/documents/${firstId}/export.xlsx`);
  assert.equal(exported.status, 200);
  const directory = mkdtempSync(join(tmpdir(), 'pxel-xlsx-roundtrip-'));
  try {
    const path = join(directory, 'roundtrip.xlsx');
    writeFileSync(path, Buffer.from(await exported.arrayBuffer()));
    const second = importFixture(path, identity);
    assert.equal(second.status, 0, second.stderr);
    const secondId = /Imported document ([0-9a-f-]+)/i.exec(second.stdout)?.[1];
    assert.ok(secondId);
    const restored = await getDocument(base, owner, secondId);
    const authored = (document) => {
      const sheet = document.sheets[0];
      return sheet.cells
        .map((cell) => ({
          row: sheet.rows.indexOf(cell.row_id),
          column: sheet.columns.indexOf(cell.column_id),
          type: cell.value.type,
          value: cell.value.type === 'formula' ? cell.value.value.source : cell.value.value,
        }))
        .sort((a, b) => a.row - b.row || a.column - b.column);
    };
    const before = authored(edited);
    const after = authored(restored);
    assert.equal(before.find((cell) => cell.type === 'formula')?.value, '=A1+B1');
    assert.equal(after.find((cell) => cell.type === 'formula')?.value, '=(A1+B1)');
    assert.deepEqual(
      after.map((cell) => (cell.type === 'formula' ? { ...cell, value: 'formula' } : cell)),
      before.map((cell) => (cell.type === 'formula' ? { ...cell, value: 'formula' } : cell)),
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
