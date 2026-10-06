import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { GRID } from '../src/grid/viewport';
import type { Projection } from '../src/document/store';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
async function state(page: Page): Promise<Projection> {
  return (
    await page.request.get(`/api/documents/${new URL(page.url()).searchParams.get('document')}`)
  ).json();
}
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
}
async function fixture(page: Page, text = 'Число\tГруппа\n3\tkeep\n1\thide\n2\tkeep') {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const previousId = new URL(page.url()).searchParams.get('document');
  await createBookFromUi(page);
  await expect.poll(() => new URL(page.url()).searchParams.get('document')).not.toBe(previousId);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  await expect(await editorButton(page, 'Сортировка')).toBeEnabled();
  await page.keyboard.press('Escape');
  await grid(page).evaluate((element, text) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', text);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, text);
  await expect(page.getByTestId('active-cell')).toHaveText(`A1: ${text.split('\t')[0]}`);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(await editorButton(page, 'Сортировка')).toBeEnabled();
  await page.keyboard.press('Escape');
}
const cell = (book: Projection, row: number, column: number) =>
  book.sheets[0]!.cells.find(
    (cell) =>
      cell.row_id === book.sheets[0]!.rows[row] &&
      cell.column_id === book.sheets[0]!.columns[column],
  );

test('confirmed multikey sort moves formulas and styles in one commit and survives Undo Redo reload', async ({
  page,
}) => {
  await fixture(page, '2\tРезультат\tГруппа\n3\t\tb\n1\t\ta\n2\t\ta');
  for (const row of [1, 2, 3]) {
    await select(page, row, 1);
    await bar(page).fill(`=A${row + 1}*$A$1`);
    await bar(page).press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  }
  await select(page, 0, 3);
  await bar(page).fill('=B2');
  await bar(page).press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await select(page, 1, 0);
  const styleBefore = await state(page);
  await page.getByRole('button', { name: 'Полужирный', exact: true }).click();
  await expect
    .poll(async () => (await state(page)).version)
    .toBe(String(BigInt(styleBefore.version) + 1n));
  await select(page, 0, 0);
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  const before = await state(page);
  await (await editorButton(page, 'Сортировка')).click();
  const dialog = page.getByRole('dialog', { name: 'Сортировка диапазона' });
  await expect(dialog.getByRole('button', { name: 'Сортировать', exact: true })).toBeDisabled();
  await dialog.getByLabel('Последняя строка', { exact: true }).fill('4');
  await dialog.getByLabel('Последний столбец', { exact: true }).selectOption('2');
  await dialog.getByLabel('Есть заголовок', { exact: true }).check();
  await dialog.getByLabel('Строка заголовка', { exact: true }).fill('1');
  await dialog.getByLabel('Столбец сортировки 1', { exact: true }).selectOption('2');
  await dialog.getByRole('button', { name: 'Добавить ключ', exact: true }).click();
  await dialog.getByLabel('Столбец сортировки 2', { exact: true }).selectOption('1');
  await dialog.getByLabel('Направление сортировки 2', { exact: true }).selectOption('desc');
  await dialog.getByRole('checkbox', { name: /^Подтверждаю границы/ }).check();
  await dialog.getByRole('button', { name: 'Сортировать', exact: true }).click();
  await expect
    .poll(async () => (await state(page)).version)
    .toBe(String(BigInt(before.version) + 1n));
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const sorted = await state(page);
  expect([1, 2, 3].map((row) => cell(sorted, row, 0)?.value)).toEqual([
    { type: 'number', value: 2 },
    { type: 'number', value: 1 },
    { type: 'number', value: 3 },
  ]);
  expect(cell(sorted, 0, 3)).toEqual(cell(before, 0, 3));
  expect(cell(sorted, 0, 0)).toEqual(cell(before, 0, 0));
  expect(sorted.calculation_context).toEqual(before.calculation_context);
  await select(page, 1, 1);
  await expect(page.getByTestId('active-cell')).toHaveText('B2: 4');
  await expect(bar(page)).toHaveValue('=(A2*$A$1)');
  await select(page, 0, 3);
  await expect(page.getByTestId('active-cell')).toHaveText('D1: 4');
  await page.getByRole('button', { name: 'Отменить последнюю правку', exact: true }).click();
  await expect
    .poll(async () => (await state(page)).sheets[0]!.cells)
    .toEqual(before.sheets[0]!.cells);
  expect((await state(page)).sheets[0]!.styles).toEqual(before.sheets[0]!.styles);
  await page.getByRole('button', { name: 'Повторить отменённую правку', exact: true }).click();
  await expect
    .poll(async () => (await state(page)).sheets[0]!.cells)
    .toEqual(sorted.sheets[0]!.cells);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  expect((await state(page)).sheets[0]!.styles).toEqual(sorted.sheets[0]!.styles);
});

test('range suggestion requires confirmation and sorting includes personally hidden rows', async ({
  page,
}) => {
  await fixture(page);
  await (await editorButton(page, 'Вид листа')).click();
  const view = page.getByRole('dialog', { name: 'Вид листа' });
  await view.getByLabel('Последняя строка', { exact: true }).fill('4');
  await view.getByLabel('Последний столбец', { exact: true }).selectOption('1');
  await view.getByLabel('Есть заголовок', { exact: true }).check();
  await view.getByRole('button', { name: 'Добавить условие', exact: true }).click();
  await view
    .getByLabel('Столбец условия 1', { exact: true })
    .selectOption((await state(page)).sheets[0]!.columns[1]!);
  await view.getByLabel('Тип условия 1', { exact: true }).selectOption('text');
  await view.getByLabel('Значение условия 1', { exact: true }).fill('keep');
  await view.getByRole('button', { name: 'Сохранить фильтр', exact: true }).click();
  await expect(view.getByRole('status').last()).toContainText('Скрыто строк: 1');
  await page.keyboard.press('Escape');
  const before = await state(page);
  await (await editorButton(page, 'Сортировка')).click();
  const dialog = page.getByRole('dialog', { name: 'Сортировка диапазона' });
  await expect(dialog).toContainText('A1:B4');
  await dialog
    .getByRole('button', { name: 'Использовать предложенный диапазон', exact: true })
    .click();
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Число');
  await expect(dialog.getByRole('button', { name: 'Сортировать', exact: true })).toBeDisabled();
  await dialog.getByRole('checkbox', { name: /^Подтверждаю границы/ }).check();
  await dialog.getByRole('button', { name: 'Сортировать', exact: true }).click();
  await expect
    .poll(async () => (await state(page)).version)
    .toBe(String(BigInt(before.version) + 1n));
  const sorted = await state(page);
  expect([1, 2, 3].map((row) => cell(sorted, row, 0)?.value)).toEqual([
    { type: 'number', value: 1 },
    { type: 'number', value: 2 },
    { type: 'number', value: 3 },
  ]);
  await grid(page).focus();
  await grid(page).press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('A3: 2');
});

for (const width of [390, 1440])
  for (const scheme of ['light', 'dark'] as const)
    test(`sort dialog keyboard and layout ${width} ${scheme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: scheme });
      await fixture(page);
      const opener = editorTrigger(page, 'Сортировка');
      await (await editorButton(page, 'Сортировка')).click();
      const dialog = page.getByRole('dialog', { name: 'Сортировка диапазона' });
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      await dialog.getByRole('checkbox', { name: /^Подтверждаю границы/ }).check();
      await dialog.getByRole('button', { name: 'Закрыть окно', exact: true }).focus();
      await page.keyboard.press('Shift+Tab');
      await expect(dialog.getByRole('button', { name: 'Сортировать', exact: true })).toBeFocused();
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
