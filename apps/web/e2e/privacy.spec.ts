import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { xml } from './zip-fixture.mjs';
import { GRID } from '../src/grid/viewport';
import { uuidV7, type Projection } from '../src/document/store';
import { copySelection, readClipboardText } from './clipboard';
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
  await expect(await editorButton(page, 'Персональные данные')).toBeEnabled();
  await page.keyboard.press('Escape');
}
async function write(page: Page, column: number, text: string) {
  await writeAt(page, 0, column, text);
}
async function writeAt(page: Page, row: number, column: number, text: string) {
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill(text);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
}
test('local mask preserves owner editing and the originals choice after automatic analysis', async ({
  page,
}) => {
  await fixture(page);
  await write(page, 0, 'test@example.com');
  await (await editorButton(page, 'Персональные данные')).click();
  const dialog = page.getByRole('dialog', { name: 'Персональные данные' });
  const reportStatus = dialog.getByRole('status').filter({ hasText: 'Совпадений:' });
  await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).check();
  await dialog.getByRole('checkbox', { name: 'Email', exact: true }).check();
  await dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }).click();
  await expect(reportStatus).toContainText('Совпадений: 1');
  await page.keyboard.press('Escape');
  await expect(page.locator('.privacy-veil')).toHaveCount(1);
  const veil = page.getByRole('button', { name: /^(Раскрыть|Скрыть) данные A1$/ });
  await veil.focus();
  await page.keyboard.press('F2');
  const cellEditor = page.getByRole('textbox', { name: 'Редактор ячейки', exact: true });
  await expect(cellEditor).toHaveValue('test@example.com');
  await cellEditor.press('Escape');
  await page.locator('.privacy-veil').first().dblclick();
  await expect(cellEditor).toHaveValue('test@example.com');
  await cellEditor.press('Escape');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.focus();
  await expect(bar).toHaveValue('test@example.com');
  await bar.fill('edited@example.org');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(veil).toBeVisible();
  await (await editorButton(page, 'Персональные данные')).click();
  await expect(reportStatus).toContainText(`Версия ${(await book(page)).version}.`);
  await dialog.getByRole('button', { name: 'Показать оригиналы', exact: true }).click();
  await page.keyboard.press('Escape');
  await write(page, 1, '42');
  const version = (await book(page)).version;
  await (await editorButton(page, 'Персональные данные')).click();
  await expect(reportStatus).toContainText(`Версия ${version}.`);
  await expect(dialog.getByRole('button', { name: 'Локальная маска', exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Обновить анализ', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Обновить анализ', exact: true })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: 'Локальная маска', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('.privacy-veil')).toHaveCount(0);
  await write(page, 0, 'final@example.net');
  expect(JSON.stringify((await book(page)).sheets)).toContain('final@example.net');
});
test('recognition safe clipboard XLSX and anonymous guest channels preserve originals', async ({
  page,
  browser,
  context,
}) => {
  await fixture(page);
  for (const [column, text] of [
    'test@example.com',
    '=LEN(A1)',
    '=IF(FALSE,A1,"public")',
    '=LEN("other@example.org")',
    '5',
  ].entries())
    await write(page, column, text);
  const before = await book(page),
    path = `/api/documents/${before.document_id}`,
    origin = new URL(page.url()).origin;
  const rules = {
    enabled: true,
    types: ['phone', 'passport', 'snils', 'inn', 'email', 'card'],
    exceptions: [],
  };
  expect(
    (
      await page.request.put(`${path}/privacy`, {
        headers: { Origin: origin },
        data: { schema_version: 1, document_version: before.version, expected_revision: 0, rules },
      })
    ).status(),
  ).toBe(200);
  const report = await (
    await page.request.post(`${path}/privacy/analyze`, {
      headers: { Origin: origin },
      data: { schema_version: 1, document_version: before.version },
    })
  ).json();
  expect(report.affected).toBe(4);
  expect(report.findings.length).toBe(2);
  const safe = JSON.stringify(report.projection);
  for (const secret of [
    'test@example.com',
    'other@example.org',
    'bound_formula',
    'state_hash',
    'calculation_context',
    '=LEN',
  ])
    expect(safe).not.toContain(secret);
  const after = await book(page);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.calculation_context).toEqual(before.calculation_context);
  const stamp = after.capabilities!.disclosure_stamp;
  const permit = await page.request.post(`${path}/clipboard/authorize`, {
    headers: { Origin: origin },
    data: { schema_version: 1, action: 'copy_anonymized', stamp },
  });
  expect(permit.status()).toBe(200);
  expect((await permit.json()).anonymized).toBe(true);
  const exported = await page.request.get(`${path}/export.xlsx?anonymized=true`);
  expect(exported.status()).toBe(200);
  const output = xml(await exported.body());
  expect(output).toContain('СКРЫТО');
  expect(output).not.toContain('test@example.com');
  expect(output).not.toContain('<f>');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await (await editorButton(page, 'Персональные данные')).click();
  const dialog = page.getByRole('dialog', { name: 'Персональные данные' });
  await dialog.getByRole('button', { name: 'Обновить анализ', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('Замаскировано');
  await expect(
    dialog.getByRole('button', { name: 'Показать оригиналы', exact: true }),
  ).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('[СКРЫТО]');
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await copySelection(page);
  expect(await readClipboardText(page)).toBe('[СКРЫТО]');
  await (await editorButton(page, 'Показать оригиналы')).click();
  const sheet = before.sheets[0]!,
    address = { sheet_id: sheet.id, row_id: sheet.rows[0], column_id: sheet.columns[0] };
  expect(
    (
      await page.request.post(`${path}/comments`, {
        headers: { Origin: origin },
        data: { request_id: uuidV7(), ...address, text: 'Пишите test@example.com' },
      })
    ).status(),
  ).toBe(200);
  const created = await page.request.post(`${path}/shares`, {
    headers: { Origin: origin },
    data: {
      request_id: uuidV7(),
      permission: 'comment',
      ttl: 'day',
      confirm_indefinite: false,
      pin: '482615',
      anonymized: true,
    },
  });
  expect(created.status()).toBe(201);
  const link = await created.json(),
    guestPath = `/api/shared/${link.share.link_id}`;
  const bookPolicy = await (await page.request.get(`${path}/disclosure-policy`)).json();
  expect(
    (
      await page.request.put(`${path}/disclosure-policy`, {
        headers: { Origin: origin },
        data: {
          schema_version: 2,
          expected_revision: bookPolicy.revision,
          rules: { ...bookPolicy.rules, allow_participant_export: true },
        },
      })
    ).status(),
  ).toBe(200);
  const linkPolicyPath = `${path}/disclosure-policy?link_id=${link.share.link_id}`,
    linkPolicy = await (await page.request.get(linkPolicyPath)).json();
  expect(
    (
      await page.request.put(linkPolicyPath, {
        headers: { Origin: origin },
        data: {
          schema_version: 1,
          expected_revision: linkPolicy.revision,
          rules: { ...linkPolicy.rules, export_formats: ['xlsx'], allow_participant_export: true },
        },
      })
    ).status(),
  ).toBe(200);
  const guestContext = await browser.newContext();
  await guestContext.grantPermissions(['clipboard-read', 'clipboard-write']);
  try {
    const guest = await guestContext.newPage(),
      frames: string[] = [];
    guest.on('websocket', (socket) =>
      socket.on('framereceived', (frame) =>
        frames.push(
          typeof frame.payload === 'string'
            ? frame.payload
            : new TextDecoder().decode(frame.payload),
        ),
      ),
    );
    await guest.goto(origin + link.url);
    await guest.getByLabel('PIN или пароль').fill('482615');
    await guest.getByRole('button', { name: 'Открыть таблицу' }).click();
    await expect(guest.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(guest.getByText('Обезличенное представление · только значения')).toBeVisible();
    await expect(guest.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('[СКРЫТО]');
    await expect(guest.getByRole('button', { name: 'Добавить лист' })).toBeDisabled();
    const response = await guest.request.get(guestPath + '/document');
    expect(response.status()).toBe(200);
    const text = await response.text();
    for (const marker of [
      'test@example.com',
      'other@example.org',
      'bound_formula',
      'source',
      'random_seed',
      'state_hash',
    ])
      expect(text).not.toContain(marker);
    for (const suffix of ['/calculation-context', '/undo-history'])
      expect((await guest.request.get(guestPath + suffix)).status()).toBe(403);
    expect(
      (
        await guest.request.post(guestPath + '/recalculate', {
          headers: { Origin: origin },
          data: { operation_id: uuidV7() },
        })
      ).status(),
    ).toBe(403);
    await (await editorButton(guest, 'Комментарии')).click();
    await expect(guest.getByText('Пишите [СКРЫТО]', { exact: true })).toBeVisible();
    await guest.getByLabel('Ваш комментарий').fill('Ответ other@example.org');
    await guest.getByRole('button', { name: 'Добавить комментарий' }).click();
    await expect(guest.getByText('Ответ [СКРЫТО]', { exact: true })).toBeVisible();
    const originals = await (
      await page.request.get(
        `${path}/comments?${new URLSearchParams(address as Record<string, string>)}`,
      )
    ).text();
    expect(originals).toContain('other@example.org');
    await (await editorButton(guest, 'Комментарии')).click();
    await guest.getByRole('region', { name: 'Область таблицы' }).focus();
    await copySelection(guest);
    expect(await readClipboardText(guest)).toBe('[СКРЫТО]');
    const gexport = await guest.request.get(guestPath + '/export.xlsx');
    expect(gexport.status()).toBe(200);
    expect(xml(await gexport.body())).not.toContain('test@example.com');
    if (await (await editorButton(page, 'Показать оригиналы')).isVisible())
      await (await editorButton(page, 'Показать оригиналы')).click();
    await write(page, 4, '6');
    await expect
      .poll(
        async () =>
          JSON.parse(await (await guest.request.get(guestPath + '/document')).text()).version,
      )
      .toBe((await book(page)).version);
    expect(frames.join('')).not.toContain('test@example.com');
    expect(frames.join('')).not.toContain('other@example.org');
    const policy = await (await page.request.get(path + '/privacy')).json();
    expect(
      (
        await page.request.put(path + '/privacy', {
          headers: { Origin: origin },
          data: {
            schema_version: 1,
            document_version: (await book(page)).version,
            expected_revision: policy.revision,
            rules,
          },
        })
      ).status(),
    ).toBe(200);
    await expect(guest.getByRole('heading', { name: 'Доступ по ссылке закрыт' })).toBeVisible();
  } finally {
    await guestContext.close();
  }
});
test('automatic rectangular veil reveals only locally and excludes checked findings or an exact range', async ({
  page,
}, info) => {
  test.setTimeout(60000);
  await page.setViewportSize({ width: 1440, height: 900 });
  await fixture(page);
  for (const [row, column, value] of [
    [0, 0, 'Паспорт'],
    [0, 1, 'Email'],
    [3, 0, '4501123456'],
    [4, 0, '4501987654'],
    [3, 1, 'test@example.com'],
    [4, 1, 'other@example.org'],
    [3, 3, '4501123456'],
    [4, 3, '=LEN(B4)'],
    [6, 1, 'Паспорт'],
    [6, 2, '45 01 345678'],
  ] as const)
    await writeAt(page, row, column, value);
  const before = await book(page);
  await (await editorButton(page, 'Персональные данные')).click();
  const dialog = page.getByRole('dialog', { name: 'Персональные данные' });
  await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).check();
  await dialog.getByRole('checkbox', { name: 'Паспорта', exact: true }).check();
  await dialog.getByRole('checkbox', { name: 'Email', exact: true }).check();
  await dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('Совпадений: 5');
  await expect(dialog.getByRole('status')).toContainText('результатов: 6');
  await page.keyboard.press('Escape');
  await expect(page.locator('.privacy-veil')).toHaveCount(6);
  await page.screenshot({ path: `../../.local/privacy-veil-${info.project.name}.png` });
  expect(
    await page
      .locator('.privacy-veil')
      .first()
      .evaluate((element) => getComputedStyle(element).borderRadius),
  ).toBe('0px');
  await expect(page.getByRole('button', { name: 'Раскрыть данные D4', exact: true })).toHaveCount(
    0,
  );
  await expect(page.getByText('test@example.com', { exact: true })).toHaveCount(0);
  const veil = page.getByRole('button', { name: 'Раскрыть данные B4', exact: true });
  const labelOffset = () =>
    veil.locator('.privacy-veil-label').evaluate((element) => {
      const area = element.closest('.grid-surface')?.querySelector('.grid-scroll');
      if (!area) throw new Error('Grid scroll area is missing');
      // Read both rectangles together: the privacy-analysis status can resize the header.
      const label = element.getBoundingClientRect();
      const grid = area.getBoundingClientRect();
      return { x: label.x - grid.x, y: label.y - grid.y };
    });
  const fullOffset = await labelOffset();
  await veil.hover();
  await expect(
    page.locator('.privacy-veil-value').filter({ hasText: 'test@example.com' }),
  ).toBeVisible();
  await page.mouse.move(1300, 20);
  await expect(page.locator('.privacy-veil-value')).toHaveCount(0);
  await page.locator('.grid-scroll').evaluate((element) => {
    element.scrollLeft = 56;
    element.scrollTop = 98;
  });
  const partial = page.getByRole('button', { name: 'Раскрыть данные A4', exact: true });
  await expect(partial).toBeVisible();
  await expect.poll(async () => (await partial.boundingBox())?.width).toBeLessThan(60);
  await expect.poll(async () => (await partial.boundingBox())?.height).toBeLessThan(15);
  const clipped = (await partial.boundingBox())!,
    area = (await page.locator('.grid-scroll').boundingBox())!;
  expect(clipped.x).toBeGreaterThanOrEqual(area.x + GRID.rowHeaderWidth);
  expect(clipped.y).toBeGreaterThanOrEqual(area.y + GRID.columnHeaderHeight);
  await expect.poll(async () => (await labelOffset()).x).toBeCloseTo(fullOffset.x - 56, 0);
  await expect.poll(async () => (await labelOffset()).y).toBeCloseTo(fullOffset.y - 98, 0);
  await page.screenshot({ path: `../../.local/privacy-veil-partial-${info.project.name}.png` });
  await page.locator('.grid-scroll').evaluate((element) => {
    element.scrollLeft = 0;
    element.scrollTop = 0;
  });
  await veil.focus();
  await expect(page.locator('.privacy-veil-value')).toHaveText('test@example.com');
  await page.keyboard.press('Escape');
  await expect(page.locator('.privacy-veil-value')).toHaveCount(0);
  await page.reload();
  await expect(page.locator('.privacy-veil')).toHaveCount(6);
  await (await editorButton(page, 'Персональные данные')).click();
  await dialog.getByRole('checkbox', { name: 'Выбрать B4 · Email', exact: true }).check();
  await dialog.getByRole('checkbox', { name: 'Выбрать B5 · Email', exact: true }).check();
  await dialog.getByRole('button', { name: 'Исключить выбранные (2)', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('Совпадений: 3');
  await dialog.getByLabel('Диапазон исключений').fill('A5:A4');
  await dialog.getByRole('button', { name: 'Исключить диапазон', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('Совпадений: 1');
  await expect(dialog).toContainText('Исключений: 4');
  const after = await book(page);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(JSON.stringify(after.sheets)).toContain('test@example.com');
  await page.keyboard.press('Escape');
  await page.reload();
  await expect(page.locator('.privacy-veil')).toHaveCount(1);
});
for (const width of [390, 681, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`privacy settings keyboard local mask ${width} ${theme}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await fixture(page);
      await write(page, 0, 'test@example.com');
      const opener = editorTrigger(page, 'Персональные данные');
      await (await editorButton(page, 'Персональные данные')).click();
      const dialog = page.getByRole('dialog', { name: 'Персональные данные' });
      await expect(
        dialog.getByRole('checkbox', { name: 'Включить распознавание' }),
      ).not.toBeChecked();
      await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).check();
      await dialog.getByRole('checkbox', { name: 'Email', exact: true }).check();
      await dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }).click();
      await expect(dialog.getByRole('status')).toContainText('Совпадений: 1');
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      const checkbox = dialog.getByRole('checkbox', { name: 'Выбрать A1 · Email', exact: true });
      const checkBounds = (await checkbox.boundingBox())!;
      expect(checkBounds.width).toBe(20);
      expect(checkBounds.height).toBe(20);
      await checkbox.focus();
      await page.keyboard.press('Space');
      await expect(checkbox).toBeChecked();
      await expect(dialog.getByRole('button', { name: 'Исключить выбранные (1)' })).toBeEnabled();
      await dialog.getByRole('button', { name: 'Обновить анализ', exact: true }).click();
      await expect(
        dialog.getByRole('button', { name: 'Обновить анализ', exact: true }),
      ).toBeEnabled();
      await expect(checkbox).toBeChecked();
      await checkbox.focus();
      await page.keyboard.press('Space');
      await expect(checkbox).not.toBeChecked();
      expect(await checkbox.evaluate((element) => getComputedStyle(element).outlineStyle)).toBe(
        'solid',
      );
      const close = dialog.getByRole('button', { name: 'Закрыть окно' });
      const closeBounds = (await close.boundingBox())!;
      const iconBounds = (await close.locator('svg').boundingBox())!;
      expect(iconBounds.x + iconBounds.width / 2).toBeCloseTo(
        closeBounds.x + closeBounds.width / 2,
        0,
      );
      expect(iconBounds.y + iconBounds.height / 2).toBeCloseTo(
        closeBounds.y + closeBounds.height / 2,
        0,
      );
      const gaps = await dialog
        .locator('.sharing-actions')
        .evaluateAll((elements) =>
          elements.map((element) => Number.parseFloat(getComputedStyle(element).gap)),
        );
      expect(gaps.length).toBe(3);
      expect(gaps.every((gap) => gap >= 10)).toBe(true);
      const address = (await dialog
        .getByRole('button', { name: 'A1 · Email', exact: true })
        .boundingBox())!;
      expect(address.x - checkBounds.x - checkBounds.width).toBeCloseTo(12, 0);
      const heading = dialog.getByRole('heading', {
        name: 'Исключить ложные срабатывания одной операцией',
      });
      expect(await heading.evaluate((element) => getComputedStyle(element).fontSize)).toBe('16px');
      expect(
        await heading.evaluate((element) => Number(getComputedStyle(element).fontWeight)),
      ).toBeGreaterThanOrEqual(600);
      await page.screenshot({
        path: `../../.local/privacy-controls-${width}-${theme}-${info.project.name}.png`,
      });
      await dialog.getByRole('button', { name: 'Ложное срабатывание', exact: true }).click();
      await expect(dialog).toContainText('Исключений: 1');
      await expect(dialog.getByRole('status')).toContainText('Совпадений: 0');
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
      await page.reload();
      await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
      await (await editorButton(page, 'Персональные данные')).click();
      await expect(dialog).toContainText('Исключений: 1');
      await page.keyboard.press('Escape');
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
