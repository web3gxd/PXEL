import { expect, test, type Page } from '@playwright/test';
import { fromBinary } from '@bufbuild/protobuf';
import { ClientEnvelopeSchema } from '../src/generated/pxel/wire/v1/session_pb';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const active = (page: Page) => page.getByTestId('active-cell');
async function createDocument(page: Page) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function write(page: Page, value: string) {
  await bar(page).fill(value);
  await bar(page).press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}

test('cut in one tab moves a formula range in another tab and updates inbound references after commit', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await createDocument(page);
  await write(page, '3');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await write(page, '=A1*2');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await write(page, '=B1+1');
  const second = await page.context().newPage();
  const moves: Uint8Array[] = [];
  second.on('pageerror', (error) => errors.push(error.message));
  second.on('websocket', (socket) =>
    socket.on('framesent', ({ payload }) => {
      if (typeof payload === 'string') return;
      const envelope = fromBinary(ClientEnvelopeSchema, new Uint8Array(payload));
      if (envelope.payload.case === 'command' && envelope.payload.value.moveRange) {
        expect(envelope.payload.value.operation).toBeUndefined();
        expect(envelope.payload.value.copyRange).toBeUndefined();
        moves.push(envelope.payload.value.operationId);
      }
    }),
  );
  await second.goto(page.url());
  await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
  await page.bringToFront();
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('ControlOrMeta+X');
  await expect(page.getByTestId('cut-status')).toContainText('Исходные ячейки пока сохранены');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('3\t=(A1*2)');
  const id = new URL(page.url()).searchParams.get('document');
  const before = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(before.version).toBe('4');

  await second.bringToFront();
  await grid(second).focus();
  await second.keyboard.press('ArrowDown');
  await second.keyboard.press('ControlOrMeta+V');
  await expect(active(second)).toHaveText('A2: 3');
  await expect(second.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(second.getByTestId('cut-status')).toHaveCount(0);
  expect(moves).toHaveLength(1);
  await second.keyboard.press('ArrowRight');
  await expect(active(second)).toHaveText('B2: 6');
  await expect(bar(second)).toHaveValue('=(A2*2)');
  await expect(second.getByRole('button', { name: 'Отменить последнюю правку' })).toBeEnabled();

  await page.bringToFront();
  await grid(page).focus();
  await page.keyboard.press('ArrowLeft');
  await expect(active(page)).toHaveText('A1: ');
  await page.keyboard.press('ArrowRight');
  await expect(active(page)).toHaveText('B1: ');
  await page.keyboard.press('ArrowRight');
  await expect(active(page)).toHaveText('C1: 7');
  await expect(bar(page)).toHaveValue('=(B2+1)');
  await second.reload();
  await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
  const restored = await (await second.request.get(`/api/documents/${id}`)).json();
  expect(restored.version).toBe('5');
  expect(restored.sheets[0].cells).toHaveLength(3);
  expect(errors).toEqual([]);
  await second.close();
});

test('rejected partial-range cut keeps source data and reports the unsupported dependency', async ({
  page,
}) => {
  await createDocument(page);
  await write(page, '3');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await write(page, '5');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await write(page, '=SUM(A1:B1)');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ControlOrMeta+X');
  await page.keyboard.press('ArrowDown');
  const id = new URL(page.url()).searchParams.get('document');
  const before = await (await page.request.get(`/api/documents/${id}`)).json();
  await page.keyboard.press('ControlOrMeta+V');
  await expect(page.getByRole('alert')).toContainText('часть ссылки');
  await expect(page.getByTestId('cut-status')).toContainText('Исходные ячейки пока сохранены');
  await expect(active(page)).toHaveText('A2: ');
  const after = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(after).toEqual(before);
  await page.keyboard.press('ArrowUp');
  await expect(active(page)).toHaveText('A1: 3');
});

test('remote edits invalidate a cut without falling back to clipboard text', async ({ page }) => {
  await createDocument(page);
  await write(page, 'source');
  const second = await page.context().newPage();
  await second.goto(page.url());
  await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
  await page.bringToFront();
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ControlOrMeta+X');
  await second.bringToFront();
  await grid(second).focus();
  await second.keyboard.press('ArrowRight');
  await write(second, 'remote');
  await expect(page.getByTestId('cut-status')).toContainText('Вырежьте диапазон ещё раз');
  await page.bringToFront();
  await grid(page).focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ControlOrMeta+V');
  await expect(page.getByRole('alert')).toContainText('Вырежьте его ещё раз');
  await expect(active(page)).toHaveText('A2: ');
  const id = new URL(page.url()).searchParams.get('document');
  const state = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(state.version).toBe('3');
  expect(state.sheets[0].cells).toHaveLength(2);
  await second.close();
});

test('cut inside the cell editor remains native text editing instead of moving cells', async ({
  page,
}) => {
  await createDocument(page);
  await write(page, 'source');
  await grid(page).focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('F2');
  const editor = page.getByRole('textbox', { name: 'Редактор ячейки' });
  await editor.press('ControlOrMeta+A');
  await editor.press('ControlOrMeta+X');
  await expect(editor).toHaveValue('');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('source');
  await expect(page.getByTestId('cut-status')).toHaveCount(0);
  await editor.press('Escape');
  await expect(active(page)).toHaveText('A1: source');
  const id = new URL(page.url()).searchParams.get('document');
  const state = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(state.version).toBe('2');
  expect(state.sheets[0].cells[0].value).toEqual({ type: 'text', value: 'source' });
});
