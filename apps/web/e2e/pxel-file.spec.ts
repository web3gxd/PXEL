import { selectValue } from './select';
import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { uuidV7 } from '../src/document/store';
import { GRID } from '../src/grid/viewport';
const secret = 'пароль файла 🔑 2026';
async function fixture(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toBeEnabled();
  return new URL(page.url()).searchParams.get('document')!;
}
async function write(page: Page, column: number, text: string) {
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + 0.5 * GRID.rowHeight,
    },
  });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill(text);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
}
test('password file UI round trip preserves formula style personal filter and creates fresh book', async ({
  page,
}) => {
  const id = await fixture(page);
  await write(page, 0, '7');
  await write(page, 1, '=A1*2');
  await page.getByRole('button', { name: 'Полужирный', exact: true }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const head = await (await page.request.get('/api/documents/' + id)).json(),
    s = head.sheets[0],
    origin = new URL(page.url()).origin;
  const view = {
    filters: [
      {
        id: uuidV7(),
        sheet_id: s.id,
        first_row_id: s.rows[0],
        last_row_id: s.rows[3],
        first_column_id: s.columns[0],
        last_column_id: s.columns[1],
        header_row_id: null,
        conditions: [
          { column_id: s.columns[0], predicate: { kind: 'number', operator: 'gt', value: 5 } },
        ],
      },
    ],
  };
  expect(
    (
      await page.request.put(`/api/documents/${id}/personal-view`, {
        headers: { Origin: origin },
        data: {
          schema_version: 1,
          expected_revision: 0,
          document_version: Number(head.version),
          settings: view,
        },
      })
    ).status(),
  ).toBe(200);
  await (await editorButton(page, 'Файл PXEL')).click();
  const dialog = page.getByRole('dialog', { name: 'Файл PXEL' });
  await dialog.getByLabel('Пароль файла', { exact: true }).fill(secret);
  await dialog.getByLabel('Повторите пароль').fill(secret);
  const downloadPromise = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Скачать .pxel', exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('PXEL.pxel');
  const path = await download.path();
  expect(path).toBeTruthy();
  await expect(dialog.getByRole('status')).toContainText('скачан');
  expect((await (await page.request.get('/api/documents/' + id)).json()).state_hash).toBe(
    head.state_hash,
  );
  await selectValue(dialog.getByLabel('Действие'), 'import');
  await dialog.getByLabel('Файл .pxel').setInputFiles(path!);
  expect(
    await dialog
      .getByLabel('Файл .pxel')
      .evaluate((input) => (input as HTMLInputElement).files?.[0]?.size),
  ).toBeGreaterThan(72);
  await dialog.getByLabel('Пароль файла', { exact: true }).fill(secret);
  await dialog.getByRole('button', { name: 'Проверить состав' }).click();
  const composition = dialog.getByRole('region', { name: 'Состав файла' });
  await expect(composition).toContainText('Формул: 1');
  await expect(composition).toContainText('Лист 1');
  await composition.getByRole('button', { name: 'Создать новую книгу из файла' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('document')).not.toBe(id);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const importedId = new URL(page.url()).searchParams.get('document')!,
    imported = await (await page.request.get('/api/documents/' + importedId)).json();
  expect(imported.sheets[0].id).not.toBe(s.id);
  expect(imported.sheets[0].rows[0]).not.toBe(s.rows[0]);
  expect(
    imported.sheets[0].cells.some(
      (c: { value: { type: string; value?: { current_text?: string } } }) =>
        c.value.type === 'formula' && c.value.value?.current_text === '=(A1*2)',
    ),
  ).toBe(true);
  expect(imported.sheets[0].styles[0].style.font_bold).toBe(true);
  const importedView = await (
    await page.request.get(`/api/documents/${importedId}/personal-view`)
  ).json();
  expect(importedView.settings.filters[0].conditions[0].predicate).toEqual(
    view.filters[0]!.conditions[0]!.predicate,
  );
  expect(importedView.settings.filters[0].sheet_id).not.toBe(s.id);
  await page.getByRole('region', { name: 'Область таблицы' }).press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toContainText('14');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
});
test('wrong password corruption version and format policy fail without a partial document', async ({
  page,
}) => {
  const id = await fixture(page);
  await write(page, 0, 'private-marker');
  const origin = new URL(page.url()).origin,
    head = await (await page.request.get('/api/documents/' + id)).json(),
    path = '/api/documents/' + id;
  const exported = await page.request.post(path + '/export.pxel', {
    headers: { Origin: origin },
    data: { schema_version: 1, password: secret, document_version: head.version },
  });
  expect(exported.status()).toBe(200);
  const bytes = await exported.body();
  expect(bytes.includes('private-marker')).toBe(false);
  const doc = uuidV7(),
    payload = {
      schema_version: 1,
      password: secret,
      file_base64: bytes.toString('base64'),
      document_id: doc,
      request_id: uuidV7(),
    };
  const failed = await page.request.post('/api/imports/pxel', {
    headers: { Origin: origin },
    data: { ...payload, password: 'wrong password 1234' },
  });
  expect(failed.status()).toBe(422);
  expect(await failed.json()).toEqual({ code: 'pxel_authentication' });
  expect((await page.request.get('/api/documents/' + doc)).status()).toBe(404);
  bytes[9] = 2;
  expect(
    (
      await page.request.post('/api/imports/pxel/preview', {
        headers: { Origin: origin },
        data: { ...payload, file_base64: bytes.toString('base64') },
      })
    ).status(),
  ).toBe(422);
  bytes[9] = 1;
  bytes[bytes.length - 1] ^= 1;
  expect(
    (
      await page.request.post('/api/imports/pxel', {
        headers: { Origin: origin },
        data: { ...payload, file_base64: bytes.toString('base64') },
      })
    ).status(),
  ).toBe(422);
  expect((await page.request.get('/api/documents/' + doc)).status()).toBe(404);
  const first = await page.request.post('/api/imports/pxel', {
    headers: { Origin: origin },
    data: payload,
  });
  expect(first.status()).toBe(201);
  const created = await first.json();
  expect(
    (
      await (
        await page.request.post('/api/imports/pxel', { headers: { Origin: origin }, data: payload })
      ).json()
    ).version,
  ).toBe(created.version);
  const policy = await (await page.request.get(path + '/disclosure-policy')).json();
  expect(
    (
      await page.request.put(path + '/disclosure-policy', {
        headers: { Origin: origin },
        data: {
          schema_version: 1,
          expected_revision: policy.revision,
          rules: { ...policy.rules, export_formats: ['xlsx'] },
        },
      })
    ).status(),
  ).toBe(200);
  expect(
    (
      await page.request.post(path + '/export.pxel', {
        headers: { Origin: origin },
        data: { schema_version: 1, password: secret, document_version: head.version },
      })
    ).status(),
  ).toBe(403);
  const audit = await (await page.request.get(path + '/audit')).json();
  expect(
    audit.events.some(
      (e: { format: string; allowed: boolean }) => e.format === 'pxel' && !e.allowed,
    ),
  ).toBe(true);
  expect(JSON.stringify(audit)).not.toContain(secret);
  expect((await (await page.request.get(path)).json()).state_hash).toBe(head.state_hash);
});
test('anonymous guest PXEL obeys both policies and imports only safe scalar values', async ({
  page,
  browser,
}) => {
  const id = await fixture(page);
  await write(page, 0, 'pxel-sensitive@example.test');
  await write(page, 1, '=LEN(A1)');
  const origin = new URL(page.url()).origin,
    path = '/api/documents/' + id,
    head = await (await page.request.get(path)).json();
  expect(
    (
      await page.request.put(path + '/privacy', {
        headers: { Origin: origin },
        data: {
          schema_version: 1,
          document_version: head.version,
          expected_revision: 0,
          rules: { enabled: true, types: ['email'], exceptions: [] },
        },
      })
    ).status(),
  ).toBe(200);
  const created = await page.request.post(path + '/shares', {
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
    guestPath = '/api/shared/' + link.share.link_id;
  const bookPolicy = await (await page.request.get(path + '/disclosure-policy')).json();
  expect(
    (
      await page.request.put(path + '/disclosure-policy', {
        headers: { Origin: origin },
        data: {
          schema_version: 2,
          expected_revision: bookPolicy.revision,
          rules: { ...bookPolicy.rules, allow_participant_export: true },
        },
      })
    ).status(),
  ).toBe(200);
  const linkPolicyPath = path + '/disclosure-policy?link_id=' + link.share.link_id,
    linkPolicy = await (await page.request.get(linkPolicyPath)).json();
  const guestContext = await browser.newContext();
  try {
    let guest = await guestContext.newPage();
    await guest.goto(origin + link.url);
    await guest.getByLabel('PIN или пароль').fill('482615');
    await guest.getByRole('button', { name: 'Открыть таблицу' }).click();
    await expect(guest.getByTestId('connection-status')).toHaveText('Подключено');
    const input = { schema_version: 1, password: secret, document_version: head.version };
    expect(
      (
        await guest.request.post(guestPath + '/export.pxel', {
          headers: { Origin: origin },
          data: input,
        })
      ).status(),
    ).toBe(403);
    expect(
      (
        await page.request.put(linkPolicyPath, {
          headers: { Origin: origin },
          data: {
            schema_version: 1,
            expected_revision: linkPolicy.revision,
            rules: {
              ...linkPolicy.rules,
              export_formats: ['pxel'],
              allow_participant_export: true,
            },
          },
        })
      ).status(),
    ).toBe(200);
    const ownerRequest = page.request;
    await page.close();
    await guest.close();
    guest = await guestContext.newPage();
    await guest.goto(origin + link.url);
    await guest.getByLabel('PIN или пароль').fill('482615');
    await guest.getByRole('button', { name: 'Открыть таблицу' }).click();
    await expect(guest.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(await editorButton(guest, 'Файл PXEL')).toBeVisible();
    await guest.keyboard.press('Escape');
    const exported = await guest.request.post(guestPath + '/export.pxel', {
      headers: { Origin: origin },
      data: input,
    });
    expect(exported.status()).toBe(200);
    const payload = {
      schema_version: 1,
      password: secret,
      file_base64: (await exported.body()).toString('base64'),
      document_id: uuidV7(),
      request_id: uuidV7(),
    };
    const preview = await ownerRequest.post('/api/imports/pxel/preview', {
      headers: { Origin: origin },
      data: payload,
    });
    expect(preview.status()).toBe(200);
    expect((await preview.json()).anonymized).toBe(true);
    expect(
      (
        await (
          await ownerRequest.post('/api/imports/pxel/preview', {
            headers: { Origin: origin },
            data: payload,
          })
        ).json()
      ).formulas,
    ).toBe(0);
    expect(
      (
        await ownerRequest.post('/api/imports/pxel', { headers: { Origin: origin }, data: payload })
      ).status(),
    ).toBe(201);
    const imported = await (await ownerRequest.get('/api/documents/' + payload.document_id)).json();
    expect(JSON.stringify(imported)).not.toContain('pxel-sensitive@example.test');
    expect(
      imported.sheets[0].cells.every(
        (c: { value: { type: string } }) => c.value.type !== 'formula',
      ),
    ).toBe(true);
    expect(JSON.stringify(imported)).toContain('ЗАВИСИМОСТЬ');
    expect(JSON.stringify(await (await ownerRequest.get(path)).json())).toContain(
      'pxel-sensitive@example.test',
    );
    const currentPolicy = await (await ownerRequest.get(path + '/disclosure-policy')).json();
    expect(
      (
        await ownerRequest.put(path + '/disclosure-policy', {
          headers: { Origin: origin },
          data: {
            schema_version: 2,
            expected_revision: currentPolicy.revision,
            rules: { ...currentPolicy.rules, allow_copy: false },
          },
        })
      ).status(),
    ).toBe(200);
    await expect(guest.getByRole('heading', { name: 'Доступ по ссылке закрыт' })).toBeVisible();
  } finally {
    await guestContext.close();
  }
});
for (const width of [390, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`PXEL file keyboard ${width} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await fixture(page);
      const opener = editorTrigger(page, 'Файл PXEL');
      await (await editorButton(page, 'Файл PXEL')).click();
      const dialog = page.getByRole('dialog', { name: 'Файл PXEL' });
      await expect(dialog).toBeVisible();
      const box = await dialog.boundingBox();
      expect(box!.width).toBeLessThanOrEqual(width);
      await page.keyboard.press('Tab');
      expect(await dialog.evaluate((el) => el.contains(window.document.activeElement))).toBe(true);
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
      await expect(opener).toBeFocused();
    });
