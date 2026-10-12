import { selectValue } from './select';
import { editorButton } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
async function fixture(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
  return new URL(page.url()).searchParams.get('document')!;
}
async function policy(page: Page, id: string, allow: boolean, anonymous = false) {
  const path = `/api/documents/${id}/disclosure-policy`,
    current = await (await page.request.get(path)).json();
  expect(
    (
      await page.request.put(path, {
        headers: { Origin: new URL(page.url()).origin },
        data: {
          schema_version: 2,
          expected_revision: current.revision,
          rules: { ...current.rules, allow_copy: allow, require_anonymization: anonymous },
        },
      })
    ).status(),
  ).toBe(200);
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
}
async function condition(page: Page, kind: 'values' | 'text') {
  await page.getByRole('region', { name: 'Область таблицы' }).press('Control+Home');
  await (await editorButton(page, 'Вид листа')).click();
  const dialog = page.getByRole('dialog', { name: 'Вид листа', exact: true });
  await dialog.getByRole('button', { name: 'Добавить условие', exact: true }).click();
  await selectValue(dialog.getByLabel('Тип условия 1', { exact: true }), kind);
  return dialog;
}
test('modal data copy and input cut obey policy and allowed cut persists the controlled filter value', async ({
  page,
  context,
}) => {
  test.setTimeout(60000);
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const id = await fixture(page),
    bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('Modal protected value');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await policy(page, id, false);
  let dialog = await condition(page, 'values');
  await page.evaluate(() => navigator.clipboard.writeText('Keep clipboard'));
  await dialog.getByText('Modal protected value', { exact: true }).evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await dialog.focus();
  let response = page.waitForResponse((r) => r.url().endsWith('/clipboard/authorize'));
  await page.keyboard.press('Control+C');
  expect((await response).status()).toBe(403);
  await expect(dialog.getByRole('alert')).toContainText('Копирование запрещено');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await selectValue(dialog.getByLabel('Тип условия 1', { exact: true }), 'text');
  let value = dialog.getByLabel('Значение условия 1', { exact: true });
  await value.fill('ABCDE');
  await value.press('Control+A');
  response = page.waitForResponse((r) => r.url().endsWith('/clipboard/authorize'));
  await value.press('Control+X');
  expect((await response).status()).toBe(403);
  await expect(value).toHaveValue('ABCDE');
  await page.keyboard.press('Escape');
  await policy(page, id, true);
  dialog = await condition(page, 'text');
  value = dialog.getByLabel('Значение условия 1', { exact: true });
  await value.fill('ABCDE');
  await value.evaluate((input) => (input as HTMLInputElement).setSelectionRange(1, 4));
  await value.press('Control+X');
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe('BCD');
  await expect(value).toHaveValue('AE');
  await dialog.getByRole('button', { name: 'Сохранить фильтр', exact: true }).click();
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/documents/${id}/personal-view`)).json()).settings
          .filters[0]?.conditions[0]?.predicate.value,
    )
    .toBe('AE');
  const audit = await (await page.request.get(`/api/documents/${id}/audit`)).json();
  expect(
    audit.events.some(
      (e: { action: string; allowed: boolean }) => e.action === 'copy' && !e.allowed,
    ),
  ).toBe(true);
  expect(
    audit.events.some(
      (e: { action: string; allowed: boolean }) => e.action === 'cut' && !e.allowed,
    ),
  ).toBe(true);
});
test('unmapped modal text cannot issue a formula-derived scalar when anonymization is mandatory', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const id = await fixture(page),
    bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('test@example.com');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.press('Control+Home');
  await grid.press('ArrowRight');
  await bar.fill('=LEN(A1)');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  const head = await (await page.request.get('/api/documents/' + id)).json();
  expect(
    (
      await page.request.put(`/api/documents/${id}/privacy`, {
        headers: { Origin: new URL(page.url()).origin },
        data: {
          schema_version: 1,
          document_version: head.version,
          expected_revision: 0,
          rules: { enabled: true, types: ['email'], exceptions: [] },
        },
      })
    ).status(),
  ).toBe(200);
  await policy(page, id, true, true);
  await grid.press('Control+Home');
  await (await editorButton(page, 'Вид листа')).click();
  const dialog = page.getByRole('dialog', { name: 'Вид листа', exact: true });
  await selectValue(dialog.getByLabel('Последний столбец', { exact: true }), '1');
  await dialog.getByRole('button', { name: 'Добавить условие', exact: true }).click();
  await selectValue(
    dialog.getByLabel('Столбец условия 1', { exact: true }),
    head.sheets[0].columns[1],
  );
  await selectValue(dialog.getByLabel('Тип условия 1', { exact: true }), 'values');
  await dialog.getByText('16', { exact: true }).evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await dialog.focus();
  await page.evaluate(() => navigator.clipboard.writeText('Keep clipboard'));
  await page.keyboard.press('Control+C');
  await expect(dialog.getByRole('alert')).toContainText('Для этого представления');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await page.keyboard.press('Escape');
  const active = page.getByTestId('active-cell');
  const openFormulaCell = async () => {
    await page.reload();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await (await editorButton(page, 'Показать оригиналы')).click();
    await grid.press('Control+Home');
    await grid.press('ArrowRight');
    await expect(active).toHaveText('B1: 16');
    await expect(page.getByRole('alert')).toHaveCount(0);
  };
  await openFormulaCell();
  await active.evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Control+C');
  await expect(page.getByRole('alert')).toContainText('Для этого представления');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await openFormulaCell();
  await bar.focus();
  await bar.press('Control+A');
  await bar.press('Control+C');
  await expect(page.getByRole('alert')).toContainText('Для этого представления');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await openFormulaCell();
  await bar.focus();
  await bar.press('Control+A');
  const deniedCut = page.waitForResponse((r) => r.url().endsWith('/clipboard/authorize'));
  await bar.press('Control+X');
  expect((await deniedCut).status()).toBe(403);
  await expect(page.getByRole('alert')).toContainText('Копирование запрещено');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('Keep clipboard');
  await expect(bar).toHaveValue('=LEN(A1)');
  await bar.press('Escape');
  await grid.focus();
  await grid.press('Control+C');
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .not.toBe('Keep clipboard');
  expect(await page.evaluate(() => navigator.clipboard.readText())).not.toContain('16');
});
