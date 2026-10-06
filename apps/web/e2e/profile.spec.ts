import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { GRID } from '../src/grid/viewport';
import { uuidV7, type Projection } from '../src/document/store';
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
  await expect(await editorButton(page, 'Профайлер')).toBeEnabled();
  await page.keyboard.press('Escape');
}
async function write(page: Page, column: number, source: string) {
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + GRID.rowHeight / 2,
    },
  });
  const input = page.getByRole('textbox', { name: 'Значение ячейки' });
  await input.fill(source);
  await input.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
}
test('fixed context profiling preserves results and head and exposes potential branches and cycles', async ({
  page,
}) => {
  await fixture(page);
  for (const [column, source] of [
    '2',
    '=A1*2',
    '=IF(TRUE,B1,A2)',
    '=E1',
    '=D1',
    '=RAND()',
  ].entries())
    await write(page, column, source);
  await expect
    .poll(async () => (await book(page)).calculation_context?.context_id)
    .not.toBeUndefined();
  const before = await book(page),
    path = `/api/documents/${before.document_id}`;
  await expect
    .poll(async () => (await (await page.request.get(`${path}/calculation`)).json()).status)
    .toBe('complete');
  const calculated = await (await page.request.get(`${path}/calculation`)).json();
  const history = await (await page.request.get(`${path}/undo-history`)).json();
  const responsePromise = page.waitForResponse(
    (response) => response.url().endsWith('/profile') && response.request().method() === 'POST',
  );
  const opener = editorTrigger(page, 'Профайлер');
  await (await editorButton(page, 'Профайлер')).click();
  const response = await responsePromise;
  expect(response.status()).toBe(200);
  const report = await response.json();
  expect(report.formula_count).toBe(5);
  expect(report.document_version).toBe(before.version);
  expect(report.context_id).toBe(before.calculation_context!.context_id);
  expect(report.calculation_epoch).toBe(before.calculation_context!.epoch);
  expect(report.result_hash).toBe(calculated.calculated_state_hash);
  expect(
    report.formulas.filter((formula: { cycle_or_blocked: boolean }) => formula.cycle_or_blocked),
  ).toHaveLength(2);
  const dialog = page.getByRole('dialog', { name: 'Профайлер формул' });
  await expect(dialog.getByRole('status').first()).toContainText('Формул: 5');
  await dialog.getByRole('button', { name: 'Лист 1!C1', exact: true }).click();
  await expect(
    dialog.getByRole('button', { name: 'Выбрать цепочку Лист 1!A2', exact: true }),
  ).toBeVisible();
  await dialog.getByText('Текстовое представление цепочки', { exact: true }).click();
  await expect(dialog.locator('details')).toContainText('Лист 1!A2');
  const node = dialog.getByRole('button', { name: 'Выбрать цепочку Лист 1!B1', exact: true });
  await node.focus();
  await node.press('Enter');
  await expect(
    dialog.getByRole('button', { name: 'Выбрать цепочку Лист 1!A1', exact: true }),
  ).toBeVisible();
  const after = await book(page);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.calculation_context).toEqual(before.calculation_context);
  expect(await (await page.request.get(`${path}/undo-history`)).json()).toEqual(history);
  expect((await (await page.request.get(`${path}/calculation`)).json()).calculated_state_hash).toBe(
    calculated.calculated_state_hash,
  );
  expect(
    (
      await page.request.post(`${path}/recalculate`, {
        headers: { Origin: new URL(page.url()).origin },
        data: { operation_id: uuidV7() },
      })
    ).status(),
  ).toBe(200);
  await expect(dialog.getByRole('alert')).toContainText('Отчёт устарел');
  await page.keyboard.press('Escape');
  await expect(opener).toBeFocused();
});
test('owner authorization version validation and graph truncation are atomic', async ({
  page,
  browser,
}) => {
  await fixture(page);
  await write(page, 1, '=SUM(A1:A1000)');
  const before = await book(page),
    path = `/api/documents/${before.document_id}`,
    origin = new URL(page.url()).origin;
  const post = (version: string, schema = 1) =>
    page.request.post(`${path}/profile`, {
      headers: { Origin: origin },
      data: { schema_version: schema, document_version: version },
    });
  expect((await post('1')).status()).toBe(409);
  expect((await post(before.version, 2)).status()).toBe(400);
  const report = await (await post(before.version)).json();
  expect(report.graph.truncated).toBe(true);
  expect(report.graph.nodes).toHaveLength(500);
  expect(report.graph.edges.length).toBeLessThanOrEqual(2000);
  const outsider = await browser.newContext();
  try {
    const account = `profile_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
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
    expect(
      (
        await outsider.request.post(origin + `${path}/profile`, {
          headers: { Origin: origin },
          data: { schema_version: 1, document_version: before.version },
        })
      ).status(),
    ).toBe(403);
    const owner = await (
      await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    await outsider.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    expect(
      (
        await page.request.post(`${path}/permissions`, {
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
    expect(
      (
        await outsider.request.post(origin + `${path}/profile`, {
          headers: { Origin: origin },
          data: { schema_version: 1, document_version: before.version },
        })
      ).status(),
    ).toBe(403);
  } finally {
    await outsider.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    await outsider.close();
  }
  const after = await book(page);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  await (await editorButton(page, 'Профайлер')).click();
  await expect(
    page.getByRole('dialog', { name: 'Профайлер формул' }).getByRole('status').last(),
  ).toContainText('Граф усечён');
});
for (const width of [390, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`profiler keyboard graph and layout ${width} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await fixture(page);
      await write(page, 0, '=2');
      await write(page, 1, '=A1*3');
      const opener = editorTrigger(page, 'Профайлер');
      await (await editorButton(page, 'Профайлер')).click();
      const dialog = page.getByRole('dialog', { name: 'Профайлер формул' });
      await expect(dialog.getByRole('status').first()).toContainText('Формул: 2');
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      await dialog.getByRole('button', { name: 'Лист 1!B1', exact: true }).click();
      await dialog.getByRole('button', { name: 'Выбрать цепочку Лист 1!B1', exact: true }).focus();
      await page.keyboard.press('Enter');
      await expect(
        dialog.getByRole('button', { name: 'Выбрать цепочку Лист 1!A1', exact: true }),
      ).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
