import { expect, test, type Page } from '@playwright/test';
import { uuidV7 } from '../src/document/store';
import { selectValue } from './select';

const faults = new WeakMap<Page, string[]>();
test.beforeEach(async ({ page, context }) => {
  await context.clearCookies();
  const errors: string[] = [];
  faults.set(page, errors);
  page.on('pageerror', (e) => errors.push(e.message));
  expect(
    (
      await page.request.post('/api/dev/session', {
        headers: {
          Origin: new URL(String(test.info().project.use.baseURL)).origin,
          'X-PXEL-DEV-ACCOUNT':
            'csv_export_' + crypto.randomUUID().replaceAll('-', '').slice(0, 16),
        },
      })
    ).status(),
  ).toBe(204);
});
test.afterEach(async ({ page }) => {
  await page.request.delete('/api/dev/session', {
    headers: { Origin: new URL(String(test.info().project.use.baseURL)).origin },
  });
  expect(faults.get(page)).toEqual([]);
});
async function fixture(page: Page) {
  const payload = {
    schema_version: 1,
    file_base64: btoa('001,"a, ""b"""\r\n=SUM(A1),true\r\n'),
    title: 'CSV export',
    delimiter: 'comma',
    column_types: [],
    header: false,
    document_id: uuidV7(),
    request_id: uuidV7(),
  };
  const headers = { Origin: new URL(String(test.info().project.use.baseURL)).origin };
  const preview = await (
    await page.request.post('/api/imports/csv/preview', { headers, data: payload })
  ).json();
  expect(
    (
      await page.request.post('/api/imports/csv', {
        headers,
        data: { ...payload, preview_digest: preview.preview_digest },
      })
    ).status(),
  ).toBe(201);
  await page.goto('/?document=' + payload.document_id);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  return payload.document_id;
}
async function write(page: Page, cell: string, value: string) {
  const address = page.getByRole('textbox', { name: 'Ячейка или диапазон', exact: true });
  await address.fill(cell);
  await address.press('Enter');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки', exact: true });
  await bar.fill(value);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
test('selected sheet CSV downloads confirmed formula values and reimports literal text', async ({
  page,
}) => {
  const id = await fixture(page);
  await write(page, 'A3', '3');
  await write(page, 'B3', '=A3*2');
  const head = await (await page.request.get('/api/documents/' + id)).json();
  await page.getByRole('button', { name: 'Файл', exact: true }).click();
  await page.getByTestId('export-csv').click();
  const panel = page.getByRole('dialog', { name: 'Экспорт CSV', exact: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: test.info().outputPath('csv-export.png') });
  await expect(panel.getByLabel('Экранировать текст, похожий на формулу')).toBeChecked();
  const firstResponse = page.waitForResponse((r) =>
    new URL(r.url()).pathname.endsWith('/export.csv'),
  );
  const firstDownload = page.waitForEvent('download');
  await panel.getByRole('button', { name: 'Скачать CSV', exact: true }).click();
  const first = await firstResponse;
  expect((await firstDownload).suggestedFilename()).toBe('PXEL.csv');
  expect((await first.text()).replace(/^\uFEFF/, '')).toBe(
    '"001","a, ""b"""\r\n"\'=SUM(A1)","true"\r\n"3","6"\r\n',
  );
  await panel.getByLabel('Экранировать текст, похожий на формулу').uncheck();
  await selectValue(panel.getByLabel('Разделитель', { exact: true }), 'semicolon');
  const downloadPromise = page.waitForEvent('download');
  await panel.getByRole('button', { name: 'Скачать CSV', exact: true }).click();
  const downloaded = await downloadPromise,
    path = await downloaded.path();
  expect(path).toBeTruthy();
  await expect(panel.getByRole('status')).toHaveText('CSV скачан.');
  expect((await (await page.request.get('/api/documents/' + id)).json()).state_hash).toBe(
    head.state_hash,
  );
  await panel.getByRole('button', { name: 'Закрыть' }).click();
  await page.goto('/');
  await page.getByRole('button', { name: 'Импорт CSV', exact: true }).click();
  const imported = page.getByRole('dialog', { name: 'Импорт CSV', exact: true });
  if (!path) throw new Error('download path unavailable');
  await imported.getByLabel('CSV-файл', { exact: true }).setInputFiles(path);
  await selectValue(imported.getByLabel('Разделитель', { exact: true }), 'semicolon');
  await imported.getByRole('button', { name: 'Предпросмотр', exact: true }).click();
  await imported.getByRole('button', { name: 'Создать книгу', exact: true }).click();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const nextId = new URL(page.url()).searchParams.get('document');
  expect(nextId).not.toBe(id);
  const next = await (await page.request.get('/api/documents/' + nextId)).json();
  const values = next.sheets[0].cells.map((c: { value: unknown }) => c.value);
  for (const value of ['001', 'a, "b"', '=SUM(A1)', 'true', '3', '6'])
    expect(values).toContainEqual({ type: 'text', value });
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  expect((await (await page.request.get('/api/documents/' + nextId)).json()).state_hash).toBe(
    next.state_hash,
  );
});
test('required CSV anonymization masks sensitive values and their formula dependents', async ({
  page,
}) => {
  const id = await fixture(page),
    path = '/api/documents/' + id;
  await write(page, 'A3', 'alice@example.com');
  await write(page, 'B3', '=LEN(A3)');
  const head = await (await page.request.get(path)).json();
  const headers = { Origin: new URL(String(test.info().project.use.baseURL)).origin };
  const privacy = await (await page.request.get(path + '/privacy')).json();
  expect(
    (
      await page.request.put(path + '/privacy', {
        headers,
        data: {
          schema_version: 1,
          document_version: head.version,
          expected_revision: privacy.revision,
          rules: { enabled: true, types: ['email'], exceptions: [] },
        },
      })
    ).status(),
  ).toBe(200);
  const policy = await (await page.request.get(path + '/disclosure-policy')).json();
  expect(
    (
      await page.request.put(path + '/disclosure-policy', {
        headers,
        data: {
          schema_version: 2,
          expected_revision: policy.revision,
          rules: { ...policy.rules, require_anonymization: true },
        },
      })
    ).status(),
  ).toBe(200);
  const query = new URLSearchParams({
    schema_version: '1',
    sheet_id: head.sheets[0].id,
    document_version: head.version,
    safe_text: 'false',
  });
  const response = await page.request.get(path + '/export.csv?' + query);
  expect(response.status()).toBe(200);
  const text = await response.text();
  expect(text).not.toContain('alice@example.com');
  expect(text).not.toContain('=LEN');
  expect(text).toContain('[СКРЫТО: ЗАВИСИМОСТЬ]');
  expect((await (await page.request.get(path)).json()).state_hash).toBe(head.state_hash);
});
test('CSV guest export requires both policies and revocation closes its endpoint', async ({
  page,
  browser,
}) => {
  const id = await fixture(page),
    path = '/api/documents/' + id;
  const head = await (await page.request.get(path)).json();
  const origin = new URL(String(test.info().project.use.baseURL)).origin,
    headers = { Origin: origin };
  const created = await page.request.post(path + '/shares', {
    headers,
    data: {
      request_id: uuidV7(),
      permission: 'view',
      ttl: 'hour',
      confirm_indefinite: false,
      pin: '482615',
    },
  });
  expect(created.status()).toBe(201);
  const link = await created.json(),
    sharedPath = origin + '/api/shared/' + link.share.link_id;
  const token = new URL(origin + link.url).hash.slice(1);
  const guest = await browser.newContext();
  try {
    const unlock = () =>
      guest.request.post(sharedPath + '/session', { headers, data: { token, secret: '482615' } });
    expect((await unlock()).status()).toBe(200);
    const query = new URLSearchParams({
      schema_version: '1',
      sheet_id: head.sheets[0].id,
      document_version: head.version,
    });
    expect((await guest.request.get(sharedPath + '/export.csv?' + query)).status()).toBe(403);
    const bookPolicy = await (await page.request.get(path + '/disclosure-policy')).json();
    expect(
      (
        await page.request.put(path + '/disclosure-policy', {
          headers,
          data: {
            schema_version: 2,
            expected_revision: bookPolicy.revision,
            rules: { ...bookPolicy.rules, allow_participant_export: true },
          },
        })
      ).status(),
    ).toBe(200);
    expect((await unlock()).status()).toBe(200);
    expect((await guest.request.get(sharedPath + '/export.csv?' + query)).status()).toBe(403);
    const linkPolicyPath = path + '/disclosure-policy?link_id=' + link.share.link_id;
    const policy = await (await page.request.get(linkPolicyPath)).json();
    expect(
      (
        await page.request.put(linkPolicyPath, {
          headers,
          data: {
            schema_version: 2,
            expected_revision: policy.revision,
            rules: { ...policy.rules, export_formats: ['csv'], allow_participant_export: true },
          },
        })
      ).status(),
    ).toBe(200);
    expect((await unlock()).status()).toBe(200);
    const response = await guest.request.get(sharedPath + '/export.csv?' + query);
    expect(response.status()).toBe(200);
    expect(await response.text()).toContain('001');
    expect((await guest.request.get(origin + path + '/export.csv?' + query)).status()).not.toBe(
      200,
    );
    expect(
      (
        await page.request.delete(path + '/shares/' + link.share.link_id, {
          headers,
          data: { request_id: uuidV7() },
        })
      ).status(),
    ).toBe(204);
    expect((await guest.request.get(sharedPath + '/export.csv?' + query)).status()).not.toBe(200);
  } finally {
    await guest.close();
  }
});
test('sheet selection exports the requested stable sheet without changing the book', async ({
  page,
}) => {
  const id = await fixture(page),
    path = '/api/documents/' + id;
  await page.getByRole('button', { name: 'Добавить лист', exact: true }).click();
  await expect
    .poll(async () => (await (await page.request.get(path)).json()).sheets.length)
    .toBe(2);
  let head = await (await page.request.get(path)).json();
  await page
    .getByRole('button', { name: 'Перейти на лист ' + head.sheets[1].name, exact: true })
    .click();
  await write(page, 'A1', 'other sheet');
  head = await (await page.request.get(path)).json();
  await page.getByRole('button', { name: 'Файл', exact: true }).click();
  await page.getByTestId('export-csv').click();
  const panel = page.getByRole('dialog', { name: 'Экспорт CSV', exact: true });
  for (const [sheet, expected, omitted] of [
    [head.sheets[0], '001', 'other sheet'],
    [head.sheets[1], 'other sheet', '001'],
  ]) {
    await selectValue(panel.getByLabel('Лист', { exact: true }), sheet.id);
    const response = page.waitForResponse((r) => new URL(r.url()).pathname.endsWith('/export.csv'));
    const downloaded = page.waitForEvent('download');
    await panel.getByRole('button', { name: 'Скачать CSV', exact: true }).click();
    const text = await (await response).text();
    await downloaded;
    expect(text).toContain(expected);
    expect(text).not.toContain(omitted);
    await expect(panel.getByRole('button', { name: 'Скачать CSV', exact: true })).toBeEnabled();
  }
  expect((await (await page.request.get(path)).json()).state_hash).toBe(head.state_hash);
});
test('CSV policy and exact-head guards cannot be bypassed by a direct request', async ({
  page,
}) => {
  const id = await fixture(page),
    path = '/api/documents/' + id;
  const head = await (await page.request.get(path)).json();
  const query = new URLSearchParams({
    schema_version: '1',
    sheet_id: head.sheets[0].id,
    document_version: head.version,
  });
  const stale = new URLSearchParams(query);
  stale.set('document_version', '0');
  expect((await page.request.get(path + '/export.csv?' + stale)).status()).toBe(409);
  const unknown = new URLSearchParams(query);
  unknown.set('schema_version', '2');
  expect((await page.request.get(path + '/export.csv?' + unknown)).status()).toBe(400);
  const foreign = new URLSearchParams(query);
  foreign.set('sheet_id', uuidV7());
  expect((await page.request.get(path + '/export.csv?' + foreign)).status()).toBe(422);
  const failedPreparation = await (await page.request.get(path + '/audit')).json();
  expect(failedPreparation.events).toContainEqual(
    expect.objectContaining({
      action: 'export',
      format: 'csv',
      allowed: false,
      reason: 'preparation_failed',
    }),
  );
  const policy = await (await page.request.get(path + '/disclosure-policy')).json();
  const headers = { Origin: new URL(String(test.info().project.use.baseURL)).origin };
  const rules = { ...policy.rules, export_formats: ['csv'] };
  expect(
    (
      await page.request.put(path + '/disclosure-policy', {
        headers,
        data: { schema_version: 1, expected_revision: policy.revision, rules },
      })
    ).status(),
  ).toBe(400);
  expect(
    (
      await page.request.put(path + '/disclosure-policy', {
        headers,
        data: {
          schema_version: 2,
          expected_revision: policy.revision,
          rules: { ...rules, export_formats: ['xlsx', 'pxel'] },
        },
      })
    ).status(),
  ).toBe(200);
  expect((await page.request.get(path + '/export.csv?' + query)).status()).toBe(403);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Файл', exact: true }).click();
  await expect(page.getByTestId('export-csv')).toHaveCount(0);
  const audit = await (await page.request.get(path + '/audit')).json();
  expect(audit.schema_version).toBe(2);
  expect(audit.events).toContainEqual(
    expect.objectContaining({
      action: 'export',
      format: 'csv',
      allowed: false,
      reason: 'format_denied',
    }),
  );
});
