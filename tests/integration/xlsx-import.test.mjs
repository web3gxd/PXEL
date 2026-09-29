import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { api, getDocument, login } from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const executable = resolve(`target/debug/pxel-server${process.platform === 'win32' ? '.exe' : ''}`);

function importFixture(path, identity) {
  const databaseUrl =
    process.env.DATABASE_URL ?? readFileSync('.local/database-url', 'utf8').trim();
  return spawnSync(
    executable,
    ['xlsx-import', resolve(path), identity.workspace_id, identity.actor_id],
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
