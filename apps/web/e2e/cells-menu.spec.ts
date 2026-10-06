import { test, expect } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';

for (const width of [390, 681, 1440]) {
  test(`nested cell operations and shared modal ${width}`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await createBookFromUi(page);
    const trigger = page.getByRole('button', { name: 'Ячейки', exact: true });
    await trigger.focus();
    await trigger.press('ArrowDown');
    const add = page.getByRole('button', { name: 'Добавить', exact: true });
    await expect(add).toBeFocused();
    await add.press('ArrowRight');
    const insert = page.getByRole('button', { name: 'Вставить строки листа', exact: true });
    await expect(insert).toBeFocused();
    await expect(page.getByRole('button', { name: /Вставить ячейки…/ })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(add).toBeFocused();
    await expect(add).toHaveAttribute('aria-expanded', 'false');
    await page.keyboard.press('Escape');
    await expect(trigger).toBeFocused();
    await (await editorButton(page, 'Строка ↑')).click();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await trigger.click();
    const format = page.getByRole('button', { name: 'Форматировать', exact: true });
    await format.click();
    const visibility = page.getByRole('button', { name: 'Видимость', exact: true });
    await visibility.click();
    await expect(page.getByRole('button', { name: /Скрыть строки/ })).toBeDisabled();
    await page.screenshot({ path: `../../.local/cells-menu-${width}-${info.project.name}.png` });
    await page.keyboard.press('Escape');
    await expect(visibility).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(format).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(trigger).toBeFocused();
    await (await editorButton(page, 'Размеры')).click();
    const modal = page.getByRole('dialog', { name: 'Размеры ячеек' });
    await expect(modal).toBeVisible();
    await expect(modal.getByRole('spinbutton', { name: 'Высота строки в пикселях' })).toBeFocused();
    await expect(modal.locator('.modal-title-icon svg')).toBeVisible();
    await page.screenshot({ path: `../../.local/cells-modal-${width}-${info.project.name}.png` });
    await page.keyboard.press('Escape');
    await expect(trigger).toBeFocused();
    await trigger.click();
    await page.getByRole('button', { name: 'Форматировать', exact: true }).click();
    await page.getByRole('button', { name: 'Ширина столбца…', exact: true }).click();
    await expect(page.getByRole('spinbutton', { name: 'Ширина столбца в пикселях' })).toBeFocused();
    await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      width,
    );
    expect(errors).toEqual([]);
  });
}
