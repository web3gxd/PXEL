import { expect, test } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';

for (const width of [390, 1440]) {
  test(`feature modal surfaces and keyboard ${width}`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await createBookFromUi(page);
    for (const action of [
      'Вид листа',
      'Сортировка',
      'Выдача и аудит',
      'Проверка констант',
      'Персональные данные',
      'Профайлер',
      'Журнал и версии',
      'Файл PXEL',
      'Размеры',
    ]) {
      await (await editorButton(page, action)).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog).toBeVisible();
      await expect(dialog.locator('.modal-title-icon')).toBeVisible();
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      const close = dialog.getByRole('button', { name: 'Закрыть окно', exact: true });
      await close.focus();
      await page.keyboard.press('Shift+Tab');
      expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(
        true,
      );
      await page.screenshot({
        path: `../../.local/modal-design-${action}-${width}-${info.project.name}.png`,
      });
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
    }
    await page.getByRole('button', { name: 'Поделиться', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Поделиться таблицей' })).toBeVisible();
    await page.keyboard.press('Escape');
    await (await editorButton(page, 'Горячие клавиши')).click();
    await expect(page.getByRole('dialog', { name: 'Горячие клавиши' })).toBeVisible();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Другие параметры оформления' }).click();
    await expect(page.getByRole('dialog', { name: 'Оформление ячеек' })).toBeVisible();
    await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      width,
    );
    expect(errors).toEqual([]);
  });
}
