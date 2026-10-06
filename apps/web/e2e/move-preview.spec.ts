import { createBookFromUi } from './create-book';
import { cutSelection } from './clipboard';
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import { fromBinary } from '@bufbuild/protobuf';
import {
  ClientEnvelopeSchema,
  ServerEnvelopeSchema,
} from '../src/generated/pxel/wire/v1/session_pb';
import { GRID, columnLabel } from '../src/grid/viewport';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const active = (page: Page) => page.getByTestId('active-cell');
const preview = (page: Page) => page.getByTestId('calculation-preview');
async function createDocument(page: Page) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
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
async function cutPaste(page: Page, from: [number, number], to: [number, number], columns = 1) {
  await select(page, ...from);
  for (let index = 1; index < columns; index++) await page.keyboard.press('Shift+ArrowRight');
  await cutSelection(page);
  await expect(page.getByTestId('cut-status')).toContainText('Исходные ячейки пока сохранены');
  await select(page, ...to);
  await page.keyboard.press('ControlOrMeta+V');
}

// Delay transport only. The real server, durable journal, Worker and WASM engine
// remain in use; each gate is explicitly released by the test.
async function moveGate(page: Page) {
  type Frame = Parameters<WebSocketRoute['send']>[0];
  let holdingCommand = false;
  let holdingEvents = false;
  let command: (() => void) | undefined;
  let publish: (() => void) | undefined;
  let committed = false;
  const rejectionCodes: number[] = [];
  const queued: Frame[] = [];
  await page.routeWebSocket('**/api/ws', (route) => {
    const server = route.connectToServer();
    route.onMessage((message) => {
      if (holdingCommand && typeof message !== 'string') {
        const payload = fromBinary(ClientEnvelopeSchema, new Uint8Array(message)).payload;
        if (payload.case === 'command' && payload.value.moveRange) {
          command ??= () => {
            holdingCommand = false;
            server.send(message);
          };
          return;
        }
      }
      server.send(message);
    });
    server.onMessage((message) => {
      if (typeof message !== 'string') {
        const payload = fromBinary(ServerEnvelopeSchema, new Uint8Array(message)).payload;
        if (payload.case === 'rejected') rejectionCodes.push(payload.value.code);
        if (
          holdingEvents &&
          [
            'committed',
            'receipt',
            'rejected',
            'documentHeadChanged',
            'calculationCommitted',
          ].includes(payload.case ?? '')
        ) {
          if (payload.case === 'committed') committed = true;
          queued.push(message);
          return;
        }
      }
      route.send(message);
    });
    publish = () => {
      holdingEvents = false;
      for (const frame of queued.splice(0)) route.send(frame);
    };
  });
  return {
    start: (delayEvents = false) => {
      holdingCommand = true;
      holdingEvents = delayEvents;
    },
    captured: () => command !== undefined,
    committed: () => committed,
    rejectionCodes,
    releaseCommand: () => {
      if (!command) throw new Error('The move command has not reached the transport gate');
      command();
    },
    releaseEvents: () => publish?.(),
  };
}

test('WASM previews overlapping moved values and formulas before commit and reconciles delayed acknowledgement', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const gate = await moveGate(page);
  await createDocument(page);
  await write(page, 0, 0, '3');
  await write(page, 0, 1, '=A1*2');
  await write(page, 0, 2, '=10');
  await write(page, 0, 3, '=B1+1');
  const before = await state(page);
  const observer = await page.context().newPage();
  observer.on('pageerror', (error) => errors.push(error.message));
  try {
    await observer.goto(page.url());
    await expect(observer.getByTestId('connection-status')).toHaveText('Подключено');
    await page.bringToFront();
    gate.start(true);
    await cutPaste(page, [0, 0], [0, 1], 2);
    await expect.poll(gate.captured).toBe(true);
    await expect(preview(page)).toContainText('не подтверждено');
    await expect(page.getByTestId('save-status')).not.toHaveText('Сохранено');
    await expect(page.getByTestId('cut-status')).toContainText('ожидает подтверждения');
    await select(page, 0, 0);
    await expect(active(page)).toHaveText('A1: ');
    await select(page, 0, 1);
    await expect(active(page)).toHaveText('B1: 3');
    await select(page, 0, 2);
    await expect(active(page)).toHaveText('C1: 6');
    await expect(bar(page)).toHaveValue('=(B1*2)');
    await select(page, 0, 3);
    await expect(active(page)).toHaveText('D1: 7');
    await expect(bar(page)).toHaveValue('=(C1+1)');
    expect(await state(page)).toEqual(before);
    await observer.bringToFront();
    await select(observer, 0, 2);
    await expect(active(observer)).toHaveText('C1: 10');
    await expect(preview(observer)).toHaveCount(0);
    await page.bringToFront();
    gate.releaseCommand();
    await expect.poll(gate.committed).toBe(true);
    await expect
      .poll(async () => (await state(page)).version)
      .toBe(String(BigInt(before.version) + 1n));
    // The server has durably committed, but this tab still owns only a preview.
    await expect(preview(page)).toContainText('не подтверждено');
    await expect(page.getByTestId('cut-status')).toContainText('ожидает подтверждения');
    gate.releaseEvents();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await expect(page.getByTestId('cut-status')).toHaveCount(0);
    await expect(preview(page)).toHaveCount(0);
    await expect(active(page)).toHaveText('D1: 7');
    await expect(bar(page)).toHaveValue('=(C1+1)');
    await expect(active(observer)).toHaveText('C1: 6');
    const committed = await state(page);
    expect(await state(observer)).toEqual(committed);
    await page.reload();
    await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
    expect(await state(page)).toEqual(committed);
    expect(errors).toEqual([]);
  } finally {
    gate.releaseEvents();
    await observer.close();
  }
});

test('a newer confirmed head discards scalar move preview and a stale refusal never changes source cells', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const gate = await moveGate(page);
  await createDocument(page);
  await write(page, 0, 0, '7');
  const before = await state(page);
  const other = await page.context().newPage();
  try {
    await other.goto(page.url());
    await expect(other.getByTestId('connection-status')).toHaveText('Подключено');
    await page.bringToFront();
    gate.start();
    await cutPaste(page, [0, 0], [1, 0]);
    await expect.poll(gate.captured).toBe(true);
    await expect(preview(page)).toContainText('не подтверждено');
    await expect(active(page)).toHaveText('A2: 7');
    expect(await state(page)).toEqual(before);
    await other.bringToFront();
    await write(other, 0, 1, '9');
    await page.bringToFront();
    await expect(preview(page)).toHaveCount(0);
    await expect(active(page)).toHaveText('A2: ');
    await select(page, 0, 0);
    await expect(active(page)).toHaveText('A1: 7');
    const current = await state(page);
    expect(BigInt(current.version)).toBe(BigInt(before.version) + 1n);
    gate.releaseCommand();
    await expect(page.getByRole('alert')).toContainText('Источник диапазона изменился');
    expect(gate.rejectionCodes).toContain(17);
    await expect(page.getByTestId('cut-status')).toContainText('Вырежьте');
    await expect(preview(page)).toHaveCount(0);
    expect(await state(page)).toEqual(current);
    await select(page, 1, 0);
    await expect(active(page)).toHaveText('A2: ');
    expect(errors).toEqual([]);
  } finally {
    await other.close();
  }
});
