import { editorButton, editorTrigger } from './editor-menu';
import { createBookFromUi } from './create-book';
import { expect, test, type Page } from '@playwright/test';
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import {
  ClientEnvelopeSchema,
  ServerEnvelopeSchema,
} from '../src/generated/pxel/wire/v1/session_pb';
import { uuidV7, type Projection } from '../src/document/store';
const bytes = (id: string) =>
  Uint8Array.from(
    id
      .replaceAll('-', '')
      .match(/../g)!
      .map((value) => parseInt(value, 16)),
  );
async function book(page: Page): Promise<Projection> {
  const path = `/api/documents/${new URL(page.url()).searchParams.get('document')}`;
  let response = await page.request.get(path);
  for (const delay of [100, 200, 400, 800, 1600]) {
    if (response.status() !== 429) break;
    await new Promise((resolve) => setTimeout(resolve, delay));
    response = await page.request.get(path);
  }
  expect(response.ok(), `Document read returned ${response.status()}`).toBe(true);
  return response.json();
}
async function fixture(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const previous = new URL(page.url()).searchParams.get('document');
  await createBookFromUi(page);
  await expect.poll(() => new URL(page.url()).searchParams.get('document')).not.toBe(previous);
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  await expect(await editorButton(page, 'Проверка констант')).toBeEnabled();
  await page.keyboard.press('Escape');
}
async function formulas(page: Page, start: number, count: number) {
  const before = await book(page),
    sheet = before.sheets[0]!;
  const identity = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: new URL(page.url()).origin } })
  ).json();
  const hello = toBinary(
    ClientEnvelopeSchema,
    create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'hello',
        value: { clientInstanceId: bytes(uuidV7()), clientName: 'diagnostics-test' },
      },
    }),
  );
  const commands = Array.from({ length: count }, (_, offset) => {
    const row = start + offset;
    return [
      ...toBinary(
        ClientEnvelopeSchema,
        create(ClientEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'command',
            value: {
              actorId: bytes(identity.actor_id),
              documentId: bytes(before.document_id),
              operationId: bytes(uuidV7()),
              baseVersion: BigInt(before.version) + BigInt(offset),
              clientSequence: BigInt(offset + 1),
              formulaCommand: {
                address: {
                  sheetId: bytes(sheet.id),
                  rowId: bytes(sheet.rows[row]!),
                  columnId: bytes(sheet.columns[1]!),
                },
                source: `=A${row + 1}*${row === 99 ? 3 : 2}`,
              },
            },
          },
        }),
      ),
    ];
  });
  const frames = await page.evaluate(
    ({ hello, commands }) =>
      new Promise<number[][]>((resolve, reject) => {
        const url = new URL('/api/ws', location.href);
        url.protocol = 'ws:';
        const socket = new WebSocket(url);
        socket.binaryType = 'arraybuffer';
        const frames: number[][] = [];
        let welcomed = false;
        const timer = setTimeout(() => {
          socket.close();
          reject(new Error('Diagnostic fixture timed out'));
        }, 20000);
        socket.onopen = () => socket.send(new Uint8Array(hello));
        socket.onmessage = (event: MessageEvent<ArrayBuffer>) => {
          if (!welcomed) {
            welcomed = true;
            socket.send(new Uint8Array(commands[0]!));
            return;
          }
          frames.push([...new Uint8Array(event.data)]);
          if (frames.length < commands.length)
            socket.send(new Uint8Array(commands[frames.length]!));
          else {
            clearTimeout(timer);
            socket.close();
            resolve(frames);
          }
        };
        socket.onerror = () => {
          clearTimeout(timer);
          socket.close();
          reject(new Error('Diagnostic fixture connection failed'));
        };
      }),
    { hello: [...hello], commands },
  );
  for (const frame of frames)
    expect(fromBinary(ServerEnvelopeSchema, new Uint8Array(frame)).payload.case).toBe('committed');
  await expect
    .poll(async () => (await book(page)).version)
    .toBe(String(BigInt(before.version) + BigInt(count)));
}
test('99/100 threshold and dismissible constant warning preserve head context and history', async ({
  page,
}) => {
  test.setTimeout(60000);
  await fixture(page);
  await formulas(page, 0, 99);
  let before = await book(page);
  const path = `/api/documents/${before.document_id}`;
  const scan = async (version: string) =>
    (
      await page.request.post(`${path}/diagnostics`, {
        headers: { Origin: new URL(page.url()).origin },
        data: { schema_version: 1, document_version: version },
      })
    ).json();
  expect((await scan(before.version)).warnings).toEqual([]);
  expect((await book(page)).state_hash).toBe(before.state_hash);
  await formulas(page, 99, 1);
  before = await book(page);
  const analysis = await scan(before.version);
  expect(analysis.warnings).toHaveLength(1);
  expect(analysis.warnings[0]).toMatchObject({ row: 99, column: 1, formulas: 100, dominant: 99 });
  expect(
    (
      await page.request.post(`${path}/diagnostics`, {
        headers: { Origin: new URL(page.url()).origin },
        data: { schema_version: 2, document_version: before.version },
      })
    ).status(),
  ).toBe(400);
  expect(
    (
      await page.request.post(`${path}/diagnostics`, {
        headers: { Origin: new URL(page.url()).origin },
        data: { schema_version: 1, document_version: '1' },
      })
    ).status(),
  ).toBe(409);
  const history = await (await page.request.get(`${path}/undo-history`)).json();
  await (await editorButton(page, 'Проверка констант')).click();
  const dialog = page.getByRole('dialog', { name: 'Проверка констант' });
  await expect(dialog.getByRole('status')).toContainText('предупреждений 1');
  await dialog.getByRole('button', { name: 'Скрыть предупреждение B100', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('предупреждений 0');
  await page.keyboard.press('Escape');
  const after = await book(page);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.calculation_context).toEqual(before.calculation_context);
  expect(await (await page.request.get(`${path}/undo-history`)).json()).toEqual(history);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await (await editorButton(page, 'Проверка констант')).click();
  await expect(dialog.getByRole('status')).toContainText('предупреждений 0');
});
for (const width of [390, 1440])
  for (const theme of ['light', 'dark'] as const)
    test(`shared WASM Unicode syntax and accessible dialog ${width} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await fixture(page);
      const before = await book(page);
      const input = page.getByRole('textbox', { name: 'Значение ячейки' });
      await input.fill('=IF(TRUE,"Я😀",A1+2)');
      await expect(page.locator('.formula-syntax-overlay .syntax-text')).toHaveText('"Я😀"');
      await expect(page.locator('.formula-syntax-overlay .syntax-reference')).toHaveText('A1');
      await expect(page.locator('.formula-syntax-overlay .syntax-boolean')).toHaveText('TRUE');
      await expect(page.locator('.formula-syntax-overlay .syntax-number')).toHaveText('2');
      await input.press('Escape');
      expect((await book(page)).state_hash).toBe(before.state_hash);
      const opener = editorTrigger(page, 'Проверка констант');
      await (await editorButton(page, 'Проверка констант')).click();
      const dialog = page.getByRole('dialog', { name: 'Проверка констант' });
      await expect(dialog.getByRole('status')).toContainText('проверено формул 0');
      expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(width);
      await page.keyboard.press('Escape');
      await expect(opener).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    });
