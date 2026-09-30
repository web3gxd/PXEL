import { expect, test, type Browser, type Page } from '@playwright/test';
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import {
  ClientEnvelopeSchema,
  ServerEnvelopeSchema,
} from '../src/generated/pxel/wire/v1/session_pb';
import { GRID, columnLabel } from '../src/grid/viewport';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const active = (page: Page) => page.getByTestId('active-cell');
const bytes = (id: string) =>
  Uint8Array.from(id.replaceAll('-', '').match(/../g) ?? [], (part) => parseInt(part, 16));
function uuidV7() {
  const id = crypto.getRandomValues(new Uint8Array(16));
  let timestamp = BigInt(Date.now());
  for (let i = 5; i >= 0; i--) {
    id[i] = Number(timestamp & 255n);
    timestamp >>= 8n;
  }
  id[6] = 0x70 | ((id[6] ?? 0) & 15);
  id[8] = 0x80 | ((id[8] ?? 0) & 63);
  const hex = [...id].map((b) => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
async function createDocument(page: Page) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
  await expect(active(page)).toContainText(`${columnLabel(column)}${row + 1}:`);
}
async function write(page: Page, row: number, column: number, value: string) {
  await select(page, row, column);
  await bar(page).fill(value);
  await bar(page).press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function state(page: Page) {
  const id = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  return response.json();
}
async function move(page: Page, from: [number, number], to: [number, number], columns = 1) {
  await select(page, ...from);
  for (let i = 1; i < columns; i++) await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('ControlOrMeta+X');
  await expect(page.getByTestId('cut-status')).toContainText('Исходные ячейки пока сохранены');
  await select(page, ...to);
  await page.keyboard.press('ControlOrMeta+V');
  await expect(page.getByTestId('cut-status')).toHaveCount(0);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function foreignEditor(page: Page, browser: Browser) {
  const origin = new URL(page.url()).origin;
  const id = new URL(page.url()).searchParams.get('document');
  const owner = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] });
  const account = `move_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
  try {
    expect(
      (
        await context.request.post(origin + '/api/dev/session', {
          headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': account },
        })
      ).status(),
    ).toBe(204);
    const identity = await (
      await context.request.get(origin + '/api/dev/identity', {
        headers: { Origin: origin },
      })
    ).json();
    await context.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    expect(
      (
        await page.request.post(`/api/documents/${id}/permissions`, {
          headers: { Origin: origin },
          data: {
            action: 'grant',
            change_id: uuidV7(),
            target_user_id: identity.actor_id,
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
    return {
      editor,
      close: async () => {
        await context.request
          .delete(origin + '/api/dev/session', { headers: { Origin: origin } })
          .catch(() => undefined);
        await context.close();
      },
    };
  } catch (error) {
    await context.request
      .delete(origin + '/api/dev/session', { headers: { Origin: origin } })
      .catch(() => undefined);
    await context.close();
    throw error;
  }
}

test('own overlapping move undo and redo restore overwritten content and external formulas in both tabs', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await createDocument(page);
  await write(page, 0, 0, '3');
  await write(page, 0, 1, '=A1*2');
  await write(page, 0, 2, '=10');
  await write(page, 0, 3, '=B1+1');
  const original = await state(page);
  const second = await page.context().newPage();
  second.on('pageerror', (error) => errors.push(error.message));
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await page.bringToFront();
    await move(page, [0, 0], [0, 1], 2);
    const moved = await state(page);
    expect(BigInt(moved.version)).toBe(BigInt(original.version) + 1n);
    await select(page, 0, 3);
    await expect(active(page)).toHaveText('D1: 7');
    await expect(bar(page)).toHaveValue('=(C1+1)');
    await page.keyboard.press('ControlOrMeta+Z');
    await expect
      .poll(async () => (await state(page)).version)
      .toBe(String(BigInt(moved.version) + 1n));
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    const undone = await state(page);
    expect(undone.sheets).toEqual(original.sheets);
    // The canonical hash includes document version; inverses restore content
    // while committing a new version, so their hashes must also differ.
    expect(undone.state_hash).not.toBe(original.state_hash);
    await expect(bar(page)).toHaveValue('=B1+1');
    await second.bringToFront();
    await select(second, 0, 2);
    await expect(active(second)).toHaveText('C1: 10');
    await page.bringToFront();
    await grid(page).focus();
    await page.keyboard.press('ControlOrMeta+Y');
    await expect
      .poll(async () => (await state(page)).version)
      .toBe(String(BigInt(undone.version) + 1n));
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    const redone = await state(page);
    expect(redone.sheets).toEqual(moved.sheets);
    expect(redone.state_hash).not.toBe(moved.state_hash);
    await expect(bar(page)).toHaveValue('=(C1+1)');
    await expect(active(second)).toHaveText('C1: 6');
    await expect(page.getByRole('button', { name: 'Повторить отменённую правку' })).toBeDisabled();
    await page.reload();
    await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
    expect(await state(page)).toEqual(redone);
    expect(errors).toEqual([]);
  } finally {
    await second.close();
  }
});

for (const conflict of [
  'equal destination value',
  'new direct reference',
  'new interior range reference',
] as const) {
  test(`move undo refuses a foreign ${conflict} without changing the document`, async ({
    page,
    browser,
  }) => {
    const rejections: number[] = [];
    page.on('websocket', (socket) =>
      socket.on('framereceived', ({ payload }) => {
        if (typeof payload === 'string') return;
        const message = fromBinary(ServerEnvelopeSchema, new Uint8Array(payload));
        if (message.payload.case === 'rejected') rejections.push(message.payload.value.code);
      }),
    );
    await createDocument(page);
    const { editor, close } = await foreignEditor(page, browser);
    try {
      await page.bringToFront();
      await write(page, 1, 1, '7');
      await move(page, [1, 1], [1, 3]);
      await editor.bringToFront();
      await select(editor, 1, 3);
      await expect(active(editor)).toHaveText('D2: 7');
      if (conflict === 'equal destination value') await write(editor, 1, 3, '7');
      else await write(editor, 0, 7, conflict === 'new direct reference' ? '=D2+1' : '=SUM(C1:E3)');
      const before = await state(page);
      await page.bringToFront();
      await grid(page).focus();
      await page.keyboard.press('ControlOrMeta+Z');
      await expect(page.getByRole('alert')).toContainText('Отмена невозможна');
      expect(rejections).toContain(13);
      const after = await state(page);
      expect(after).toEqual(before);
      await select(page, 1, 1);
      await expect(active(page)).toHaveText('B2: ');
      await select(page, 1, 3);
      await expect(active(page)).toHaveText('D2: 7');
    } finally {
      await close();
    }
  });
}

test('a document owner cannot use an inverse command to undo another actor move', async ({
  page,
  browser,
}) => {
  await createDocument(page);
  const { editor, close } = await foreignEditor(page, browser);
  try {
    let target: Uint8Array | undefined;
    // Reconnect the editor once so the test sees its real outgoing move identity.
    editor.on('websocket', (socket) =>
      socket.on('framesent', ({ payload }) => {
        if (typeof payload === 'string') return;
        const envelope = fromBinary(ClientEnvelopeSchema, new Uint8Array(payload));
        if (envelope.payload.case === 'command' && envelope.payload.value.moveRange)
          target = envelope.payload.value.operationId;
      }),
    );
    await editor.reload();
    await expect(editor.getByTestId('connection-status')).toHaveText('Подключено');
    await editor.bringToFront();
    await write(editor, 1, 1, '7');
    await move(editor, [1, 1], [1, 3]);
    expect(target).toBeDefined();
    if (!target) throw new Error('The foreign move command was not captured');
    await expect(page.getByRole('button', { name: 'Отменить последнюю правку' })).toBeDisabled();
    const before = await state(page);
    const origin = new URL(page.url()).origin;
    const owner = await (
      await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    const hello = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'hello',
          value: { clientInstanceId: bytes(uuidV7()), clientName: 'move-history-test' },
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
            actorId: bytes(owner.actor_id),
            operationId: bytes(uuidV7()),
            baseVersion: BigInt(before.version),
            clientSequence: 1n,
            undoCell: { targetOperationId: target },
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
            reject(new Error('Inverse response timed out'));
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
            reject(new Error('Test connection failed'));
          };
        }),
      { helloBytes: [...hello], commandBytes: [...command] },
    );
    const result = fromBinary(ServerEnvelopeSchema, new Uint8Array(response));
    expect(result.payload.case).toBe('rejected');
    if (result.payload.case !== 'rejected') throw new Error('Missing typed rejection');
    expect(result.payload.value.code).toBe(14);
    expect(await state(page)).toEqual(before);
  } finally {
    await close();
  }
});
