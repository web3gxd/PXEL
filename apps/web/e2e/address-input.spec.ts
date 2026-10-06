import { expect, test } from '@playwright/test';
import { createBookFromUi } from './create-book';

test('address input selects cells and ranges, rejects invalid bounds and cancels drafts', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  const address = page.getByRole('textbox', { name: 'Ячейка или диапазон', exact: true });
  await address.click();
  await address.fill('b9');
  await address.press('Enter');
  await expect(address).toHaveValue('B9');
  await expect(page.getByTestId('active-cell')).toHaveText('B9: ');
  await address.fill('$E$5 : $A$3');
  await address.press('Enter');
  await expect(address).toHaveValue('A3:E5');
  await expect(page.getByTestId('active-cell')).toHaveText('A3: ');
  await address.fill('ZZ99999999999999999999');
  await address.press('Enter');
  await expect(address).toHaveAttribute('aria-invalid', 'true');
  await expect(page.getByTestId('active-cell')).toHaveText('A3: ');
  await address.press('Escape');
  await expect(address).toHaveValue('A3:E5');
  await address.fill('Z900');
  await address.press('Enter');
  await expect(page.getByTestId('active-cell')).toHaveText('Z900: ');
  await expect
    .poll(() => page.locator('.grid-scroll').evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  await expect
    .poll(() => page.locator('.grid-scroll').evaluate((element) => element.scrollLeft))
    .toBeGreaterThan(0);
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('active-cell')).toHaveText('Z899: ');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
});
