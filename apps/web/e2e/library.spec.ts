import { editorButton } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { uuidV7 } from '../src/document/store';

const origin = () => String(test.info().project.use.baseURL);
const headers = () => ({ Origin: new URL(origin()).origin });
test.beforeEach(async ({ page, context }) => {
  await context.clearCookies();
  expect(
    (
      await page.request.post('/api/dev/session', {
        headers: {
          ...headers(),
          'X-PXEL-DEV-ACCOUNT': 'library_' + crypto.randomUUID().replaceAll('-', '').slice(0, 20),
        },
      })
    ).status(),
  ).toBe(204);
});
test.afterEach(async ({ page }) => {
  await page.request.delete('/api/dev/session', { headers: headers() });
});
async function home(page: Page) {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Рабочие книги', exact: true })).toBeVisible();
  await expect(page.getByText('Загружаем библиотеку…', { exact: true })).toHaveCount(0);
}
async function create(page: Page) {
  await home(page);
  await createBookFromUi(page);
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  return new URL(page.url()).searchParams.get('document')!;
}
async function rename(page: Page, title: string) {
  const opener = page.getByRole('button', { name: 'Переименовать книгу', exact: true });
  await opener.click();
  const dialog = page.getByRole('dialog', { name: 'Название книги' });
  await dialog.getByLabel('Название', { exact: true }).fill(title);
  await dialog.getByRole('button', { name: 'Сохранить название', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
}
async function book(page: Page, id: string) {
  return (await page.request.get('/api/documents/' + id)).json();
}

test('library searches names preserves favorites recent and layout after reload without changing the book', async ({
  page,
}) => {
  const id = await create(page),
    title = 'Бюджет команды 🔑';
  const before = await book(page, id);
  await rename(page, title);
  await (await editorButton(page, 'Библиотека')).click();
  await expect(page.getByRole('link', { name: title, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'В избранное: ' + title, exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Убрать из избранного: ' + title, exact: true }),
  ).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Избранные', exact: true }).click();
  await page.getByRole('button', { name: 'Список', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Список', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await page.reload();
  await expect(page.getByRole('link', { name: title, exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Список', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await page.getByRole('searchbox', { name: 'Поиск по названию' }).fill('КОМАНДЫ 🔑');
  await expect(page.locator('[data-book-id]')).toHaveCount(1);
  await page.getByRole('searchbox', { name: 'Поиск по названию' }).fill('нет такой книги');
  await expect(page.getByText('Книги не найдены', { exact: true })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Поиск по названию' }).fill('');
  await page.getByRole('button', { name: 'Недавние', exact: true }).click();
  await expect(page.getByRole('link', { name: title, exact: true })).toBeVisible();
  await page.getByRole('link', { name: title, exact: true }).click();
  await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
  const after = await book(page, id);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
});

test('all three templates contain working formulas formatting merges and frozen headers', async ({
  page,
}) => {
  for (const [title, address, result] of [
    ['Бюджет', 'D10', '17000'],
    ['Учёт задач', 'B10', '2'],
    ['План продаж', 'B10', '850000'],
  ] as const) {
    await home(page);
    await page.getByRole('button', { name: 'Создать из шаблона: ' + title, exact: true }).click();
    await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
    await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
    const id = new URL(page.url()).searchParams.get('document')!,
      head = await book(page, id);
    expect(head.sheets[0].frozen_rows).toBe(2);
    expect(head.sheets[0].merges).toHaveLength(1);
    expect(
      head.sheets[0].styles.some((s: { style: { font_bold: boolean } }) => s.style.font_bold),
    ).toBe(true);
    expect(
      head.sheets[0].cells.some((c: { value: { type: string } }) => c.value.type === 'formula'),
    ).toBe(true);
    const grid = page.getByRole('region', { name: 'Область таблицы' });
    await grid.focus();
    await grid.press('Control+Home');
    for (let row = 1; row < Number(address!.slice(1)); row++) await grid.press('ArrowDown');
    for (let column = 0; column < address!.charCodeAt(0) - 65; column++)
      await grid.press('ArrowRight');
    await expect(page.getByTestId('active-cell')).toHaveText(address + ': ' + result);
    await page.reload();
    await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
  }
});

test('catalog exposes only current ACL and shared titles cannot be renamed by viewers', async ({
  page,
  browser,
}) => {
  const id = await create(page),
    title = 'Секретная книга каталога';
  await rename(page, title);
  const other = await browser.newContext({ baseURL: origin() });
  try {
    const account = 'catalog_' + crypto.randomUUID().replaceAll('-', '').slice(0, 20);
    expect(
      (
        await other.request.post('/api/dev/session', {
          headers: { ...headers(), 'X-PXEL-DEV-ACCOUNT': account },
        })
      ).status(),
    ).toBe(204);
    expect(
      JSON.stringify(await (await other.request.get('/api/documents?schema_version=1')).json()),
    ).not.toContain(title);
    const viewer = await (
        await other.request.get('/api/dev/identity', { headers: headers() })
      ).json(),
      owner = await (await page.request.get('/api/dev/identity', { headers: headers() })).json();
    await other.request.delete('/api/dev/session', { headers: headers() });
    expect(
      (
        await page.request.post(`/api/documents/${id}/permissions`, {
          headers: headers(),
          data: {
            action: 'grant',
            change_id: uuidV7(),
            target_user_id: viewer.actor_id,
            role: 'viewer',
          },
        })
      ).status(),
    ).toBe(200);
    expect(
      (
        await other.request.post('/api/dev/session', {
          headers: {
            ...headers(),
            'X-PXEL-DEV-ACCOUNT': account,
            'X-PXEL-DEV-WORKSPACE': owner.workspace_id,
          },
        })
      ).status(),
    ).toBe(204);
    const guestPage = await other.newPage();
    await home(guestPage);
    await guestPage.getByRole('button', { name: 'Доступные мне', exact: true }).click();
    await expect(guestPage.getByRole('link', { name: title, exact: true })).toBeVisible();
    expect(
      (
        await other.request.put(`/api/documents/${id}/metadata`, {
          headers: headers(),
          data: { schema_version: 1, expected_revision: 1, title: 'Cannot rename' },
        })
      ).status(),
    ).toBe(403);
    expect(
      (
        await page.request.post(`/api/documents/${id}/permissions`, {
          headers: headers(),
          data: { action: 'revoke', change_id: uuidV7(), target_user_id: viewer.actor_id },
        })
      ).status(),
    ).toBe(200);
    await guestPage.reload();
    await expect(guestPage.locator('[data-book-id]')).toHaveCount(0);
    expect((await other.request.get(`/api/documents/${id}/metadata`)).status()).toBe(403);
  } finally {
    await other.request.delete('/api/dev/session', { headers: headers() });
    await other.close();
  }
});

test('navigation blocks unfinished input and permits explicit stay save and discard', async ({
  page,
}) => {
  const id = await create(page),
    bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('Черновик');
  await (await editorButton(page, 'Библиотека')).click();
  const dialog = page.getByRole('dialog', { name: 'Несохранённые изменения' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Остаться в книге', exact: true }).click();
  await expect(bar).toHaveValue('Черновик');
  await (await editorButton(page, 'Библиотека')).click();
  await dialog.getByRole('button', { name: 'Сохранить и перейти', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Рабочие книги', exact: true })).toBeVisible();
  expect((await book(page, id)).sheets[0].cells[0].value.value).toBe('Черновик');
  await page.goto('/?document=' + id);
  await expect(bar).toBeEnabled();
  await bar.fill('Потерянный ввод');
  await (await editorButton(page, 'Библиотека')).click();
  await dialog.getByRole('button', { name: 'Уйти без ожидания', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Рабочие книги', exact: true })).toBeVisible();
  expect((await book(page, id)).sheets[0].cells[0].value.value).toBe('Черновик');
});

test('library title copy uses durable policy guard and multi-book selections and cut are blocked', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const id = await create(page),
    title = 'Protected library title';
  await rename(page, title);
  expect(
    (
      await page.request.put(`/api/documents/${id}/disclosure-policy`, {
        headers: headers(),
        data: {
          schema_version: 1,
          expected_revision: 0,
          rules: {
            allow_copy: false,
            export_formats: ['xlsx', 'pxel'],
            allow_participant_export: false,
            require_anonymization: false,
          },
        },
      })
    ).status(),
  ).toBe(200);
  await home(page);
  await page.evaluate(() => navigator.clipboard.writeText('Keep clipboard'));
  await page.getByRole('link', { name: title, exact: true }).evaluate((link) => {
    const selection = getSelection()!,
      range = document.createRange();
    range.selectNodeContents(link);
    selection.removeAllRanges();
    selection.addRange(range);
  });
  const denied = page.waitForResponse(
    (r) => r.url().endsWith('/clipboard/authorize') && r.request().method() === 'POST',
  );
  await page.keyboard.press('Control+C');
  expect((await denied).status()).toBe(403);
  await expect(page.getByRole('alert')).toContainText('Копирование запрещено');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await page.keyboard.press('Control+X');
  await expect(page.getByRole('alert')).toContainText('только для копирования');
  await page.locator('main.library').evaluate((main) => {
    const selection = getSelection()!,
      range = document.createRange();
    range.selectNodeContents(main);
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await page.keyboard.press('Control+C');
  await expect(page.getByRole('alert')).toContainText('одной книги');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
});

test('PXEL and Markdown imports are available from home and passwords remain outside the URL', async ({
  page,
}) => {
  const id = await create(page),
    title = 'Переносимая книга';
  await rename(page, title);
  const head = await book(page, id),
    password = 'password for library 2026';
  const exported = await page.request.post(`/api/documents/${id}/export.pxel`, {
    headers: headers(),
    data: { schema_version: 1, document_version: head.version, password },
  });
  expect(exported.status()).toBe(200);
  const bytes = await exported.body();
  await home(page);
  await page.getByRole('button', { name: 'Импорт .pxel', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Файл PXEL' });
  await dialog
    .getByLabel('Файл .pxel')
    .setInputFiles({ name: 'book.pxel', mimeType: 'application/octet-stream', buffer: bytes });
  await dialog.getByLabel('Пароль файла', { exact: true }).fill(password);
  await dialog.getByRole('button', { name: 'Проверить состав', exact: true }).click();
  await expect(dialog.getByRole('region', { name: 'Состав файла' })).toContainText(title);
  await dialog.getByRole('button', { name: 'Создать новую книгу из файла', exact: true }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('document')).not.toBe(id);
  await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
  expect(page.url()).not.toContain(password);
  await home(page);
  await page.getByLabel('Markdown-файл для новой книги').setInputFiles({
    name: 'table.md',
    mimeType: 'text/markdown',
    buffer: bytes.constructor.from('| Name | Count |\n| --- | --- |\n| Team | 7 |'),
  });
  await expect(page.getByTestId('markdown-import')).toBeVisible();
  await page
    .getByTestId('markdown-import')
    .getByRole('button', { name: /Вставить таблицу/ })
    .click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Name');
});

for (const width of [390, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`home keyboard modal focus empty error and layout ${width} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await home(page);
      await expect(page.getByText('Здесь будут ваши книги', { exact: true })).toBeVisible();
      const opener = page.getByRole('button', { name: 'Импорт .pxel', exact: true });
      await opener.focus();
      await page.keyboard.press('Enter');
      const dialog = page.getByRole('dialog', { name: 'Файл PXEL' });
      await expect(dialog.locator(':focus')).toHaveCount(1);
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
      await expect(opener).toBeFocused();
      await page.getByRole('button', { name: 'Список', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('button', { name: 'Список', exact: true })).toHaveAttribute(
        'aria-pressed',
        'true',
      );
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
      await page.route('**/api/documents?schema_version=1', (route) =>
        route.fulfill({ status: 503, body: '{}' }),
      );
      await page.reload();
      await expect(page.getByRole('alert')).toContainText('Не удалось');
      await page.unroute('**/api/documents?schema_version=1');
      await page.getByRole('button', { name: 'Обновить библиотеку', exact: true }).click();
      await expect(page.getByRole('alert')).toHaveCount(0);
      await createBookFromUi(page);
      const rename = page.getByRole('button', { name: 'Переименовать книгу', exact: true });
      await rename.focus();
      await page.keyboard.press('Enter');
      const nameDialog = page.getByRole('dialog', { name: 'Название книги' });
      await expect(nameDialog.getByLabel('Название', { exact: true })).toBeFocused();
      await page.keyboard.press('Shift+Tab');
      await expect(nameDialog.locator(':focus')).toHaveCount(1);
      await page.keyboard.press('Escape');
      await expect(rename).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
