import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test } from '@playwright/test';

async function fixture(page: import('@playwright/test').Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', 'Город\tСумма\nМосква\t10\nПермь\t20\nМосква\t30');
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Город');
  await expect(await editorButton(page, 'Вид листа')).toBeEnabled();
  await page.keyboard.press('Escape');
  const id = new URL(page.url()).searchParams.get('document')!;
  return { grid, id, state: async () => (await page.request.get(`/api/documents/${id}`)).json() };
}

test('personal filter survives reload without a document commit and navigation skips hidden rows', async ({
  page,
}) => {
  const { grid, id, state } = await fixture(page);
  const before = await state();
  const opener = editorTrigger(page, 'Вид листа');
  await (await editorButton(page, 'Вид листа')).click();
  const dialog = page.getByRole('dialog', { name: 'Вид листа' });
  await dialog.getByLabel('Последняя строка', { exact: true }).fill('4');
  await dialog.getByLabel('Последний столбец', { exact: true }).selectOption('1');
  await dialog.getByLabel('Есть заголовок', { exact: true }).check();
  await dialog.getByRole('button', { name: 'Добавить условие' }).click();
  await dialog.getByLabel('Тип условия 1', { exact: true }).selectOption('text');
  await dialog.getByLabel('Значение условия 1', { exact: true }).fill('Москва');
  await dialog.getByRole('button', { name: 'Сохранить фильтр', exact: true }).click();
  await expect(dialog.getByRole('status').last()).toContainText('Скрыто строк: 1');
  expect((await state()).version).toBe(before.version);
  expect((await state()).state_hash).toBe(before.state_hash);
  const view = await (await page.request.get(`/api/documents/${id}/personal-view`)).json();
  expect(view.revision).toBe(1);
  expect(view.settings.filters[0].conditions[0].predicate.value).toBe('Москва');
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
  await grid.focus();
  await grid.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('A2: Москва');
  await grid.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('A4: Москва');
  await page.reload();
  await expect(page.locator('canvas')).toHaveAttribute('data-hidden-rows', '1');
  await (await editorButton(page, 'Вид листа')).click();
  await expect(dialog.getByLabel('Значение условия 1', { exact: true })).toHaveValue('Москва');
  await dialog.getByRole('button', { name: 'Снять фильтр листа' }).click();
  await expect(dialog.getByRole('status').last()).toContainText('Скрыто строк: 0');
  expect((await state()).version).toBe(before.version);
});

test('a filter hiding every row exposes no hidden active cell and never extends the book', async ({
  page,
}) => {
  const { grid, state } = await fixture(page);
  const before = await state();
  await grid.evaluate((element) => {
    element.scrollTop = 500;
  });
  await (await editorButton(page, 'Вид листа')).click();
  const dialog = page.getByRole('dialog', { name: 'Вид листа' });
  await dialog.getByLabel('Последняя строка', { exact: true }).fill('1000');
  await dialog.getByLabel('Последний столбец', { exact: true }).selectOption('0');
  await dialog.getByRole('button', { name: 'Добавить условие' }).click();
  await dialog.getByLabel('Тип условия 1', { exact: true }).selectOption('text');
  await dialog.getByLabel('Значение условия 1', { exact: true }).fill('Несуществующий город');
  await dialog.getByRole('button', { name: 'Сохранить фильтр', exact: true }).click();
  await expect(dialog.getByRole('status').last()).toContainText('Скрыто строк: 1000');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('active-cell')).toHaveText('Фильтр скрыл все строки.');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeDisabled();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('');
  await grid.focus();
  await grid.press('ArrowDown');
  await grid.press('End');
  await grid.press('Enter');
  await grid.evaluate(async (element) => {
    element.scrollTop = element.scrollHeight;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const after = await state();
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.sheets[0].rows).toHaveLength(1000);
  await (await editorButton(page, 'Вид листа')).click();
  await dialog.getByRole('button', { name: 'Снять фильтр листа' }).click();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Город');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
});

test('shared frozen rows persist and remain hit targets after scrolling', async ({ page }) => {
  const { grid, state } = await fixture(page);
  const before = await state();
  await (await editorButton(page, 'Вид листа')).click();
  const dialog = page.getByRole('dialog', { name: 'Вид листа' });
  await dialog.getByLabel('Верхних строк', { exact: true }).fill('2');
  await dialog.getByRole('button', { name: 'Сохранить закрепление' }).click();
  await expect.poll(async () => (await state()).sheets[0].frozen_rows).toBe(2);
  expect((await state()).version).toBe(String(BigInt(before.version) + 1n));
  expect((await state()).sheets[0].cells).toEqual(before.sheets[0].cells);
  await page.keyboard.press('Escape');
  await expect(page.locator('canvas')).toHaveAttribute('data-frozen-height', '56');
  await grid.evaluate((element) => {
    element.scrollTop = 500;
  });
  await grid.click({ position: { x: 70, y: 42 } });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Город');
  await page.reload();
  await expect(page.locator('canvas')).toHaveAttribute('data-frozen-height', '56');
  await (await editorButton(page, 'Вид листа')).click();
  await expect(dialog.getByLabel('Верхних строк', { exact: true })).toHaveValue('2');
});

for (const width of [390, 1440])
  for (const scheme of ['light', 'dark'] as const)
    test(`view dialog keyboard and layout ${width} ${scheme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: scheme });
      await fixture(page);
      const opener = editorTrigger(page, 'Вид листа');
      await (await editorButton(page, 'Вид листа')).click();
      const dialog = page.getByRole('dialog', { name: 'Вид листа' });
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      await dialog.getByRole('button', { name: 'Закрыть' }).focus();
      await page.keyboard.press('Shift+Tab');
      await expect(dialog.getByRole('button', { name: 'Снять фильтр листа' })).toBeFocused();
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
    });
