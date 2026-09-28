import { expect, test } from '@playwright/test';

test('missing WASM visibly disables preview while server calculation remains available', async ({
  page,
}) => {
  await page.route('**/*.wasm*', (route) => route.abort());
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('preview-status')).toHaveText('Предпросмотр недоступен');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=2+3');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 5');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
});

test('WASM previews an unsaved draft and cancellation leaves the durable document unchanged', async ({
  page,
}) => {
  const wasmResponses: number[] = [];
  page.on('response', (response) => {
    if (response.headers()['content-type']?.startsWith('application/wasm'))
      wasmResponses.push(response.status());
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  const before = await page.evaluate(async (documentId) => {
    const response = await fetch(`/api/documents/${documentId}`);
    if (!response.ok) throw new Error('Document read failed');
    return response.json();
  }, id);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=SUM(');
  // Wait for the invalid draft to reach the Worker before replacing it.
  await expect(page.getByTestId('preview-status')).toContainText('формул');
  await bar.fill('=2+3');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 5');
  await expect(page.getByTestId('calculation-preview')).toContainText('не подтверждено');
  await bar.press('Escape');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  const after = await page.evaluate(async (documentId) => {
    const response = await fetch(`/api/documents/${documentId}`);
    if (!response.ok) throw new Error('Document read failed');
    return response.json();
  }, id);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.sheets[0].cells).toEqual([]);
  expect(wasmResponses).toEqual([200]);
});

test('formula results recalculate while editors preserve source across reload', async ({
  page,
}) => {
  const calculationHttpRequests: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.endsWith('/calculation'))
      calculationHttpRequests.push(request.method());
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const active = page.getByTestId('active-cell');
  await bar.fill('2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText('B1: ');
  await bar.fill('=A1+3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText('B1: 5');
  await expect(bar).toHaveValue('=A1+3');
  await page.keyboard.press('ArrowLeft');
  await bar.fill('8');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(bar).toHaveValue('8');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText('B1: 11');
  await expect(bar).toHaveValue('=A1+3');
  await bar.fill('=B1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText('B1: #CYCLE!');
  expect(calculationHttpRequests).toEqual([]);
});

test('create, edit sparse cells, keyboard navigation, reload durable values', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page).toHaveURL(/document=[0-9a-f-]{36}/);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await page.keyboard.type('42');
  await expect(page.getByRole('textbox', { name: 'Редактор ячейки' })).toHaveValue('42');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('active-cell')).toHaveText('A2: ');
  await page.keyboard.press('Tab');
  await expect(page.getByTestId('active-cell')).toHaveText('B2: ');
  await page.keyboard.press('F2');
  await page.getByRole('textbox', { name: 'Редактор ячейки' }).fill('Проверка сохранения');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('42');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue(
    'Проверка сохранения',
  );
  await page.keyboard.press('F2');
  await page.getByRole('textbox', { name: 'Редактор ячейки' }).fill('Отмена');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue(
    'Проверка сохранения',
  );
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('TRUE');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('42');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(bar).toHaveValue('TRUE');
  expect(errors).toEqual([]);
});

test('two browser tabs see each other’s committed cell after a head hint', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    let fullDocumentReads = 0;
    second.on('request', (request) => {
      if (
        request.method() === 'GET' &&
        /\/api\/documents\/[0-9a-f-]{36}$/.test(new URL(request.url()).pathname)
      )
        fullDocumentReads++;
    });
    const firstBar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await firstBar.fill('17');
    await firstBar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await expect(second.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('17');
    expect(fullDocumentReads).toBe(0);
    const secondBar = second.getByRole('textbox', { name: 'Значение ячейки' });
    await secondBar.fill('23');
    await secondBar.press('Enter');
    await expect(second.getByTestId('save-status')).toHaveText('Сохранено');
    await page.getByRole('region', { name: 'Область таблицы' }).focus();
    await page.keyboard.press('ArrowUp');
    await expect(firstBar).toHaveValue('23');
  } finally {
    await second.close();
  }
});
