import { selectValue } from './select';
import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { GRID } from '../src/grid/viewport';
import { uuidV7, type Projection } from '../src/document/store';
const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
async function book(page: Page): Promise<Projection> {
  return (
    await page.request.get(`/api/documents/${new URL(page.url()).searchParams.get('document')}`)
  ).json();
}
async function fixture(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const previous = new URL(page.url()).searchParams.get('document');
  await createBookFromUi(page);
  await expect.poll(() => new URL(page.url()).searchParams.get('document')).not.toBe(previous);
  await expect(await editorButton(page, 'Журнал и версии')).toBeEnabled();
  await page.keyboard.press('Escape');
}
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
}
async function write(page: Page, row: number, column: number, value: string) {
  await select(page, row, column);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill(value);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
}
test('owner activity historical formula results structural diff and untouched Undo', async ({
  page,
}) => {
  await fixture(page);
  await write(page, 0, 0, '7');
  await write(page, 0, 1, '=A1*2');
  const before = await book(page);
  await write(page, 0, 0, '8');
  await select(page, 0, 1);
  await page.getByRole('button', { name: 'Полужирный', exact: true }).click();
  await expect.poll(async () => (await book(page)).version).not.toBe(before.version);
  const styled = await book(page);
  await (await editorButton(page, 'Строка ↑')).click();
  await expect
    .poll(async () => (await book(page)).sheets[0]!.rows.length)
    .toBe(styled.sheets[0]!.rows.length + 1);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const head = await book(page),
    path = `/api/documents/${head.document_id}`,
    undo = await (await page.request.get(path + '/undo-history')).json();
  const activity = await (
    await page.request.get(`${path}/history?schema_version=1&through_version=${head.version}`)
  ).json();
  expect(activity.events[0].structural).toBe(true);
  expect(
    activity.events.some((e: { operation_type: string }) => e.operation_type === 'style_write'),
  ).toBe(true);
  expect(JSON.stringify(activity)).not.toContain('=A1');
  expect(activity.events.every((e: { actor_id: string | null }) => e.actor_id !== null)).toBe(true);
  const opener = editorTrigger(page, 'Журнал и версии');
  await (await editorButton(page, 'Журнал и версии')).click();
  const dialog = page.getByRole('dialog', { name: 'Журнал и версии' });
  await expect(
    dialog.getByRole('button', { name: `Просмотр версии ${before.version}`, exact: true }),
  ).toBeEnabled();
  await dialog
    .getByRole('button', { name: `Просмотр версии ${before.version}`, exact: true })
    .click();
  const historical = dialog.getByRole('region', { name: 'Историческая версия' });
  await expect(historical).toContainText('Только чтение');
  await expect(historical).toContainText('=(A1*2)');
  await expect(historical).toContainText('Результат: 14');
  expect(await historical.getByRole('textbox').count()).toBe(0);
  await dialog.getByLabel('Версия до', { exact: true }).fill(before.version);
  await dialog.getByLabel('Версия после', { exact: true }).fill(head.version);
  await dialog.getByRole('button', { name: 'Сравнить версии', exact: true }).click();
  const comparison = dialog.getByRole('region', { name: 'Сравнение версий' });
  await expect(comparison).toContainText('Ссылки формул сравниваются');
  await selectValue(comparison.getByLabel('Тип изменений'), 'formula');
  await expect(comparison).toContainText('Ссылка изменилась');
  await expect(comparison).toContainText('=(A1*2)');
  await expect(comparison).toContainText('=(A2*2)');
  await selectValue(comparison.getByLabel('Тип изменений'), 'calculated');
  await expect(comparison.locator('.diff-before')).toContainText('14');
  await expect(comparison.locator('.diff-after')).toContainText('16');
  await selectValue(comparison.getByLabel('Тип изменений'), 'style');
  await expect(comparison).toContainText('font_bold');
  await expect(comparison.locator('.diff-before')).toContainText('false');
  await expect(comparison.locator('.diff-after')).toContainText('true');
  expect((await book(page)).version).toBe(head.version);
  expect((await book(page)).state_hash).toBe(head.state_hash);
  expect((await book(page)).calculation_context).toEqual(head.calculation_context);
  expect(await (await page.request.get(path + '/undo-history')).json()).toEqual(undo);
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('=(A2*2)');
  await page.getByRole('button', { name: 'Отменить последнюю правку' }).click();
  await expect.poll(async () => (await book(page)).sheets[0]!.rows).toEqual(styled.sheets[0]!.rows);
});
test('history diff owner guards unknown schemas and unknown versions fail without changes', async ({
  page,
  browser,
}) => {
  await fixture(page);
  await write(page, 0, 0, '42');
  const head = await book(page),
    path = `/api/documents/${head.document_id}`,
    origin = new URL(page.url()).origin;
  expect(
    (
      await page.request.get(path + '/history?schema_version=2&through_version=' + head.version)
    ).status(),
  ).toBe(400);
  expect(
    (
      await page.request.get(
        path + '/diff?schema_version=2&from_version=1&to_version=' + head.version,
      )
    ).status(),
  ).toBe(400);
  expect((await page.request.get(path + '/history/999999')).status()).toBe(404);
  const outsider = await browser.newContext();
  try {
    const account = `history_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
    expect(
      (
        await outsider.request.post(origin + '/api/dev/session', {
          headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': account },
        })
      ).status(),
    ).toBe(204);
    const viewer = await (
      await outsider.request.get(origin + '/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    const owner = await (
      await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    const queries = [
      '/history?schema_version=1&through_version=' + head.version,
      '/history/1',
      '/diff?schema_version=1&from_version=1&to_version=' + head.version,
    ];
    for (const suffix of queries)
      expect((await outsider.request.get(origin + path + suffix)).status()).toBe(403);
    await outsider.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    expect(
      (
        await page.request.post(path + '/permissions', {
          headers: { Origin: origin },
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
        await outsider.request.post(origin + '/api/dev/session', {
          headers: {
            Origin: origin,
            'X-PXEL-DEV-ACCOUNT': account,
            'X-PXEL-DEV-WORKSPACE': owner.workspace_id,
          },
        })
      ).status(),
    ).toBe(204);
    for (const suffix of queries)
      expect((await outsider.request.get(origin + path + suffix)).status()).toBe(403);
  } finally {
    await outsider.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    await outsider.close();
  }
  expect((await book(page)).version).toBe(head.version);
  expect((await book(page)).state_hash).toBe(head.state_hash);
});
for (const width of [390, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`history keyboard table and diff ${width} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await fixture(page);
      await write(page, 0, 0, '17');
      const head = await book(page),
        opener = editorTrigger(page, 'Журнал и версии');
      await (await editorButton(page, 'Журнал и версии')).click();
      const dialog = page.getByRole('dialog', { name: 'Журнал и версии' });
      await expect(
        dialog.getByRole('button', { name: 'Открыть версию', exact: true }),
      ).toBeEnabled();
      await dialog.getByRole('button', { name: 'Открыть версию', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(dialog.getByRole('region', { name: 'Историческая версия' })).toContainText('17');
      await dialog.getByRole('button', { name: 'Сравнить версии', exact: true }).click();
      await expect(dialog.getByRole('region', { name: 'Сравнение версий' })).toContainText(
        'Добавлено',
      );
      expect((await book(page)).version).toBe(head.version);
      await dialog.getByRole('button', { name: 'Более ранние действия', exact: true }).count();
      const close = dialog.getByRole('button', { name: 'Закрыть окно', exact: true });
      await close.focus();
      await page.keyboard.press('Shift+Tab');
      await expect(dialog.locator(':focus')).toHaveCount(1);
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
