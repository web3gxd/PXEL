import { expect, type Page } from '@playwright/test';

/** Wait for the new editor and confirmed snapshot, rather than the old blank cell. */
export async function createBookFromUi(page: Page) {
  const previous = new URL(page.url()).searchParams.get('document');
  if (previous) await page.getByRole('button', { name: 'Файл', exact: true }).click();
  await Promise.all([
    page.waitForURL(
      (url) => url.searchParams.has('document') && url.searchParams.get('document') !== previous,
      { waitUntil: 'load' },
    ),
    page.getByRole('button', { name: 'Создать таблицу', exact: true }).click(),
  ]);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
