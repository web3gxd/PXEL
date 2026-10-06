import { expect, test, type Page } from '@playwright/test';

const browserErrors = new WeakMap<Page, string[]>();
test.beforeEach(({ page }) => {
  const errors: string[] = [];
  browserErrors.set(page, errors);
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
});
test.afterEach(({ page }) => {
  expect(browserErrors.get(page)).toEqual([]);
});

async function createSheet(page: Page) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  return new URL(page.url()).searchParams.get('document')!;
}

async function stateAt(page: Page, id: string, version: string) {
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe(version);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  return (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0];
}

test('range paste fills the selection in one version and undoes after reload', async ({ page }) => {
  const id = await createSheet(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('Shift+ArrowDown');
  await page.keyboard.press('Shift+ArrowDown');
  await page.evaluate(() => navigator.clipboard.writeText('7'));
  await page.keyboard.press('ControlOrMeta+V');
  const sheet = await stateAt(page, id, '2');
  expect(sheet.cells).toHaveLength(6);
  expect(
    sheet.cells.every(
      (cell: { value: unknown }) =>
        JSON.stringify(cell.value) === JSON.stringify({ type: 'number', value: 7 }),
    ),
  ).toBe(true);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Отменить последнюю правку' }).click();
  expect((await stateAt(page, id, '3')).cells).toHaveLength(0);
  await page.getByRole('button', { name: 'Повторить отменённую правку' }).click();
  expect((await stateAt(page, id, '4')).cells).toHaveLength(6);
});

test('Ctrl+D fills down mixed formulas values and blanks atomically with Undo and Redo', async ({
  page,
}) => {
  const id = await createSheet(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await page.evaluate(() => navigator.clipboard.writeText('3\t1\t\n9\t9\t9\n8\t8\t8'));
  await page.keyboard.press('ControlOrMeta+V');
  await stateAt(page, id, '2');
  await page.keyboard.press('ArrowRight');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=A1+$A$1');
  await bar.press('Enter');
  const before = await stateAt(page, id, '3');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 3');
  await grid.focus();
  for (let i = 0; i < 2; i++) await page.keyboard.press('Shift+ArrowRight');
  for (let i = 0; i < 2; i++) await page.keyboard.press('Shift+ArrowDown');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено A1:C3');
  await page.keyboard.press('ControlOrMeta+D');
  const after = await stateAt(page, id, '4');
  expect(after.cells).toHaveLength(6);
  for (let row = 1; row < 3; row++) {
    expect(
      after.cells.find(
        (cell: { row_id: string; column_id: string }) =>
          cell.row_id === after.rows[row] && cell.column_id === after.columns[0],
      ).value,
    ).toEqual({ type: 'number', value: 3 });
    expect(
      after.cells.find(
        (cell: { row_id: string; column_id: string }) =>
          cell.row_id === after.rows[row] && cell.column_id === after.columns[1],
      ).value.value.current_text,
    ).toContain(`A${row + 1}+$A$1`);
  }
  await page.screenshot({ path: '../../.local/range-fill.png' });
  await page.keyboard.press('ControlOrMeta+Z');
  expect((await stateAt(page, id, '5')).cells).toEqual(before.cells);
  await page.keyboard.press('ControlOrMeta+Y');
  const redone = await stateAt(page, id, '6');
  expect(redone.cells.map((cell: { value: unknown }) => cell.value)).toEqual(
    after.cells.map((cell: { value: unknown }) => cell.value),
  );
  await page.reload();
  expect((await stateAt(page, id, '6')).cells).toEqual(redone.cells);
});

test('single copied formula fills a destination selection with relative and absolute references', async ({
  page,
}) => {
  const id = await createSheet(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=B1+$B$1');
  await bar.press('Enter');
  await stateAt(page, id, '2');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ControlOrMeta+C');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Shift+ArrowDown');
  await page.keyboard.press('ControlOrMeta+V');
  const sheet = await stateAt(page, id, '3');
  expect(sheet.cells).toHaveLength(3);
  for (let row = 0; row < 3; row++)
    expect(
      sheet.cells.find((cell: { row_id: string }) => cell.row_id === sheet.rows[row]).value.value
        .current_text,
    ).toContain(`B${row + 1}+$B$1`);
});

test('mixed copied block tiles across the selected rectangle in one version', async ({ page }) => {
  const id = await createSheet(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('3');
  await bar.press('Enter');
  await stateAt(page, id, '2');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1+$A$1');
  await bar.press('Enter');
  await stateAt(page, id, '3');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 3');
  await grid.focus();
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('ControlOrMeta+C');
  for (let i = 0; i < 2; i++) await page.keyboard.press('ArrowRight');
  for (let i = 0; i < 3; i++) await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('Shift+ArrowDown');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено D1:G2');
  await page.keyboard.press('ControlOrMeta+V');
  const sheet = await stateAt(page, id, '4');
  expect(sheet.cells).toHaveLength(10);
  for (let row = 0; row < 2; row++) {
    for (const column of [3, 5]) {
      const cell = sheet.cells.find(
        (cell: { row_id: string; column_id: string }) =>
          cell.row_id === sheet.rows[row] && cell.column_id === sheet.columns[column],
      );
      expect(cell.value).toEqual({ type: 'number', value: 3 });
    }
    for (const [column, reference] of [
      [4, 'D'],
      [6, 'F'],
    ] as const) {
      const cell = sheet.cells.find(
        (cell: { row_id: string; column_id: string }) =>
          cell.row_id === sheet.rows[row] && cell.column_id === sheet.columns[column],
      );
      expect(cell.value.value.current_text).toContain(`${reference}${row + 1}+$A$1`);
    }
  }
});

test('Ctrl+D fills a single cell from above', async ({ page }) => {
  const id = await createSheet(page);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('5');
  await bar.press('Enter');
  await stateAt(page, id, '2');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('A2: ');
  await page.keyboard.press('ControlOrMeta+D');
  const sheet = await stateAt(page, id, '3');
  expect(sheet.cells).toHaveLength(2);
  await expect(page.getByTestId('active-cell')).toHaveText('A2: 5');
});
