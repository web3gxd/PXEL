import { expect, test } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';

for (const width of [390, 681, 1440]) {
  test(`compact PXEL chrome and menus ${width}`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await createBookFromUi(page);
    const nav = page.getByRole('navigation', { name: 'Меню таблицы' });
    await expect(nav.getByRole('button')).toHaveCount(7);
    await expect(page.locator('.editor-menu-popup')).toHaveCount(0);
    const menu = page.getByRole('button', { name: 'Данные', exact: true });
    await menu.focus();
    await menu.press('ArrowDown');
    await expect(page.getByRole('button', { name: 'Сортировка', exact: true })).toBeFocused();
    await page.keyboard.press('End');
    await expect(page.getByRole('button', { name: 'Выдача и аудит', exact: true })).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(menu).toBeFocused();
    await expect(page.locator('.editor-menu-popup')).toHaveCount(0);

    await (await editorButton(page, 'Персональные данные')).click();
    const dialog = page.getByRole('dialog', { name: 'Персональные данные' });
    await expect(dialog).toBeVisible();
    expect(await dialog.evaluate((el) => getComputedStyle(el).borderRadius)).toBe('12px');
    await page.screenshot({ path: `../../.local/editor-dialog-${width}-${info.project.name}.png` });
    await page.keyboard.press('Escape');
    await expect(menu).toBeFocused();
    await (await editorButton(page, 'Строка ↑')).click();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await (await editorButton(page, 'Файл PXEL')).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.keyboard.press('Escape');

    const connection = page.getByRole('button', { name: 'Соединение: Подключено', exact: true });
    await connection.focus();
    const dashboard = page.getByRole('region', { name: 'Состояние соединения' });
    await expect(dashboard).toBeVisible();
    await expect(dashboard).toContainText('Отклик WebSocket');
    await expect(dashboard).toContainText(/\d+ мс/);
    await page.screenshot({
      path: `../../.local/editor-connection-${width}-${info.project.name}.png`,
    });
    await connection.press('Escape');
    await expect(dashboard).toHaveCount(0);
    await connection.blur();

    const toolbar = page.getByRole('toolbar', { name: 'Форматирование' });
    await expect(
      toolbar.getByRole('button', { name: 'Объединить ячейки', exact: true }),
    ).toBeVisible();
    expect(await toolbar.evaluate((el) => getComputedStyle(el).scrollbarWidth)).toBe('none');
    if (width < 1440) {
      await toolbar.hover();
      await page.mouse.wheel(200, 0);
      await expect.poll(() => toolbar.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
    }
    await toolbar.evaluate((el) => {
      el.scrollLeft = 0;
    });
    await page.mouse.move(width - 2, 600);
    await page
      .getByRole('region', { name: 'Область таблицы' })
      .click({ position: { x: 100, y: 42 } });
    await page.keyboard.press('Shift+ArrowRight');
    await toolbar.getByRole('button', { name: 'Объединить ячейки', exact: true }).click();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await (await editorButton(page, 'Размеры')).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Формат', exact: true }).click();
    await page
      .locator('.editor-menu-popup')
      .getByRole('button', { name: 'Разъединить ячейки', exact: true })
      .click();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await toolbar.evaluate((el) => {
      el.scrollLeft = 0;
    });
    await page.getByRole('button', { name: 'Данные', exact: true }).click();
    await page.screenshot({ path: `../../.local/editor-menu-${width}-${info.project.name}.png` });
    await page.keyboard.press('Escape');
    await page.screenshot({ path: `../../.local/editor-chrome-${width}-${info.project.name}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
    expect(errors).toEqual([]);
  });
}
