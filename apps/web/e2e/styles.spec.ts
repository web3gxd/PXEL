import { expect, test, type Browser, type Page } from '@playwright/test';
import { create, fromBinary, toBinary, type MessageInitShape } from '@bufbuild/protobuf';
import {
  ClientEnvelopeSchema,
  DocumentCommandSchema,
  ServerEnvelopeSchema,
} from '../src/generated/pxel/wire/v1/session_pb';
import { uuidV7, type Projection } from '../src/document/store';
import { encodeStylePatch } from '../src/document/styles';
import { GRID } from '../src/grid/viewport';

const browserFailures = new WeakMap<Page, string[]>();
test.beforeEach(async ({ page }) => {
  const failures: string[] = [];
  browserFailures.set(page, failures);
  page.on('pageerror', () => failures.push('uncaught page error'));
  page.on('console', (message) => {
    if (message.type() === 'error') failures.push('browser console error');
  });
  page.on('response', (response) => {
    if (response.status() >= 500)
      failures.push(`HTTP ${response.status()} ${new URL(response.url()).pathname}`);
  });
});
test.afterEach(async ({ page }) => {
  expect(browserFailures.get(page)).toEqual([]);
});

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const undo = (page: Page) => page.getByRole('button', { name: 'Отменить последнюю правку' });
const redo = (page: Page) => page.getByRole('button', { name: 'Повторить отменённую правку' });
const bytes = (id: string) =>
  Uint8Array.from(id.replaceAll('-', '').match(/../g) ?? [], (part) => parseInt(part, 16));
async function state(page: Page): Promise<Projection> {
  const document = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${document}`);
  expect(response.ok()).toBe(true);
  return response.json();
}
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
}
async function committed(page: Page, action: () => Promise<unknown>) {
  const before = await state(page);
  await action();
  await expect
    .poll(async () => (await state(page)).version)
    .toBe(String(BigInt(before.version) + 1n));
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  return state(page);
}
async function workerMatches(page: Page) {
  const confirmed = await state(page);
  const canvas = page.locator('canvas');
  await expect(canvas).toHaveAttribute('data-confirmed-state-hash', confirmed.state_hash);
  await expect(canvas).toHaveAttribute('data-preview-authored-hash', confirmed.state_hash);
}
async function createDocument(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByRole('button', { name: 'Полужирный', exact: true })).toBeEnabled();
}

test('styled export returns a typed refusal instead of dropping authored formatting', async ({
  page,
}) => {
  await createDocument(page);
  await select(page, 0, 0);
  const styled = await committed(page, () =>
    page.getByRole('button', { name: 'Полужирный' }).click(),
  );
  expect(styled.sheets[0]?.styles).toHaveLength(1);
  const documentId = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${documentId}/export.xlsx`);
  expect(response.status()).toBe(422);
  expect(await response.json()).toEqual({ code: 'xlsx_styles_not_supported' });
  expect(await state(page)).toEqual(styled);
});

async function reload(page: Page) {
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByRole('button', { name: 'Полужирный', exact: true })).toBeEnabled();
  await workerMatches(page);
}
async function write(page: Page, value: string) {
  return committed(page, async () => {
    const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await bar.fill(value);
    await bar.press('Enter');
  });
}
function styleAt(projection: Projection, row: string, column: string) {
  return projection.sheets[0]?.styles?.find(
    (cell) => cell.row_id === row && cell.column_id === column,
  )?.style;
}
async function rawCommand(page: Page, fields: MessageInitShape<typeof DocumentCommandSchema>) {
  const before = await state(page),
    origin = new URL(page.url()).origin;
  const identity = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const hello = toBinary(
    ClientEnvelopeSchema,
    create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'hello',
        value: { clientInstanceId: bytes(uuidV7()), clientName: 'styles-browser-test' },
      },
    }),
  );
  const command = toBinary(
    ClientEnvelopeSchema,
    create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          documentId: bytes(before.document_id),
          actorId: bytes(identity.actor_id),
          operationId: bytes(uuidV7()),
          baseVersion: BigInt(before.version),
          clientSequence: 1n,
          ...fields,
        },
      },
    }),
  );
  const response = await page.evaluate(
    ({ helloBytes, commandBytes }) =>
      new Promise<number[]>((resolve, reject) => {
        const url = new URL('/api/ws', location.href);
        url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const socket = new WebSocket(url);
        socket.binaryType = 'arraybuffer';
        let welcomed = false;
        const timeout = setTimeout(() => {
          socket.close();
          reject(new Error('Style command timed out'));
        }, 10000);
        socket.onopen = () => socket.send(new Uint8Array(helloBytes));
        socket.onmessage = (event: MessageEvent<ArrayBuffer>) => {
          if (!welcomed) {
            welcomed = true;
            socket.send(new Uint8Array(commandBytes));
            return;
          }
          clearTimeout(timeout);
          resolve([...new Uint8Array(event.data)]);
          socket.close();
        };
        socket.onerror = () => {
          clearTimeout(timeout);
          socket.close();
          reject(new Error('Style connection failed'));
        };
      }),
    { helloBytes: [...hello], commandBytes: [...command] },
  );
  return fromBinary(ServerEnvelopeSchema, new Uint8Array(response));
}
async function history(page: Page) {
  const document = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${document}/undo-history`);
  expect(response.ok()).toBe(true);
  return response.json() as Promise<{
    undo: { operation_id: string }[];
    redo: { operation_id: string }[];
  }>;
}
async function editorFor(page: Page, browser: Browser) {
  const origin = new URL(page.url()).origin,
    document = new URL(page.url()).searchParams.get('document');
  const owner = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const context = await browser.newContext({
    deviceScaleFactor: await page.evaluate(() => devicePixelRatio),
  });
  const account = `styles_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
  const close = async () => {
    await context.request
      .delete(origin + '/api/dev/session', { headers: { Origin: origin } })
      .catch(() => undefined);
    await context.close();
  };
  try {
    expect(
      (
        await context.request.post(origin + '/api/dev/session', {
          headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': account },
        })
      ).status(),
    ).toBe(204);
    const actor = await (
      await context.request.get(origin + '/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    await context.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    expect(
      (
        await page.request.post(`/api/documents/${document}/permissions`, {
          headers: { Origin: origin },
          data: {
            action: 'grant',
            change_id: uuidV7(),
            target_user_id: actor.actor_id,
            role: 'editor',
          },
        })
      ).status(),
    ).toBe(200);
    expect(
      (
        await context.request.post(origin + '/api/dev/session', {
          headers: {
            Origin: origin,
            'X-PXEL-DEV-ACCOUNT': account,
            'X-PXEL-DEV-WORKSPACE': owner.workspace_id,
          },
        })
      ).status(),
    ).toBe(204);
    const editor = await context.newPage();
    await editor.goto(page.url());
    await expect(editor.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(editor.getByRole('button', { name: 'Полужирный', exact: true })).toBeEnabled();
    return { editor, close };
  } catch (error) {
    await close();
    throw error;
  }
}

test('styled blank renders real Canvas fill and survives reload Undo Redo and content clear', async ({
  page,
}, testInfo) => {
  await createDocument(page);
  await select(page, 0, 0);
  const styled = await committed(page, () =>
    page.getByLabel('Цвет заливки', { exact: true }).fill('#0c2238'),
  );
  expect(styled.sheets[0]?.cells).toEqual([]);
  const row = styled.sheets[0]!.rows[0]!,
    column = styled.sheets[0]!.columns[0]!;
  expect(styleAt(styled, row, column)?.fill).toEqual([12, 34, 56]);
  await select(page, 0, 1);
  const pixel = () =>
    page.locator('canvas').evaluate(
      (canvas: HTMLCanvasElement, location) => {
        const rgba = canvas
          .getContext('2d')
          ?.getImageData(
            Math.floor(location.x * devicePixelRatio),
            Math.floor(location.y * devicePixelRatio),
            1,
            1,
          ).data;
        return rgba ? Array.from(rgba) : [];
      },
      { x: GRID.rowHeaderWidth + 10, y: GRID.columnHeaderHeight + 10 },
    );
  await expect.poll(pixel).toEqual([12, 34, 56, 255]);
  await testInfo.attach('styled-blank-canvas', {
    body: await page.locator('canvas').screenshot(),
    contentType: 'image/png',
  });
  await workerMatches(page);
  await reload(page);
  await expect(undo(page)).toBeEnabled();
  const undone = await committed(page, () => undo(page).click());
  expect(undone.sheets[0]?.styles).toEqual([]);
  await reload(page);
  await expect(redo(page)).toBeEnabled();
  const redone = await committed(page, () => redo(page).click());
  expect(redone.sheets[0]?.styles).toEqual(styled.sheets[0]?.styles);
  await select(page, 0, 0);
  await write(page, '7.25');
  await select(page, 0, 0);
  const formatted = await committed(page, () =>
    page.getByLabel('Числовой формат', { exact: true }).selectOption('percent2'),
  );
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 725.00%');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('7.25');
  await committed(page, () => grid(page).press('Delete'));
  const cleared = await state(page);
  expect(cleared.sheets[0]?.cells).toEqual([]);
  expect(cleared.sheets[0]?.styles).toEqual(formatted.sheets[0]?.styles);
  await reload(page);
  expect((await state(page)).sheets).toEqual(cleared.sheets);
  if (testInfo.project.name === 'chromium-dpr1') {
    await select(page, 0, 1);
    await write(page, 'Оформление PXEL');
    for (const [name, width] of [
      ['desktop', 1440],
      ['mobile', 390],
    ] as const) {
      await page.setViewportSize({ width, height: 900 });
      await workerMatches(page);
      await page.screenshot({ path: `../../.local/styles-${name}.png`, fullPage: true });
    }
  }
});

test('frozen range styling keeps inserted row untouched through non-top Undo and Redo', async ({
  page,
}) => {
  await createDocument(page);
  await select(page, 0, 0);
  await grid(page).press('Shift+ArrowDown');
  const styled = await committed(page, () =>
    page.getByRole('button', { name: 'Полужирный', exact: true }).click(),
  );
  expect(styled.sheets[0]?.styles).toHaveLength(2);
  const target = (await history(page)).undo.at(-1)?.operation_id;
  expect(target).toBeTruthy();
  const originalRows = styled.sheets[0]!.rows.slice(0, 2),
    column = styled.sheets[0]!.columns[0]!;
  await select(page, 1, 0);
  const inserted = await committed(page, () =>
    page.getByRole('button', { name: 'Строка ↑', exact: true }).click(),
  );
  const middle = inserted.sheets[0]!.rows[1]!;
  expect(middle).not.toBe(originalRows[1]);
  const beforeUndo = inserted.version;
  expect(
    (await rawCommand(page, { undoCell: { targetOperationId: bytes(target!) } })).payload.case,
  ).toBe('committed');
  await expect.poll(async () => (await state(page)).version).toBe(String(BigInt(beforeUndo) + 1n));
  expect((await state(page)).sheets[0]?.styles).toEqual([]);
  const compensation = (await history(page)).redo.at(-1)?.operation_id;
  expect(compensation).toBeTruthy();
  expect(
    (await rawCommand(page, { undoCell: { targetOperationId: bytes(compensation!) } })).payload
      .case,
  ).toBe('committed');
  const redone = await state(page);
  expect(redone.sheets[0]?.styles).toEqual(styled.sheets[0]?.styles);
  expect(styleAt(redone, middle, column)).toBeUndefined();
  for (const row of originalRows) expect(styleAt(redone, row!, column)?.font_bold).toBe(true);
  await reload(page);
  expect((await state(page)).sheets).toEqual(redone.sheets);
});

test('distinct editors concurrently merge independent leaves without changing content', async ({
  page,
  browser,
}) => {
  await createDocument(page);
  await select(page, 0, 0);
  await write(page, '42');
  await select(page, 0, 0);
  const before = await state(page),
    { editor, close } = await editorFor(page, browser);
  try {
    await select(editor, 0, 0);
    await Promise.all([
      page.getByRole('button', { name: 'Полужирный', exact: true }).click(),
      editor.getByRole('button', { name: 'Курсив', exact: true }).click(),
    ]);
    await expect
      .poll(async () => (await state(page)).version)
      .toBe(String(BigInt(before.version) + 2n));
    for (const current of [page, editor]) {
      await expect(
        current.getByRole('button', { name: 'Полужирный', exact: true }),
      ).toHaveAttribute('aria-pressed', 'true');
      await expect(current.getByRole('button', { name: 'Курсив', exact: true })).toHaveAttribute(
        'aria-pressed',
        'true',
      );
      await expect(current.getByTestId('save-status')).toHaveText('Сохранено');
      await workerMatches(current);
    }
    const merged = await state(page);
    expect(merged.sheets[0]?.cells).toEqual(before.sheets[0]?.cells);
    await committed(page, () => undo(page).click());
    await expect(editor.getByRole('button', { name: 'Полужирный', exact: true })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    await expect(editor.getByRole('button', { name: 'Курсив', exact: true })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect((await state(page)).sheets[0]?.cells).toEqual(before.sheets[0]?.cells);
    await reload(page);
    await reload(editor);
  } finally {
    await close();
  }
});

test('foreign equal-value style write blocks owner inverse without a durable version', async ({
  page,
  browser,
}) => {
  await createDocument(page);
  await select(page, 0, 0);
  const styled = await committed(page, () =>
    page.getByRole('button', { name: 'Полужирный', exact: true }).click(),
  );
  const { editor, close } = await editorFor(page, browser);
  try {
    const sheet = styled.sheets[0]!;
    const result = await rawCommand(editor, {
      setCellStyle: {
        address: {
          sheetId: bytes(sheet.id),
          rowId: bytes(sheet.rows[0]!),
          columnId: bytes(sheet.columns[0]!),
        },
        patch: encodeStylePatch([{ property: 'font_bold', value: true }]),
      },
    });
    expect(result.payload.case).toBe('committed');
    const foreign = await state(page);
    expect(BigInt(foreign.version)).toBe(BigInt(styled.version) + 1n);
    expect(foreign.sheets).toEqual(styled.sheets);
    await reload(page);
    await expect(undo(page)).toBeEnabled();
    await undo(page).click();
    await expect(page.getByRole('alert')).toContainText('Отмена невозможна');
    expect(await state(page)).toEqual(foreign);
    await expect(undo(page)).toBeDisabled();
  } finally {
    await close();
  }
});
