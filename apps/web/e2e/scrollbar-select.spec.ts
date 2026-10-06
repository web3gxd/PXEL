import { expect, test, type Page } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';
import { selectValue } from './select';

async function fixture(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', () => errors.push('pageerror'));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push('console error');
  });
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.locator('canvas')).toHaveAttribute('data-ready', 'true');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено', { timeout: 15_000 });
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  return { errors, state };
}

test('explicit scrollbar mode persists, keeps offsets and never commits document content', async ({
  page,
}) => {
  const { errors, state } = await fixture(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData(
      'text/plain',
      Array.from({ length: 60 }, (_, i) => `row ${i}`).join('\n'),
    );
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: row 0');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const before = await state();
  const row = page.getByTestId('smart-scrollbar-row');
  await expect(row).toHaveAttribute('data-scroll-mode', 'adaptive');
  const thumb = page.getByTestId('smart-scrollbar-thumb-row');
  expect((await thumb.boundingBox())!.width).toBe(8);
  expect((await page.getByTestId('smart-scrollbar-thumb-column').boundingBox())!.height).toBe(8);
  expect(await thumb.evaluate((e) => getComputedStyle(e).borderRadius)).toBe('3px');
  await grid.hover();
  await page.mouse.wheel(0, 280);
  await expect.poll(() => grid.evaluate((e) => e.scrollTop)).toBeGreaterThan(0);
  const offset = await grid.evaluate((e) => e.scrollTop);
  await (await editorButton(page, 'Настройки')).click();
  const settings = page.getByRole('dialog', { name: 'Настройки', exact: true });
  await selectValue(settings.getByRole('combobox', { name: 'Режим скроллбара' }), 'ordinary');
  await expect(row).toHaveAttribute('data-scrollbar-mode', 'ordinary');
  await expect(row).toHaveAttribute('data-scroll-mode', 'linear');
  await expect(page.getByTestId('smart-scrollbar-column')).toHaveAttribute(
    'data-scrollbar-mode',
    'ordinary',
  );
  expect(await grid.evaluate((e) => e.scrollTop)).toBe(offset);
  expect((await state()).version).toBe(before.version);
  await page.keyboard.press('Escape');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(row).toHaveAttribute('data-scrollbar-mode', 'ordinary');
  await expect(row).toHaveAttribute('data-scroll-mode', 'linear');
  const box = (await row.boundingBox())!;
  await row.click({ position: { x: box.width / 2, y: box.height / 2 } });
  const max = Number(await row.getAttribute('data-scroll-limit'));
  await expect.poll(() => grid.evaluate((e) => e.scrollTop)).toBeGreaterThan(max * 0.4);
  expect(await grid.evaluate((e) => e.scrollTop)).toBeLessThan(max * 0.6);
  const farOffset = await grid.evaluate((e) => e.scrollTop);
  await (await editorButton(page, 'Настройки')).click();
  await selectValue(settings.getByRole('combobox', { name: 'Режим скроллбара' }), 'smart');
  await expect(row).toHaveAttribute('data-scrollbar-mode', 'smart');
  await expect(row).toHaveAttribute('data-scroll-mode', 'adaptive');
  expect(await grid.evaluate((e) => e.scrollTop)).toBe(farOffset);
  expect((await state()).version).toBe(before.version);
  expect(errors).toEqual([]);
});

test('custom formatting choices support keyboard, typeahead, cancel and one durable write', async ({
  page,
}) => {
  const { errors, state } = await fixture(page);
  await expect(page.locator('select')).toHaveCount(0);
  const font = page.getByRole('combobox', { name: 'Шрифт', exact: true });
  const before = await state();
  await font.focus();
  await font.press('Enter');
  const list = page.getByRole('listbox', { name: 'Шрифт', exact: true });
  await expect(list).toBeVisible();
  await font.press('ArrowDown');
  expect((await state()).version).toBe(before.version);
  await font.press('Escape');
  await expect(list).toBeHidden();
  await expect(font).toContainText('Arial');
  await expect(font).toBeFocused();
  await font.press('c');
  await font.press('a');
  await font.press('Enter');
  await expect(font).toContainText('Calibri');
  await expect.poll(async () => Number((await state()).version)).toBe(Number(before.version) + 1);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await font.click();
  await page.getByRole('textbox', { name: 'Значение ячейки' }).click();
  await expect(list).toBeHidden();
  await page.reload();
  await expect(font).toContainText('Calibri');
  expect(errors).toEqual([]);
});

for (const width of [390, 768, 1440]) {
  test(`custom popup stays inside dialog and viewport at ${width}`, async ({ page }, info) => {
    const { errors } = await fixture(page);
    await page.setViewportSize({ width, height: 900 });
    await page.getByRole('button', { name: 'Другие параметры оформления' }).click();
    const dialog = page.getByRole('dialog', { name: 'Оформление ячеек' });
    const underline = dialog.getByRole('combobox', { name: 'Подчёркивание', exact: true });
    await underline.click();
    const list = dialog.getByRole('listbox', { name: 'Подчёркивание', exact: true });
    await expect(list).toBeVisible();
    const box = (await list.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(900);
    await page.screenshot({
      path: `../../.local/custom-dropdown-${width}-${info.project.name}.png`,
    });
    await underline.press('Escape');
    await expect(list).toBeHidden();
    await expect(dialog).toBeVisible();
    await underline.press('Tab');
    expect(await dialog.evaluate((e) => e.contains(document.activeElement))).toBe(true);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    expect(errors).toEqual([]);
  });
}
