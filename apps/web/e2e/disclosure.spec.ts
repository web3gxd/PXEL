import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test } from '@playwright/test';

test('copy and cut are audited and a denied cut leaves contents and clipboard intact', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('Disclosure fixture');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document')!;
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.press('Control+C');
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe('Disclosure fixture');
  const opener = editorTrigger(page, 'Выдача и аудит');
  await (await editorButton(page, 'Выдача и аудит')).click();
  const dialog = page.getByRole('dialog', { name: 'Выдача и аудит' });
  await expect(dialog.getByRole('checkbox', { name: 'Копирование и вырезание' })).toBeChecked();
  await dialog.getByRole('checkbox', { name: 'Копирование и вырезание' }).uncheck();
  await dialog.getByRole('button', { name: 'Сохранить политику' }).click();
  await expect(dialog.getByText('Политика сохранена.', { exact: false })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
  await page.evaluate(() => navigator.clipboard.writeText('Keep clipboard'));
  await grid.focus();
  const denied = page.waitForResponse(
    (response) =>
      response.url().endsWith('/clipboard/authorize') && response.request().method() === 'POST',
  );
  await grid.press('Control+X');
  const refused = await denied;
  expect(refused.status()).toBe(403);
  expect(refused.request().postDataJSON().stamp.book_revision).toBe(1);
  await expect(page.getByRole('alert')).toContainText('Копирование запрещено');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await expect(page.getByTestId('cut-status')).toHaveCount(0);
  const snapshot = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(snapshot.version).toBe('2');
  expect(snapshot.sheets[0].cells[0].value.value).toBe('Disclosure fixture');
  expect(snapshot.capabilities.can_copy).toBe(false);
  await (await editorButton(page, 'Выдача и аудит')).click();
  await dialog.getByRole('button', { name: 'Показать журнал' }).click();
  await expect(
    dialog.getByText('Вырезание · Копирование запрещено', { exact: true }),
  ).toBeVisible();
  await expect(dialog.getByText('Копирование · Разрешено', { exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await (await editorButton(page, 'Выдача и аудит')).click();
  await expect(dialog.getByRole('checkbox', { name: 'Копирование и вырезание' })).not.toBeChecked();
});

test('authorized clipboard retains formula references and internal range metadata', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=B1+2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.press('Control+C');
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe('=(B1+2)');
  const html = await page.evaluate(async () => {
    const item = (await navigator.clipboard.read())[0]!;
    return (await item.getType('text/html')).text();
  });
  expect(html).toContain('data-pxel-clipboard');
  await grid.press('ArrowDown');
  await grid.press('Control+V');
  await expect(bar).toHaveValue('=(B2+2)');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
});

for (const width of [390, 1440])
  for (const scheme of ['light', 'dark'] as const)
    test(`disclosure dialog keyboard and layout ${width} ${scheme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto('/');
      await createBookFromUi(page);
      await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
      await createBookFromUi(page);
      const opener = editorTrigger(page, 'Выдача и аудит');
      await (await editorButton(page, 'Выдача и аудит')).click();
      const dialog = page.getByRole('dialog', { name: 'Выдача и аудит' });
      await expect(dialog.getByRole('checkbox', { name: 'Копирование и вырезание' })).toBeVisible();
      await expect
        .poll(() => dialog.evaluate((element) => element.contains(document.activeElement)))
        .toBe(true);
      await page.keyboard.press('Shift+Tab');
      await expect
        .poll(() => dialog.evaluate((element) => element.contains(document.activeElement)))
        .toBe(true);
      expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
        true,
      );
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
      await expect(opener).toBeFocused();
    });
