import { expect, test, type APIResponse, type Page } from '@playwright/test';
import { selectValue } from './select';

const Buffer = (
  globalThis as unknown as {
    process: {
      getBuiltinModule(name: 'buffer'): {
        Buffer: { from(value: string): Awaited<ReturnType<APIResponse['body']>> };
      };
    };
  }
).process.getBuiltinModule('buffer').Buffer;
const faults = new WeakMap<Page, string[]>();

test.beforeEach(async ({ page, context }) => {
  const errors: string[] = [];
  faults.set(page, errors);
  page.on('pageerror', (error) => errors.push(error.message));
  await context.clearCookies();
  const origin = String(test.info().project.use.baseURL);
  expect(
    (
      await page.request.post('/api/dev/session', {
        headers: {
          Origin: new URL(origin).origin,
          'X-PXEL-DEV-ACCOUNT': 'csv_' + crypto.randomUUID().replaceAll('-', '').slice(0, 20),
        },
      })
    ).status(),
  ).toBe(204);
});
test.afterEach(async ({ page }) => {
  await page.request.delete('/api/dev/session', {
    headers: { Origin: new URL(String(test.info().project.use.baseURL)).origin },
  });
  expect(faults.get(page)).toEqual([]);
});

test('CSV preview converts selected columns and commits a new book that survives reload', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Импорт CSV', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Импорт CSV', exact: true });
  await dialog.getByLabel('CSV-файл', { exact: true }).setInputFiles({
    name: 'Учёт.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from('\uFEFFКод;Сумма;Флаг\r\n001;12.50;true\r\n=SUM(A1);-3;false\r\n'),
  });
  await selectValue(dialog.getByLabel('Разделитель', { exact: true }), 'semicolon');
  await dialog.getByLabel('Первая строка содержит заголовки').check();
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('table', { name: 'Предпросмотр CSV' })).toBeVisible();
  await selectValue(dialog.getByLabel('Тип столбца 2', { exact: true }), 'number');
  await selectValue(dialog.getByLabel('Тип столбца 3', { exact: true }), 'boolean');
  await expect(dialog.getByRole('button', { name: 'Создать книгу', exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Создать книгу', exact: true })).toBeEnabled();
  await page.screenshot({ path: test.info().outputPath('csv-preview.png') });
  await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).click();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const id = new URL(page.url()).searchParams.get('document');
  expect(id).toBeTruthy();
  const head = await (await page.request.get('/api/documents/' + id)).json();
  const sheet = head.sheets[0];
  const at = (row: number, col: number) =>
    sheet.cells.find(
      (c: { row_id: string; column_id: string }) =>
        c.row_id === sheet.rows[row] && c.column_id === sheet.columns[col],
    ).value;
  expect(at(1, 0)).toEqual({ type: 'text', value: '001' });
  expect(at(1, 1)).toEqual({ type: 'number', value: 12.5 });
  expect(at(1, 2)).toEqual({ type: 'boolean', value: true });
  expect(at(2, 0)).toEqual({ type: 'text', value: '=SUM(A1)' });
  expect(at(0, 1)).toEqual({ type: 'text', value: 'Сумма' });
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
  expect((await (await page.request.get('/api/documents/' + id)).json()).state_hash).toBe(
    head.state_hash,
  );
  const address = page.getByRole('textbox', { name: 'Ячейка или диапазон', exact: true });
  for (const cell of ['A2', 'A3']) {
    await address.fill(cell);
    await address.press('Enter');
    await page.getByRole('textbox', { name: 'Значение ячейки' }).click();
    await address.click();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  }
  expect((await (await page.request.get('/api/documents/' + id)).json()).state_hash).toBe(
    head.state_hash,
  );
});

test('retry after a lost committed response creates exactly one book', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Импорт CSV', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Импорт CSV', exact: true });
  await dialog.getByLabel('CSV-файл', { exact: true }).setInputFiles({
    name: 'retry.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from('001,=SUM(A1)'),
  });
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Создать книгу', exact: true })).toBeEnabled();
  let firstBody = '';
  let committedId = '';
  let lost = false;
  let release = () => {};
  const responseGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/api/imports/csv', async (route) => {
    if (!lost) {
      firstBody = route.request().postData() ?? '';
      const committed = await route.fetch();
      expect(committed.status()).toBe(201);
      committedId = (await committed.json()).document_id;
      lost = true;
      await responseGate;
      await route.abort('failed');
    } else {
      expect(route.request().postData()).toBe(firstBody);
      await route.continue();
    }
  });
  await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).click();
  try {
    await expect.poll(() => lost).toBe(true);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeVisible();
  } finally {
    release();
  }
  await expect(dialog.getByRole('alert')).toBeVisible();
  await expect(dialog.getByLabel('CSV-файл', { exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Создать книгу', exact: true }).click();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  expect(new URL(page.url()).searchParams.get('document')).toBe(committedId);
  const catalog = await (await page.request.get('/api/documents?schema_version=1')).json();
  expect(catalog.documents).toHaveLength(1);
});

test('malformed and mistyped CSV refuse publication without adding books', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Импорт CSV', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Импорт CSV', exact: true });
  await dialog
    .getByLabel('CSV-файл', { exact: true })
    .setInputFiles({ name: 'bad.csv', mimeType: 'text/csv', buffer: Buffer.from('a,"unfinished') });
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('CSV');
  await expect(dialog.getByRole('button', { name: 'Создать книгу', exact: true })).toBeDisabled();
  await dialog.getByLabel('CSV-файл', { exact: true }).setInputFiles({
    name: 'types.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from('not-a-number'),
  });
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('table', { name: 'Предпросмотр CSV' })).toBeVisible();
  await selectValue(dialog.getByLabel('Тип столбца 1', { exact: true }), 'number');
  await dialog.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('тип');
  const catalog = await (await page.request.get('/api/documents?schema_version=1')).json();
  expect(catalog.documents).toHaveLength(0);
});
