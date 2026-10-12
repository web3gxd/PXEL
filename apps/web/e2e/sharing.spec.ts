import { editorButton } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test } from '@playwright/test';

test('comment share unlocks without member login and revocation closes an idle guest', async ({
  page,
  browser,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);

  await page.getByRole('button', { name: 'Поделиться' }).click();
  const dialog = page.getByRole('dialog', { name: 'Поделиться таблицей' });
  await dialog.getByRole('radio', { name: 'Комментирование' }).check();
  await dialog
    .getByRole('group', { name: 'PIN из шести цифр' })
    .locator('input')
    .first()
    .fill('482615');
  const createResponse = page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname.endsWith('/shares'),
  );
  await dialog.getByRole('button', { name: 'Создать ссылку' }).click();
  const created = await createResponse;
  if (!created.ok()) throw new Error(`share creation failed: HTTP ${created.status()}`);
  const responseBody = (await created.json()) as {
    share: { created_at: number; expires_at: number | null };
  };
  expect(responseBody.share.expires_at).toBeGreaterThan(responseBody.share.created_at);
  const serverExpiry = new Intl.DateTimeFormat('ru', {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(responseBody.share.expires_at! * 1000));
  await expect(
    dialog.getByRole('region', { name: 'Созданная ссылка' }).locator('p').first(),
  ).toContainText(serverExpiry);
  await expect(dialog.getByRole('heading', { name: 'Ссылка создана' })).toBeVisible();

  const shareUrl = await dialog.getByRole('textbox', { name: 'Ссылка' }).inputValue();
  const pin = await dialog.getByRole('textbox', { name: 'PIN' }).inputValue();
  const parsed = new URL(shareUrl);
  expect(parsed.pathname).toMatch(/^\/share\/[0-9a-f-]+$/);
  expect(parsed.search).toBe('');
  expect(parsed.hash).toMatch(/^#[0-9a-f]{64}$/);
  expect(pin).toBe('482615');

  const guestContext = await browser.newContext();
  try {
    const guest = await guestContext.newPage();
    await guest.goto(shareUrl);
    await expect(guest.getByRole('heading', { name: 'Открыть таблицу' })).toBeVisible();
    await expect(guest.getByRole('region', { name: 'Область таблицы' })).toHaveCount(0);
    await expect(guest.getByRole('button', { name: 'Создать таблицу' })).toHaveCount(0);
    await guest.getByLabel('PIN или пароль').fill(pin);
    await guest.getByRole('button', { name: 'Открыть таблицу' }).click();
    await expect(guest.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(await editorButton(guest, 'Комментарии')).toBeVisible();
    await guest.keyboard.press('Escape');
    await expect(guest.getByRole('button', { name: 'Поделиться' })).toHaveCount(0);
    await expect(guest.getByRole('button', { name: 'Добавить лист' })).toBeDisabled();

    await (await editorButton(guest, 'Комментарии')).click();
    await guest.getByLabel('Ваш комментарий').fill('Тестовый комментарий гостя');
    await guest.getByRole('button', { name: 'Добавить комментарий' }).click();
    await expect(guest.getByText('Тестовый комментарий гостя')).toBeVisible();

    await dialog.getByRole('button', { name: 'Отозвать' }).click();
    await expect(dialog.getByText('Отозвана')).toBeVisible();
    await expect(guest.getByRole('heading', { name: 'Доступ по ссылке закрыт' })).toBeVisible({
      timeout: 10_000,
    });
  } finally {
    await guestContext.close();
  }
});

test('sharing dialog supports keyboard, paste, clipboard, and indefinite confirmation', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const externalRequests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.protocol === 'http:' || url.protocol === 'https:') {
      if (!['localhost', '127.0.0.1'].includes(url.hostname)) externalRequests.push(url.href);
    }
  });
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  const opener = page.getByRole('button', { name: 'Поделиться' });
  await opener.click();
  const dialog = page.getByRole('dialog', { name: 'Поделиться таблицей' });
  await expect
    .poll(() => dialog.evaluate((element) => element.contains(document.activeElement)))
    .toBe(true);
  await page.keyboard.press('Shift+Tab');
  await expect
    .poll(() => dialog.evaluate((element) => element.contains(document.activeElement)))
    .toBe(true);
  const ttl = dialog.getByRole('slider', { name: 'Срок доступа' });
  await expect(ttl).toHaveAttribute('aria-valuetext', '1 день');

  const pinInputs = dialog.getByRole('group', { name: 'PIN из шести цифр' }).locator('input');
  await pinInputs.first().focus();
  await page.evaluate(() => navigator.clipboard.writeText('739204'));
  await pinInputs.first().press('Control+V');
  await expect
    .poll(() =>
      pinInputs.evaluateAll((inputs) => inputs.map((input) => (input as HTMLInputElement).value)),
    )
    .toEqual(['7', '3', '9', '2', '0', '4']);

  await ttl.focus();
  await ttl.press('Home');
  await expect(ttl).toHaveAttribute('aria-valuetext', '1 час');
  await ttl.press('End');
  await expect(ttl).toHaveAttribute('aria-valuetext', 'Бессрочно');
  const indefinite = dialog.locator('.sharing-indefinite');
  await expect(indefinite).toHaveCSS('border-top-color', 'rgb(215, 146, 146)');
  await expect(dialog.locator('.sharing-warning-icon')).toHaveText('⚠');
  const confirmation = dialog.getByRole('checkbox', {
    name: 'Я понимаю, что эта ссылка не имеет срока действия, и отзову её, когда доступ больше не нужен',
  });
  await confirmation.check();
  await ttl.press('ArrowLeft');
  await expect(ttl).toHaveAttribute('aria-valuetext', '1 неделя');
  await expect(confirmation).toHaveCount(0);
  await ttl.press('End');
  await dialog
    .getByRole('checkbox', {
      name: 'Я понимаю, что эта ссылка не имеет срока действия, и отзову её, когда доступ больше не нужен',
    })
    .check();

  const createResponse = page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname.endsWith('/shares'),
  );
  await dialog.getByRole('button', { name: 'Создать ссылку' }).click();
  const response = await createResponse;
  expect(response.ok()).toBeTruthy();
  const requestBody = response.request().postDataJSON() as {
    ttl: string;
    confirm_indefinite: boolean;
    pin: string;
  };
  expect(requestBody).toMatchObject({ ttl: 'indefinite', confirm_indefinite: true, pin: '739204' });

  const shareUrl = await dialog.getByRole('textbox', { name: 'Ссылка' }).inputValue();
  await dialog.getByRole('button', { name: 'Копировать ссылку' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Ссылка скопирована' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(shareUrl);
  await dialog.getByRole('button', { name: 'Копировать PIN' }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe('739204');
  await expect(dialog.getByText('Без срока действия').first()).toBeVisible();
  const indefiniteBadge = dialog.locator('.sharing-expiry-indefinite');
  await expect(indefiniteBadge).toHaveText('Бессрочно');
  await expect(indefiniteBadge).toHaveCSS('color', 'rgb(146, 46, 46)');

  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
  expect(externalRequests).toEqual([]);
});

test('sharing surfaces keep the light design palette for either system preference', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  await page.getByRole('button', { name: 'Поделиться' }).click();
  const dialog = page.getByRole('dialog', { name: 'Поделиться таблицей' });
  await page.emulateMedia({ colorScheme: 'light' });
  const lightSurface = await dialog.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  );
  await expect(dialog).toHaveCSS('color-scheme', 'light');
  await dialog.getByRole('slider', { name: 'Срок доступа' }).press('End');
  const lightWarning = await dialog
    .locator('.sharing-warning')
    .evaluate((element) => getComputedStyle(element).backgroundColor);
  await page.emulateMedia({ colorScheme: 'dark' });
  const darkSurface = await dialog.evaluate((element) => getComputedStyle(element).backgroundColor);
  const darkWarning = await dialog
    .locator('.sharing-warning')
    .evaluate((element) => getComputedStyle(element).backgroundColor);
  expect(darkSurface).toBe(lightSurface);
  expect(darkWarning).toBe(lightWarning);
  expect(lightWarning).not.toBe(lightSurface);
  await expect(dialog).toHaveCSS('color-scheme', 'light');
  await page.emulateMedia({ colorScheme: 'light' });
  await expect(dialog).toHaveCSS('color-scheme', 'light');
});
