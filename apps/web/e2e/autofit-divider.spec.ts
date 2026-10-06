import { expect, test } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { GRID } from '../src/grid/viewport';

test('double-clicking a divider fits the longest text durably without entering an editor', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('Иванов Иван Иванович — демонстрационный пример');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => await (await page.request.get(`/api/documents/${id}`)).json();
  const before = await state();
  await grid.dblclick({
    position: { x: GRID.rowHeaderWidth + GRID.columnWidth, y: GRID.columnHeaderHeight / 2 },
  });
  await expect.poll(async () => (await state()).version).toBe(String(Number(before.version) + 1));
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const after = await state();
  expect(after.sheets[0].column_widths[0].size_css_px).toBeGreaterThan(GRID.columnWidth);
  await expect(page.locator('.cell-editor')).toHaveCount(0);
  await expect(page.locator('.editor-brand')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Протянуть заполнение диапазона' })).toBeVisible();
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  expect((await state()).sheets[0].column_widths).toEqual(after.sheets[0].column_widths);
});
