import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { api, connect, getDocument, login, setCommand } from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const browserOrigin = process.env.PXEL_WEB_URL ?? 'http://localhost:5173';
const executable = resolve(
  process.env.PXEL_TEST_SERVER_BINARY ??
    `target/debug/pxel-server${process.platform === 'win32' ? '.exe' : ''}`,
);
const { chromium } = createRequire(resolve('apps/web/package.json'))('@playwright/test');

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

test('Excel shared formula groups import, expand and survive XLSX export/reimport', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const result = importFixture('fixtures/xlsx/excel-shared-formulas.xlsx', identity, [
    '--allow-loss',
  ]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /UnsupportedPackagePart/);
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(result.stdout)?.[1];
  assert.ok(documentId);
  const document = await getDocument(base, owner, documentId);
  const sheet = document.sheets[0];
  const at = (row, column) =>
    sheet.cells.find(
      (cell) => cell.row_id === sheet.rows[row - 1] && cell.column_id === sheet.columns[column - 1],
    );
  for (const address of [
    [2, 5],
    [3, 5],
    [6, 5],
    [2, 6],
    [6, 6],
    [2, 7],
    [6, 7],
  ]) {
    assert.equal(at(...address)?.value.type, 'formula', `expected formula at ${address.join(',')}`);
  }
  assert.match(at(3, 5).value.value.source, /A3.*B\$1.*\$C3.*\$D\$1/);
  assert.match(at(6, 6).value.value.source, /SUM\(A6:B6\)/);
  assert.match(at(6, 7).value.value.source, /"A1 text"/);
  const exported = await api(base, owner, `/api/documents/${documentId}/export.xlsx`);
  assert.equal(exported.status, 200, await exported.clone().text());
  const directory = mkdtempSync(join(tmpdir(), 'pxel-shared-formula-roundtrip-'));
  try {
    const path = join(directory, 'roundtrip.xlsx');
    writeFileSync(path, Buffer.from(await exported.arrayBuffer()));
    const restoredResult = importFixture(path, identity);
    assert.equal(restoredResult.status, 0, restoredResult.stderr);
    const restoredId = /Imported document ([0-9a-f-]+)/i.exec(restoredResult.stdout)?.[1];
    assert.ok(restoredId);
    const restored = await getDocument(base, owner, restoredId);
    const restoredSheet = restored.sheets[0];
    const restoredE3 = restoredSheet.cells.find(
      (cell) =>
        cell.row_id === restoredSheet.rows[2] && cell.column_id === restoredSheet.columns[4],
    );
    assert.equal(restoredE3?.value.type, 'formula');
    assert.match(restoredE3.value.value.source, /A3.*B\$1.*\$C3.*\$D\$1/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('multi-sheet XLSX is committed, replayed and exported with both sheets', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const result = importFixture('fixtures/xlsx/minimal_scalars.xlsx', identity);
  assert.equal(result.status, 0, result.stderr);
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(result.stdout)?.[1];
  assert.ok(documentId);
  const document = await getDocument(base, owner, documentId);
  assert.equal(document.sheets.length, 2);
  assert.equal(document.sheets[0].name, 'Budget');
  assert.equal(document.sheets[1].name, 'Plan');
  assert.equal(
    document.sheets.reduce((count, sheet) => count + sheet.cells.length, 0),
    5,
  );
  assert.equal((await api(base, owner, `/api/documents/${documentId}/export.xlsx`)).status, 200);
  const browser = await chromium.launch();
  try {
    for (const deviceScaleFactor of [1, 2]) {
      const context = await browser.newContext({ deviceScaleFactor });
      try {
        const [name, value] = owner.split('=');
        await context.addCookies([{ name, value, domain: 'localhost', path: '/api' }]);
        const page = await context.newPage();
        await page.goto(`${browserOrigin}/?document=${documentId}`);
        await page.getByRole('button', { name: 'Перейти на лист Plan' }).click();
        assert.match(await page.locator('.sheet-summary').textContent(), /^Plan/);
        const grid = page.getByRole('region', { name: 'Область таблицы' });
        await grid.focus();
        for (let index = 0; index < 25; index++) await page.keyboard.press('ArrowRight');
        for (let index = 0; index < 3; index++) await page.keyboard.press('ArrowDown');
        assert.equal(await page.locator('.cell-address').textContent(), 'Z4');
        assert.equal(
          await page.getByRole('textbox', { name: 'Значение ячейки' }).inputValue(),
          'world!',
        );
      } finally {
        await context.close();
      }
    }
  } finally {
    await browser.close();
  }
});

test('cross-sheet XLSX formula recalculates after a browser edit and survives export', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const imported = importFixture('fixtures/xlsx/cross_sheet_formula.xlsx', identity);
  assert.equal(imported.status, 0, imported.stderr);
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(imported.stdout)?.[1];
  assert.ok(documentId);
  const initial = await getDocument(base, owner, documentId);
  assert.equal(initial.sheets.length, 2);
  assert.equal(initial.sheets[0].cells[0].value.value.source, '=Plan!A1+2');
  const calculated = async (version, expected) => {
    for (let attempt = 0; attempt < 20; attempt++) {
      const response = await api(base, owner, `/api/documents/${documentId}/calculation`);
      if (response.status === 200) {
        const state = await response.json();
        if (state.input_version === version && state.status === 'complete') {
          assert.equal(state.values[0]?.value.value, expected);
          return;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.fail(`Calculation for version ${version} did not complete`);
  };
  await calculated(initial.version, 5);
  const browser = await chromium.launch();
  try {
    const context = await browser.newContext();
    try {
      const [name, value] = owner.split('=');
      await context.addCookies([{ name, value, domain: 'localhost', path: '/api' }]);
      const page = await context.newPage();
      await page.goto(`${browserOrigin}/?document=${documentId}`);
      await page.getByRole('button', { name: 'Перейти на лист Plan' }).click();
      const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
      await bar.fill('9');
      await bar.press('Enter');
      await page.getByTestId('save-status').filter({ hasText: 'Сохранено' }).waitFor();
      await page.getByRole('button', { name: 'Перейти на лист Budget' }).click();
      await page.getByTestId('active-cell').filter({ hasText: 'A1: 11' }).waitFor();
    } finally {
      await context.close();
    }
  } finally {
    await browser.close();
  }
  const edited = await getDocument(base, owner, documentId);
  assert.equal(edited.version, '5');
  await calculated(edited.version, 11);
  const exported = await api(base, owner, `/api/documents/${documentId}/export.xlsx`);
  assert.equal(exported.status, 200);
  const directory = mkdtempSync(join(tmpdir(), 'pxel-cross-sheet-roundtrip-'));
  try {
    const path = join(directory, 'roundtrip.xlsx');
    writeFileSync(path, Buffer.from(await exported.arrayBuffer()));
    const again = importFixture(path, identity);
    assert.equal(again.status, 0, again.stderr);
    const restoredId = /Imported document ([0-9a-f-]+)/i.exec(again.stdout)?.[1];
    assert.ok(restoredId);
    const restored = await getDocument(base, owner, restoredId);
    assert.equal(restored.sheets.length, 2);
    assert.equal(restored.sheets[0].cells[0].value.type, 'formula');
    assert.equal(restored.sheets[0].cells[0].value.value.source, "=('Plan'!A1+2)");
    assert.equal(restored.sheets[1].cells[0].value.value, 9);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('merge ranges import, export and reimport without dropping cells', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const imported = importFixture('fixtures/xlsx/single_sheet_merge.xlsx', identity);
  assert.equal(imported.status, 0, imported.stderr);
  assert.equal(imported.stderr, '');
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(imported.stdout)?.[1];
  assert.ok(documentId);
  const document = await getDocument(base, owner, documentId);
  assert.equal(document.sheets[0].cells.length, 4);
  assert.deepEqual(document.sheets[0].merges, [
    {
      first_row_id: document.sheets[0].rows[0],
      last_row_id: document.sheets[0].rows[0],
      first_column_id: document.sheets[0].columns[4],
      last_column_id: document.sheets[0].columns[5],
    },
  ]);
  const exported = await api(base, owner, `/api/documents/${documentId}/export.xlsx`);
  assert.equal(exported.status, 200);
  const directory = mkdtempSync(join(tmpdir(), 'pxel-merge-roundtrip-'));
  try {
    const path = join(directory, 'roundtrip.xlsx');
    writeFileSync(path, Buffer.from(await exported.arrayBuffer()));
    const restoredImport = importFixture(path, identity);
    assert.equal(restoredImport.status, 0, restoredImport.stderr);
    const restoredId = /Imported document ([0-9a-f-]+)/i.exec(restoredImport.stdout)?.[1];
    assert.ok(restoredId);
    const restored = await getDocument(base, owner, restoredId);
    const mergeCoordinates = (sheet) =>
      sheet.merges.map((merge) => ({
        first_row: sheet.rows.indexOf(merge.first_row_id),
        last_row: sheet.rows.indexOf(merge.last_row_id),
        first_column: sheet.columns.indexOf(merge.first_column_id),
        last_column: sheet.columns.indexOf(merge.last_column_id),
      }));
    assert.deepEqual(mergeCoordinates(restored.sheets[0]), mergeCoordinates(document.sheets[0]));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
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

test('styled XLSX survives durable import, export and reimport including a styled blank', async () => {
  const owner = await login(base);
  const identity = await (await api(base, owner, '/api/dev/identity')).json();
  const imported = importFixture('fixtures/xlsx/styled_profile.xlsx', identity);
  assert.equal(imported.status, 0, imported.stderr);
  const documentId = /Imported document ([0-9a-f-]+)/i.exec(imported.stdout)?.[1];
  assert.ok(documentId);
  const document = await getDocument(base, owner, documentId);
  const sheet = document.sheets[0];
  assert.equal(sheet.name, 'Profile');
  assert.equal(sheet.cells.length, 2);
  assert.equal(sheet.styles.length, 3);
  const expectedStyle = {
    font_family: 'Courier New',
    font_size_hundredth_points: 1200,
    font_bold: true,
    font_italic: true,
    font_underline: 'double',
    font_strike: true,
    font_color: [255, 128, 0],
    fill: [12, 34, 56],
    border_top: { line: 'double', color: [64, 96, 128] },
    border_right: { line: 'double', color: [64, 96, 128] },
    border_bottom: { line: 'double', color: [64, 96, 128] },
    border_left: { line: 'double', color: [64, 96, 128] },
    horizontal: 'center',
    vertical: 'center',
    text_overflow: 'wrap',
    number_format: 'decimal2',
  };
  for (const cell of sheet.styles) assert.deepEqual(cell.style, expectedStyle);
  const exported = await api(base, owner, `/api/documents/${documentId}/export.xlsx`);
  assert.equal(exported.status, 200);
  assert.match(exported.headers.get('content-type') ?? '', /spreadsheetml\.sheet/);
  const directory = mkdtempSync(join(tmpdir(), 'pxel-styled-xlsx-roundtrip-'));
  try {
    const path = join(directory, 'roundtrip.xlsx');
    const bytes = Buffer.from(await exported.arrayBuffer());
    assert.equal(bytes.subarray(0, 2).toString('ascii'), 'PK');
    writeFileSync(path, bytes);
    const second = importFixture(path, identity);
    assert.equal(second.status, 0, second.stderr);
    const restoredId = /Imported document ([0-9a-f-]+)/i.exec(second.stdout)?.[1];
    assert.ok(restoredId);
    const restored = await getDocument(base, owner, restoredId);
    const restoredSheet = restored.sheets[0];
    assert.equal(restoredSheet.styles.length, 3);
    for (const cell of restoredSheet.styles) assert.deepEqual(cell.style, expectedStyle);
    assert.equal(restoredSheet.cells.length, 2);
    assert.equal(
      restoredSheet.cells.find((cell) => cell.column_id === restoredSheet.columns[0])?.value.value,
      'Profile style',
    );
    assert.equal(
      restoredSheet.cells.find((cell) => cell.column_id === restoredSheet.columns[2])?.value.value
        .source,
      '=(1+2)',
    );
    assert.ok(
      restoredSheet.styles.some(
        (cell) =>
          cell.row_id === restoredSheet.rows[0] && cell.column_id === restoredSheet.columns[1],
      ),
      'the style on an otherwise empty B1 must survive the round trip',
    );

    const browser = await chromium.launch();
    try {
      const context = await browser.newContext({ deviceScaleFactor: 1 });
      try {
        const [name, value] = owner.split('=');
        await context.addCookies([{ name, value, domain: 'localhost', path: '/api' }]);
        const page = await context.newPage();
        await page.goto(`${browserOrigin}/?document=${restoredId}`);
        await page.getByRole('button', { name: 'Полужирный', exact: true }).waitFor();
        await page.getByTestId('save-status').filter({ hasText: 'Сохранено' }).waitFor();
        assert.equal(
          await page
            .getByRole('button', { name: 'Полужирный', exact: true })
            .getAttribute('aria-pressed'),
          'true',
        );
        const blankFill = await page.locator('canvas').evaluate((element) => {
          const canvas = element;
          const context = canvas.getContext('2d');
          if (!context) throw new Error('Canvas 2D context is unavailable');
          const rect = canvas.getBoundingClientRect();
          const scaleX = canvas.width / rect.width;
          const scaleY = canvas.height / rect.height;
          const pixel = context.getImageData(
            Math.round((46 + 112 + 56) * scaleX),
            Math.round((28 + 14) * scaleY),
            1,
            1,
          ).data;
          return Array.from(pixel);
        });
        assert.deepEqual(blankFill, [12, 34, 56, 255]);
      } finally {
        await context.close();
      }
    } finally {
      await browser.close();
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
