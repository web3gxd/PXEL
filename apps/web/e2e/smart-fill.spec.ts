import { expect, test, type Page } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { copySelection } from './clipboard';

const errors = new WeakMap<Page, string[]>();
test.beforeEach(({ page }) => {
  const collected: string[] = [];
  errors.set(page, collected);
  page.on('pageerror', (error) => collected.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') collected.push(message.text());
  });
});
test.afterEach(({ page }) => expect(errors.get(page)).toEqual([]));
const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const handle = (page: Page) => page.getByRole('button', { name: 'Протянуть заполнение диапазона' });
async function createSheet(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
}
async function state(page: Page, version: string) {
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe(version);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  return (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0];
}
async function enter(page: Page, text: string, version: string) {
  await bar(page).fill(text);
  await bar(page).press('Enter');
  await state(page, version);
}
async function drag(page: Page, dx: number, dy: number) {
  const box = (await handle(page).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2 + dy, { steps: 8 });
  await expect(page.locator('.autofill-preview')).toBeVisible();
  await page.mouse.up();
}

test('dragging a formula fills relative references in one version with durable Undo and Redo', async ({
  page,
}) => {
  await createSheet(page);
  await enter(page, '=B1+$B$1', '2');
  await drag(page, -10, 65);
  const sheet = await state(page, '3');
  expect(sheet.cells).toHaveLength(4);
  for (let row = 0; row < 4; row++)
    expect(
      sheet.cells.find((c: { row_id: string }) => c.row_id === sheet.rows[row]).value.value
        .current_text,
    ).toContain(`B${row + 1}+$B$1`);
  await page.reload();
  await expect(page.getByTestId('active-cell')).toContainText('A1:');
  await page.getByRole('button', { name: 'Отменить последнюю правку', exact: true }).click();
  expect((await state(page, '4')).cells).toHaveLength(1);
  await page.getByRole('button', { name: 'Повторить отменённую правку', exact: true }).click();
  expect((await state(page, '5')).cells).toHaveLength(4);
});

test('numeric seed extrapolates downward and keyboard fill can be cancelled', async ({ page }) => {
  await createSheet(page);
  await enter(page, '1', '2');
  await grid(page).focus();
  await page.keyboard.press('ArrowDown');
  await enter(page, '3', '3');
  await grid(page).focus();
  await page.keyboard.press('Shift+ArrowUp');
  await drag(page, -10, 90);
  const sheet = await state(page, '4');
  expect(
    sheet.cells
      .map((c: { value: { value: number } }) => c.value.value)
      .sort((a: number, b: number) => a - b),
  ).toEqual([1, 3, 5, 7, 9, 11]);
  await handle(page).focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.locator('.autofill-preview')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('.autofill-preview')).toHaveCount(0);
  expect((await state(page, '4')).cells).toHaveLength(6);
});

test('assistant detects repetition, fills only blank destinations, and can be completely disabled across reload', async ({
  page,
}) => {
  await createSheet(page);
  for (let i = 0; i < 3; i++) {
    await enter(page, 'same', String(i + 2));
    if (i < 2) {
      await grid(page).focus();
      await page.keyboard.press('ArrowDown');
    }
  }
  await expect(page.getByText('Повторяющийся ввод', { exact: true })).toBeVisible();
  await page.getByRole('spinbutton', { name: 'Ячеек' }).fill('3');
  await page.getByRole('button', { name: 'Заполнить 3', exact: true }).click();
  expect((await state(page, '5')).cells).toHaveLength(6);
  await page.getByRole('checkbox', { name: 'Подсказки и анализ действий' }).uncheck();
  await expect(page.getByText('Помощник выключен.', { exact: false })).toBeVisible();
  await page.reload();
  await expect(page.getByTestId('active-cell')).toHaveText('A1: same');
  await page.getByRole('button', { name: 'Помощник · выключен', exact: false }).click();
  await expect(
    page.getByRole('checkbox', { name: 'Подсказки и анализ действий' }),
  ).not.toBeChecked();
  await page.getByRole('button', { name: 'Закрыть помощника' }).click();
  await drag(page, -10, 43);
  expect((await state(page, '6')).cells).toHaveLength(6);
  await handle(page).focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  expect((await state(page, '7')).cells).toHaveLength(9);
});

test('assistant recognizes horizontal numeric sequences', async ({ page }) => {
  await createSheet(page);
  for (let i = 0; i < 3; i++) {
    await enter(page, String(i * 2 + 1), String(i + 2));
    if (i < 2) {
      await grid(page).focus();
      await page.keyboard.press('ArrowRight');
    }
  }
  await expect(
    page.getByText('Обнаружена числовая последовательность', { exact: true }),
  ).toBeVisible();
  await page.getByRole('spinbutton', { name: 'Ячеек' }).fill('2');
  await page.getByRole('button', { name: 'Заполнить 2', exact: true }).click();
  const sheet = await state(page, '5');
  expect(
    sheet.cells
      .map((c: { value: { value: number } }) => c.value.value)
      .sort((a: number, b: number) => a - b),
  ).toEqual([1, 3, 5, 7, 9]);
  await expect(page.getByRole('checkbox', { name: 'Подсказки и анализ действий' })).toBeVisible();
  await page.screenshot({ path: '../../.local/smart-fill-assistant.png' });
});

test('numeric autofill extrapolates upward and repeated blocks fill left', async ({ page }) => {
  await createSheet(page);
  await grid(page).focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await enter(page, '5', '2');
  await grid(page).focus();
  await page.keyboard.press('ArrowDown');
  await enter(page, '7', '3');
  await grid(page).focus();
  await page.keyboard.press('Shift+ArrowUp');
  await handle(page).focus();
  for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Enter');
  const sheet = await state(page, '4');
  expect(
    sheet.cells
      .map((c: { value: { value: number } }) => c.value.value)
      .sort((a: number, b: number) => a - b),
  ).toEqual([1, 3, 5, 7]);
  await page.reload();
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 1');
  await grid(page).focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await enter(page, 'seed', '5');
  await drag(page, -225, -10);
  const after = await state(page, '6');
  for (let column = 0; column < 3; column++)
    expect(
      after.cells.find(
        (c: { row_id: string; column_id: string }) =>
          c.row_id === after.rows[0] && c.column_id === after.columns[column],
      ).value,
    ).toEqual({ type: 'text', value: 'seed' });
});

test('a concurrent edit cancels a captured fill gesture without writing', async ({ page }) => {
  await createSheet(page);
  await enter(page, '7', '2');
  const box = (await handle(page).boundingBox())!;
  await page.mouse.move(box.x + 8, box.y + 8);
  await page.mouse.down();
  await page.mouse.move(box.x, box.y + 90);
  const other = await page.context().newPage();
  await other.goto(page.url());
  await expect(other.getByTestId('active-cell')).toHaveText('A1: 7');
  await grid(other).focus();
  await other.keyboard.press('ArrowRight');
  await enter(other, '9', '3');
  await state(page, '3');
  await page.mouse.up();
  await expect(page.locator('.autofill-preview')).toHaveCount(0);
  expect((await state(page, '3')).cells).toHaveLength(2);
  await other.close();
});

test('repeated formatting and authorized copying show their own shortcut advice', async ({
  page,
}) => {
  await createSheet(page);
  await enter(page, '7', '2');
  for (let i = 0; i < 3; i++) {
    await page.getByRole('button', { name: 'Полужирный', exact: true }).click();
    await state(page, String(i + 3));
  }
  await expect(page.getByText('Оформите диапазон сразу', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Закрыть помощника' }).click();
  await grid(page).focus();
  for (let i = 0; i < 3; i++) await copySelection(page);
  await expect(page.getByText('Копирование без лишних кликов', { exact: true })).toBeVisible();
  expect((await state(page, '5')).cells).toHaveLength(1);
  await page.getByRole('button', { name: 'Все горячие клавиши', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
});
