import { expect, test } from '@playwright/test';
import { fromBinary } from '@bufbuild/protobuf';
import { ClientEnvelopeSchema } from '../src/generated/pxel/wire/v1/session_pb';

test('first volatile formula calculates without manual recalc and renews context on reload', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await bar.fill('=RAND()');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  const active = page.getByTestId('active-cell');
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  const value = await active.textContent();
  const id = new URL(page.url()).searchParams.get('document');
  const state = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(state.version).toBe('3');
  expect(state.calculation_context.epoch).toBe('1');
  expect(state.calculation_context.reason).toBe('open');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(active).not.toHaveText(value!);
  await expect(bar).toHaveValue('=RAND()');
  const restored = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(restored.version).toBe('4');
  expect(restored.calculation_context.epoch).toBe('2');
  expect(restored.calculation_context.reason).toBe('open');
  expect(restored.calculation_context.context_id).not.toBe(state.calculation_context.context_id);
});

test('active NOW updates from a durable heartbeat timer without user edits', async ({ page }) => {
  test.setTimeout(95_000);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=NOW()');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  const active = page.getByTestId('active-cell');
  await expect(active).toHaveText(/^A1: \d/);
  const initial = await active.textContent();
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  const before = await state();
  expect(before.version).toBe('3');
  await expect
    .poll(async () => (await state()).calculation_context?.epoch, {
      timeout: 80_000,
      intervals: [1000],
    })
    .toBe('2');
  const after = await state();
  expect(after.version).toBe('4');
  expect(after.calculation_context.reason).toBe('timer');
  expect(
    BigInt(after.calculation_context.effective_timestamp_utc_ms) -
      BigInt(before.calculation_context.effective_timestamp_utc_ms),
  ).toBeGreaterThanOrEqual(60_000n);
  await expect(active).not.toHaveText(initial!);
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  expect(await bar.inputValue()).toBe('=NOW()');
});

test('editing another cell advances RAND epoch automatically and survives reload', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const active = page.getByTestId('active-cell');
  await bar.fill('=RAND()');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('1');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText(/^A1: 0\./);
  const first = await active.textContent();
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText(/^B1:/);
  await bar.fill('7');
  await bar.press('Enter');
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('2');
  expect((await state()).version).toBe('5');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowLeft');
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(active).not.toHaveText(first!);
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  const second = await active.textContent();
  await page.reload();
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(active).not.toHaveText(second!);
  expect((await state()).calculation_context.epoch).toBe('3');
  expect((await state()).calculation_context.reason).toBe('open');
});

test('volatile RAND preview stays separate and durable epoch survives reload', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await page.getByTestId('recalculate').click();
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('1');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const active = page.getByTestId('active-cell');
  await bar.fill('=RAND()');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('2');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText(/^A1: 0\./);
  const first = await active.textContent();
  const number = Number(first?.slice(4));
  expect(number).toBeGreaterThanOrEqual(0);
  expect(number).toBeLessThan(1);
  await bar.fill('=RAND()+1');
  await expect.poll(async () => Number((await active.textContent())?.slice(4))).toBe(number + 1);
  await expect(page.getByTestId('calculation-preview')).toContainText('не подтверждено');
  expect((await state()).version).toBe('4');
  await bar.press('Escape');
  await expect(active).toHaveText(first!);
  await page.getByTestId('recalculate').click();
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('3');
  await expect(active).not.toHaveText(first!);
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  const second = await active.textContent();
  await page.reload();
  await expect(active).toHaveText(/^A1: 0\./);
  await expect(active).not.toHaveText(second!);
  await expect(bar).toHaveValue('=RAND()');
  expect((await state()).calculation_context.epoch).toBe('4');
  expect((await state()).calculation_context.reason).toBe('open');
});

test('opening a second tab publishes the same renewed RAND to both clients', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=RAND()');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowUp');
  const active = page.getByTestId('active-cell');
  await expect(active).toHaveText(/^A1: 0\./);
  const first = await active.textContent();
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(active).not.toHaveText(first!);
    await expect(active).toHaveText(/^A1: 0\./);
    await expect(second.getByTestId('active-cell')).toHaveText((await active.textContent())!);
    await expect(second.getByTestId('calculation-preview')).toHaveCount(0);
    await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
    const id = new URL(page.url()).searchParams.get('document');
    const state = await (await page.request.get(`/api/documents/${id}`)).json();
    expect(state.version).toBe('4');
    expect(state.calculation_context.epoch).toBe('2');
    expect(state.calculation_context.reason).toBe('open');
  } finally {
    await second.close();
  }
});

for (const scenario of [
  {
    name: 'calendar date functions',
    source: '=YEAR(DATE(A1,2,29))*10000+MONTH(DATE(A1,2,29))*100+DAY(DATE(A1,2,29))',
    input: '2000',
    edit: '2100',
    before: '20000229',
    after: '21000301',
  },
  {
    name: 'Unicode text functions',
    source: '=CONCAT(UPPER(LEFT(TRIM(A1),2)),LOWER(RIGHT("Я",1)),MID("😀abc",2,2),LEN(A1))',
    input: 'абв',
    edit: 'д',
    before: 'АБяab3',
    after: 'Дяab1',
  },
  {
    name: 'ROUND family',
    source: '=IF(ROUND(A1,2)=ROUNDUP(A1,2),ROUNDDOWN(A1,2),0)',
    input: '1.005',
    edit: '1.004',
    before: '1',
    after: '0',
  },
  {
    name: 'POWER and exponentiation',
    source: '=POWER(A1,0.5)+A1^0.5',
    input: '4',
    edit: '9',
    before: '4',
    after: '6',
  },
]) {
  test(`${scenario.name} follows dependent edits and survives reload`, async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
    await page.getByRole('button', { name: 'Создать таблицу' }).click();
    const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
    const grid = page.getByRole('region', { name: 'Область таблицы' });
    const source = scenario.source;
    await bar.fill(scenario.input);
    await bar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await grid.focus();
    await page.keyboard.press('ArrowUp');
    await page.keyboard.press('ArrowRight');
    await bar.fill(source);
    await bar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await grid.focus();
    await page.keyboard.press('ArrowUp');
    await expect(page.getByTestId('active-cell')).toHaveText(`B1: ${scenario.before}`);
    await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
    await page.keyboard.press('ArrowLeft');
    await bar.fill(scenario.edit);
    await bar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await page.reload();
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await grid.focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByTestId('active-cell')).toHaveText(`B1: ${scenario.after}`);
    await expect(bar).toHaveValue(source);
    await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  });
}

test('ABS and MOD update a referenced input and survive reload', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await bar.fill('-3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=ABS(MOD(A1,2))');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: 1');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await page.keyboard.press('ArrowLeft');
  await bar.fill('4');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 4');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: 0');
  await expect(bar).toHaveValue('=ABS(MOD(A1,2))');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
});

test('persisted server context reaches real WASM initialization and survives reload', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const target = window as Window & {
      __pxelPreviewInitializations?: unknown[];
      __pxelPreviewErrors?: string[];
    };
    target.__pxelPreviewInitializations = [];
    target.__pxelPreviewErrors = [];
    const Original = window.Worker;
    window.Worker = class extends Original {
      constructor(...args: ConstructorParameters<typeof Worker>) {
        super(...args);
        this.addEventListener(
          'message',
          (event: MessageEvent<{ error?: string; phase?: string }>) => {
            if (event.data.error)
              target.__pxelPreviewErrors?.push(`${event.data.error}:${event.data.phase}`);
          },
        );
      }
      override postMessage(
        message: unknown,
        options?: StructuredSerializeOptions | Transferable[],
      ) {
        const request = message as { kind?: string; data?: unknown };
        if (request?.kind === 'init') target.__pxelPreviewInitializations?.push(request.data);
        if (Array.isArray(options)) super.postMessage(message, options);
        else super.postMessage(message, options ?? {});
      }
    };
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=1+2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByTestId('recalculate').click();
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  await expect.poll(async () => (await state()).calculation_context?.epoch).toBe('1');
  const context = (await state()).calculation_context;
  const latest = () =>
    page.evaluate(() => {
      const entries = (window as Window & { __pxelPreviewInitializations?: unknown[] })
        .__pxelPreviewInitializations;
      return entries?.at(-1) as
        { schema_version: number; calculation_context: unknown } | undefined;
    });
  await expect.poll(async () => (await latest())?.calculation_context).toEqual(context);
  expect((await latest())?.schema_version).toBe(3);
  await page.reload();
  await expect.poll(async () => (await latest())?.calculation_context).toEqual(context);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 3');
  await bar.fill('=2+3');
  await expect
    .poll(async () => ({
      cell: await page.getByTestId('active-cell').textContent(),
      errors: await page.evaluate(
        () => (window as Window & { __pxelPreviewErrors?: string[] }).__pxelPreviewErrors,
      ),
    }))
    .toEqual({ cell: 'A1: 5', errors: [] });
  await expect(page.getByTestId('calculation-preview')).toContainText('не подтверждено');
  expect((await state()).version).toBe('3');
});

test('delayed formula retains its target after another tab inserts a row', async ({
  page,
  context,
}) => {
  let holding = false;
  let release: (() => void) | undefined;
  await page.routeWebSocket('**/api/ws', (route) => {
    const server = route.connectToServer();
    route.onMessage((message) => {
      if (holding && typeof message !== 'string') {
        const payload = fromBinary(ClientEnvelopeSchema, new Uint8Array(message)).payload;
        if (payload.case === 'command' && payload.value.formulaCommand) {
          release ??= () => {
            holding = false;
            server.send(message);
          };
          return;
        }
      }
      server.send(message);
    });
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await bar.fill('11');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  holding = true;
  await bar.fill('=A1+1');
  await bar.press('Enter');
  await expect.poll(() => release !== undefined).toBe(true);
  const other = await context.newPage();
  await other.goto(page.url());
  await expect(other.getByTestId('connection-status')).toHaveText('Подключено');
  await other.getByRole('button', { name: 'Строка ↑', exact: true }).click();
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('3');
  release?.();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('4');
  await grid.focus();
  await expect(page.getByTestId('active-cell')).toHaveText('B2: 12');
  await expect(bar).toHaveValue('=(A2+1)');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await other.reload();
  await expect(other.getByTestId('save-status')).toHaveText('Сохранено');
  await other.getByRole('region', { name: 'Область таблицы' }).focus();
  await other.keyboard.press('ArrowDown');
  await other.keyboard.press('ArrowRight');
  await expect(other.getByTestId('active-cell')).toHaveText('B2: 12');
  await expect(other.getByTestId('calculation-preview')).toHaveCount(0);
});

function uuidV7() {
  const id = crypto.getRandomValues(new Uint8Array(16));
  let time = BigInt(Date.now());
  for (let index = 5; index >= 0; index--) {
    id[index] = Number(time & 255n);
    time >>= 8n;
  }
  id[6] = 0x70 | ((id[6] ?? 0) & 15);
  id[8] = 0x80 | ((id[8] ?? 0) & 63);
  const raw = [...id].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${raw.slice(0, 8)}-${raw.slice(8, 12)}-${raw.slice(12, 16)}-${raw.slice(16, 20)}-${raw.slice(20)}`;
}

test('owner downloads the confirmed scalar workbook as XLSX', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('23');
  await bar.press('Enter');
  const id = new URL(page.url()).searchParams.get('document');
  expect(id).not.toBeNull();
  await expect
    .poll(async () => {
      const response = await page.request.get(`/api/documents/${id}`);
      return (await response.json()).version;
    })
    .toBe('2');
  const downloadPromise = page.waitForEvent('download');
  await page.getByTestId('export-xlsx').click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('PXEL.xlsx');
  expect(await download.path()).toBeTruthy();
  await expect(page.getByTestId('export-xlsx')).toBeEnabled();
});

test('recalculate retries a lost HTTP response without committing a second epoch', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=AND(TRUE,TRUE)');
  await bar.press('Enter');
  const id = new URL(page.url()).searchParams.get('document');
  const state = async () => (await page.request.get(`/api/documents/${id}`)).json();
  await expect.poll(async () => (await state()).version).toBe('2');
  const requests: string[] = [];
  await page.route('**/api/documents/*/recalculate', async (route) => {
    requests.push((route.request().postDataJSON() as { operation_id: string }).operation_id);
    if (requests.length === 1) {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      await route.abort('failed');
    } else {
      await route.continue();
    }
  });
  const button = page.getByTestId('recalculate');
  await button.click();
  await expect.poll(async () => (await state()).version).toBe('3');
  await expect(button).toBeEnabled();
  await button.click();
  await expect.poll(() => requests.length).toBe(2);
  await expect(button).toBeEnabled();
  expect(requests[1]).toBe(requests[0]);
  const confirmed = await state();
  expect(confirmed.version).toBe('3');
  expect(confirmed.calculation_context.epoch).toBe('1');
  await page.reload();
  await expect(bar).toHaveValue('=AND(TRUE,TRUE)');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: TRUE');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
});

test('logical formula recalculates after editing its input and survives reload', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=AND(A1>0,NOT(A1=0),OR(A1=2,FALSE))');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: TRUE');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await page.keyboard.press('ArrowLeft');
  await bar.fill('-1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: FALSE');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await page.reload();
  await expect(bar).toHaveValue('-1');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: FALSE');
});

test('copying one formula shifts relative references in WASM and commits server-bound text', async ({
  page,
}) => {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1+$B$1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ControlOrMeta+C');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('A1');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ControlOrMeta+V');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('D2: 3');
  await expect(bar).toHaveValue(/B2.*\$B\$1/);
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(bar).toHaveValue('2');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('D2: 3');
});

test('TSV paste commits one durable batch and survives reload', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', '12\tTRUE\nalpha\t34');
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  expect(id).not.toBeNull();
  await expect
    .poll(async () => {
      const response = await page.request.get(`/api/documents/${id}`);
      return (await response.json()).version;
    })
    .toBe('2');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.version).toBe('2');
  expect(state.sheets[0].cells).toHaveLength(4);
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('12');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toHaveText('B2: 34');
});

test('keyboard range selection copies a sparse rectangle and pastes it in one version', async ({
  page,
}) => {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  await grid.focus();
  await page.evaluate(() => navigator.clipboard.writeText('1\tTRUE\nx\t'));
  await grid.focus();
  await page.keyboard.press('ControlOrMeta+V');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 1');
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('Shift+ArrowDown');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено A1:B2');
  await page.keyboard.press('ControlOrMeta+C');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1\tTRUE\nx\t');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('D4: ');
  await page.keyboard.press('ControlOrMeta+V');
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => {
      const state = await page.evaluate(
        async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
        id,
      );
      return state.version;
    })
    .toBe('3');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.sheets[0].cells).toHaveLength(6);
  await expect(page.getByTestId('active-cell')).toHaveText('D4: 1');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await grid.focus();
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 1');
  for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowRight');
  for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('D4: 1');
});

test('mixed value and formula range copies atomically with shifted references', async ({
  page,
}) => {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1*2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const second = await page.context().newPage();
  await second.goto(page.url());
  await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Shift+ArrowRight');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено A1:B1');
  await page.keyboard.press('ControlOrMeta+C');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByTestId('active-cell')).toHaveText('C3: ');
  await page.keyboard.press('ControlOrMeta+V');
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => {
      const state = await page.evaluate(
        async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
        id,
      );
      return state.version;
    })
    .toBe('4');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toHaveText('C3: 3');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByTestId('active-cell')).toHaveText('D3: 6');
  await expect(bar).toHaveValue('=(C3*2)');
  const secondGrid = second.getByRole('region', { name: 'Область таблицы' });
  await secondGrid.focus();
  await second.keyboard.press('ArrowRight');
  await second.keyboard.press('ArrowRight');
  await second.keyboard.press('ArrowRight');
  await second.keyboard.press('ArrowDown');
  await second.keyboard.press('ArrowDown');
  await expect(second.getByTestId('active-cell')).toHaveText('D3: 6');
  await second.close();
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.version).toBe('4');
  expect(
    state.sheets[0].cells.some(
      (cell: { value: { type: string; value?: { current_text?: string } } }) =>
        cell.value.type === 'formula' && cell.value.value?.current_text === '=(C3*2)',
    ),
  ).toBe(true);
});

test('pointer drag selects a rectangle without allocating DOM cells', async ({ page }) => {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  const box = await grid.boundingBox();
  if (!box) throw new Error('Grid has no bounds');
  await page.mouse.move(box.x + 46 + 56, box.y + 28 + 14);
  await page.mouse.down();
  await page.mouse.move(box.x + 46 + 112 + 56, box.y + 28 + 28 + 14, { steps: 4 });
  await page.mouse.up();
  await page.keyboard.press('ControlOrMeta+C');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('\t\n\t');
  expect(await page.locator('[role="gridcell"]').count()).toBe(0);
});

test('missing WASM visibly disables preview while server calculation remains available', async ({
  page,
}) => {
  await page.route('**/*.wasm*', (route) => route.abort());
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('preview-status')).toHaveText('Предпросмотр недоступен');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=2+3');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 5');
  await expect(page.getByTestId('calculation-preview')).toHaveCount(0);
});

test('WASM previews an unsaved draft and cancellation leaves the durable document unchanged', async ({
  page,
}) => {
  const wasmResponses: number[] = [];
  page.on('response', (response) => {
    if (response.headers()['content-type']?.startsWith('application/wasm'))
      wasmResponses.push(response.status());
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  const before = await page.evaluate(async (documentId) => {
    const response = await fetch(`/api/documents/${documentId}`);
    if (!response.ok) throw new Error(`Document read failed (${response.status})`);
    return response.json();
  }, id);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=SUM(');
  // Wait for the invalid draft to reach the Worker before replacing it.
  await expect(page.getByTestId('preview-status')).toContainText('формул');
  await bar.fill('=2+3');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: 5');
  await expect(page.getByTestId('calculation-preview')).toContainText('не подтверждено');
  await bar.press('Escape');
  await expect(page.getByTestId('active-cell')).toHaveText('A1: ');
  const after = await page.evaluate(async (documentId) => {
    const response = await fetch(`/api/documents/${documentId}`);
    if (!response.ok) throw new Error(`Document read failed (${response.status})`);
    return response.json();
  }, id);
  expect(after.version).toBe(before.version);
  expect(after.state_hash).toBe(before.state_hash);
  expect(after.sheets[0].cells).toEqual([]);
  expect(wasmResponses).toEqual([200]);
});

test('formula results recalculate while editors preserve source across reload', async ({
  page,
}) => {
  const calculationHttpRequests: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.endsWith('/calculation'))
      calculationHttpRequests.push(request.method());
  });
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const active = page.getByTestId('active-cell');
  await bar.fill('2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText('B1: ');
  await bar.fill('=A1+3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText('B1: 5');
  await expect(bar).toHaveValue('=A1+3');
  await page.keyboard.press('ArrowLeft');
  await bar.fill('8');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(bar).toHaveValue('8');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText('B1: 11');
  await expect(bar).toHaveValue('=A1+3');
  await bar.fill('=B1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await expect(active).toHaveText('B1: #CYCLE!');
  expect(calculationHttpRequests).toEqual([]);
});

test('create, edit sparse cells, keyboard navigation, reload durable values', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page).toHaveURL(/document=[0-9a-f-]{36}/);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await page.keyboard.type('42');
  await expect(page.getByRole('textbox', { name: 'Редактор ячейки' })).toHaveValue('42');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('active-cell')).toHaveText('A2: ');
  await page.keyboard.press('Tab');
  await expect(page.getByTestId('active-cell')).toHaveText('B2: ');
  await page.keyboard.press('F2');
  await page.getByRole('textbox', { name: 'Редактор ячейки' }).fill('Проверка сохранения');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('42');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue(
    'Проверка сохранения',
  );
  await page.keyboard.press('F2');
  await page.getByRole('textbox', { name: 'Редактор ячейки' }).fill('Отмена');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue(
    'Проверка сохранения',
  );
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('TRUE');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('42');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowRight');
  await expect(bar).toHaveValue('TRUE');
  expect(errors).toEqual([]);
});

test('adding a sheet persists stable tabs and reaches another browser', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await page.getByRole('button', { name: 'Добавить лист' }).click();
    await expect(page.getByRole('button', { name: 'Перейти на лист Лист 2' })).toBeVisible();
    await expect(page.locator('.sheet-summary')).toContainText('Лист 2');
    await expect(second.getByRole('button', { name: 'Перейти на лист Лист 2' })).toBeVisible();
    await second.getByRole('button', { name: 'Перейти на лист Лист 2' }).click();
    await expect(second.locator('.sheet-summary')).toContainText('Лист 2');
    await second.reload();
    await expect(second.getByRole('button', { name: 'Перейти на лист Лист 2' })).toBeVisible();
  } finally {
    await second.close();
  }
});

test('two browser tabs see each other’s committed cell after a head hint', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    let fullDocumentReads = 0;
    second.on('request', (request) => {
      if (
        request.method() === 'GET' &&
        /\/api\/documents\/[0-9a-f-]{36}$/.test(new URL(request.url()).pathname)
      )
        fullDocumentReads++;
    });
    const firstBar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await firstBar.fill('17');
    await firstBar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await expect(second.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('17');
    expect(fullDocumentReads).toBe(0);
    const secondBar = second.getByRole('textbox', { name: 'Значение ячейки' });
    await secondBar.fill('23');
    await secondBar.press('Enter');
    await expect(second.getByTestId('save-status')).toHaveText('Сохранено');
    await page.getByRole('region', { name: 'Область таблицы' }).focus();
    await page.keyboard.press('ArrowUp');
    await expect(firstBar).toHaveValue('23');
  } finally {
    await second.close();
  }
});

test('one-step undo is a durable compensation broadcast to another browser', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    const firstBar = page.getByRole('textbox', { name: 'Значение ячейки' });
    const secondBar = second.getByRole('textbox', { name: 'Значение ячейки' });
    await firstBar.fill('42');
    await firstBar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await expect(secondBar).toHaveValue('42');
    const grid = page.getByRole('region', { name: 'Область таблицы' });
    await grid.focus();
    await page.keyboard.press('ArrowUp');
    await expect(
      page.getByRole('button', { name: 'Отменить последнюю правку ячейки' }),
    ).toBeEnabled();
    await page.keyboard.press('Control+z');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await expect(firstBar).toHaveValue('');
    await expect(secondBar).toHaveValue('');
    await page.reload();
    await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('');
    await expect(
      page.getByRole('button', { name: 'Отменить последнюю правку ячейки' }),
    ).toBeDisabled();
  } finally {
    await second.close();
  }
});

test('undo restores a formula overwritten by a plain value', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=2+3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(bar).toHaveValue('=2+3');
  await bar.fill('42');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(bar).toHaveValue('42');
  await page.getByRole('button', { name: 'Отменить последнюю правку ячейки' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(bar).toHaveValue('=2+3');
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(page.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('=2+3');
});

test('one-step undo rejects a later edit by another user', async ({ page, browser }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const documentId = new URL(page.url()).searchParams.get('document');
  if (!documentId) throw new Error('Document ID missing');
  const origin = new URL(page.url()).origin;
  const owner = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const editorContext = await browser.newContext();
  const account = `undo_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
  try {
    const temporarySession = await editorContext.request.post(origin + '/api/dev/session', {
      headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': account },
    });
    expect(temporarySession.status()).toBe(204);
    const editorIdentity = await (
      await editorContext.request.get(origin + '/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    await editorContext.request.delete(origin + '/api/dev/session', {
      headers: { Origin: origin },
    });
    const grant = await page.request.post(`/api/documents/${documentId}/permissions`, {
      headers: { Origin: origin },
      data: {
        action: 'grant',
        change_id: uuidV7(),
        target_user_id: editorIdentity.actor_id,
        role: 'editor',
      },
    });
    expect(grant.status()).toBe(200);
    const shared = await editorContext.request.post(origin + '/api/dev/session', {
      headers: {
        Origin: origin,
        'X-PXEL-DEV-ACCOUNT': account,
        'X-PXEL-DEV-WORKSPACE': owner.workspace_id,
      },
    });
    expect(shared.status()).toBe(204);
    const editor = await editorContext.newPage();
    await editor.goto(page.url());
    await expect(editor.getByTestId('connection-status')).toHaveText('Подключено');
    const ownerBar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await ownerBar.fill('mine');
    await ownerBar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    const editorBar = editor.getByRole('textbox', { name: 'Значение ячейки' });
    await expect(editorBar).toHaveValue('mine');
    await editorBar.fill('theirs');
    await editorBar.press('Enter');
    await expect(editor.getByTestId('save-status')).toHaveText('Сохранено');
    await page.getByRole('button', { name: 'Отменить последнюю правку ячейки' }).click();
    await expect(page.getByRole('alert')).toContainText('ячейка была изменена');
    const saved = await (await page.request.get(`/api/documents/${documentId}`)).json();
    expect(saved.sheets[0].cells[0].value).toEqual({ type: 'text', value: 'theirs' });
  } finally {
    await editorContext.close();
  }
});

test('scrolling near the end extends stable rows and updates both browsers', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const documentId = new URL(page.url()).searchParams.get('document');
  if (!documentId) throw new Error('Document ID missing');
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByRole('textbox', { name: 'Значение ячейки' })).toBeVisible();
    const firstGrid = page.getByRole('region', { name: 'Область таблицы' });
    const secondGrid = second.getByRole('region', { name: 'Область таблицы' });
    await firstGrid.hover();
    await page.mouse.wheel(0, 50_000);
    await expect
      .poll(async () => {
        const response = await page.request.get(`/api/documents/${documentId}`);
        if (!response.ok()) return 0;
        return (await response.json()).sheets[0].rows.length;
      })
      .toBe(2000);
    await expect
      .poll(() => firstGrid.evaluate((element) => element.scrollHeight))
      .toBeGreaterThan(50_000);
    await expect
      .poll(() => secondGrid.evaluate((element) => element.scrollHeight))
      .toBeGreaterThan(50_000);
    await second.reload();
    await expect
      .poll(() => secondGrid.evaluate((element) => element.scrollHeight))
      .toBeGreaterThan(50_000);
  } finally {
    await second.close();
  }
});

test('insert row before selection keeps the authored value on its stable row after reload', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  if (!id) throw new Error('Document ID missing');
  const beforeResponse = await page.request.get(`/api/documents/${id}`);
  expect(beforeResponse.ok()).toBe(true);
  const before = await beforeResponse.json();
  const oldFirstRow = before.sheets[0].rows[0];
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('42');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('button', { name: 'Строка ↑' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect
    .poll(async () => {
      const response = await page.request.get(`/api/documents/${id}`);
      if (!response.ok()) return 0;
      return (await response.json()).sheets[0].rows.length;
    })
    .toBe(1001);
  const after = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(after.sheets[0].rows[0]).not.toBe(oldFirstRow);
  expect(after.sheets[0].rows[1]).toBe(oldFirstRow);
  expect(
    after.sheets[0].cells.find((cell: { row_id: string }) => cell.row_id === oldFirstRow)?.value,
  ).toEqual({ type: 'number', value: 42 });
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(bar).toHaveValue('');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(bar).toHaveValue('42');
});

test('insertion with formulas retains the formula on its stable row', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  if (!id) throw new Error('Document ID missing');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('button', { name: 'Строка ↑' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  const projection = await response.json();
  expect(projection.version).toBe('3');
  expect(projection.sheets[0].rows).toHaveLength(1001);
  expect(projection.sheets[0].cells[0].row_id).toBe(projection.sheets[0].rows[1]);
  expect(projection.sheets[0].cells[0].value.value.current_text).toBe('=1');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(bar).toHaveValue('=1');
});

test('formula bar follows a shifted stable target after row insertion', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const active = page.getByTestId('active-cell');
  await bar.fill('7');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.getByRole('button', { name: 'Строка ↑' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(active).toHaveText('B2: 7');
  await expect(bar).toHaveValue('=A2');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(bar).toHaveValue('=A2');
  await expect(active).toHaveText('B2: 7');
});

test('deleting a referenced row shows REF and survives reload', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  if (!id) throw new Error('Document ID missing');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const active = page.getByTestId('active-cell');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await bar.fill('7');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await expect(active).toHaveText('B1: ');
  await bar.fill('=A2');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowLeft');
  await expect(active).toHaveText('A2: 7');
  await page.getByRole('button', { name: 'Строка −' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  expect((await response.json()).sheets[0].rows).toHaveLength(999);
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await expect(bar).toHaveValue('=#REF!');
  await expect(active).toHaveText('B1: #REF!');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(bar).toHaveValue('=#REF!');
  await expect(active).toHaveText('B1: #REF!');
});

test('inserting a column keeps formula target stable and shifts its A1 display', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  if (!id) throw new Error('Document ID missing');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  const active = page.getByTestId('active-cell');
  await bar.fill('7');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('button', { name: 'Столбец ↑' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  const projection = await response.json();
  expect(projection.sheets[0].columns).toHaveLength(27);
  expect(
    projection.sheets[0].cells.find(
      (cell: { value: { type: string } }) => cell.value.type === 'formula',
    )?.value.value.current_text,
  ).toBe('=B1');
  await expect(active).toHaveText('B1: 7');
  await grid.focus();
  await page.keyboard.press('ArrowRight');
  await expect(bar).toHaveValue('=B1');
});

test('deleting a referenced column shows REF and survives reload', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const id = new URL(page.url()).searchParams.get('document');
  if (!id) throw new Error('Document ID missing');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('7');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await grid.focus();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowRight');
  await bar.fill('=A1');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.getByRole('button', { name: 'Столбец −' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  const projection = await response.json();
  expect(projection.sheets[0].columns).toHaveLength(25);
  expect(
    projection.sheets[0].cells.find(
      (cell: { value: { type: string } }) => cell.value.type === 'formula',
    )?.value.value.current_text,
  ).toBe('=#REF!');
  await grid.focus();
  await expect(bar).toHaveValue('=#REF!');
  await page.reload();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(bar).toHaveValue('=#REF!');
});

test('two tabs converge after one tab is offline during a remote edit', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const documentUrl = page.url();
  const secondContext = await page
    .context()
    .browser()!
    .newContext({
      storageState: await page.context().storageState(),
    });
  await secondContext.addInitScript(() => {
    const original = window.WebSocket;
    const sockets: WebSocket[] = [];
    (window as Window & { __pxelTestSockets?: WebSocket[] }).__pxelTestSockets = sockets;
    window.WebSocket = new Proxy(original, {
      construct(target, args, newTarget) {
        const socket = Reflect.construct(target, args, newTarget) as WebSocket;
        sockets.push(socket);
        return socket;
      },
    });
  });
  const second = await secondContext.newPage();
  try {
    await second.goto(documentUrl);
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(second.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('');
    await expect
      .poll(() =>
        second.evaluate(
          () =>
            (window as Window & { __pxelTestSockets?: WebSocket[] }).__pxelTestSockets?.filter(
              (socket) => socket.readyState === 1,
            ).length,
        ),
      )
      .toBeGreaterThan(0);
    await second.evaluate(() => {
      for (const socket of (window as Window & { __pxelTestSockets?: WebSocket[] })
        .__pxelTestSockets ?? [])
        if (socket.readyState === 1) socket.close(4000);
    });
    await secondContext.setOffline(true);
    const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await bar.fill('317');
    await bar.press('Enter');
    await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    await secondContext.setOffline(false);
    // Whole-context offline mode can make Vite HMR navigate to chrome-error://.
    // A fresh document navigation verifies durable recovery for either outcome.
    await second.goto(documentUrl);
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await expect(second.getByRole('textbox', { name: 'Значение ячейки' })).toHaveValue('317');
  } finally {
    await secondContext.close();
  }
});

test('two different users see ephemeral presence without changing the saved version', async ({
  page,
  browser,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await page.getByRole('button', { name: 'Создать таблицу' }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const documentId = new URL(page.url()).searchParams.get('document');
  if (!documentId) throw new Error('Document ID missing');
  const origin = new URL(page.url()).origin;
  const ownerResponse = await page.request.get('/api/dev/identity', {
    headers: { Origin: origin },
  });
  expect(ownerResponse.status()).toBe(200);
  const owner = await ownerResponse.json();
  const viewer = await browser.newContext();
  const account = `presence_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
  try {
    const home = await viewer.request.post(origin + '/api/dev/session', {
      headers: { Origin: origin, 'X-PXEL-DEV-ACCOUNT': account },
    });
    expect(home.status()).toBe(204);
    const viewerIdentity = await (
      await viewer.request.get(origin + '/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    await viewer.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    const id = crypto.getRandomValues(new Uint8Array(16));
    let time = BigInt(Date.now());
    for (let index = 5; index >= 0; index--) {
      id[index] = Number(time & 255n);
      time >>= 8n;
    }
    id[6] = 0x70 | ((id[6] ?? 0) & 15);
    id[8] = 0x80 | ((id[8] ?? 0) & 63);
    const raw = [...id].map((byte) => byte.toString(16).padStart(2, '0')).join('');
    const changeId = `${raw.slice(0, 8)}-${raw.slice(8, 12)}-${raw.slice(12, 16)}-${raw.slice(16, 20)}-${raw.slice(20)}`;
    const grant = await page.request.post(`/api/documents/${documentId}/permissions`, {
      headers: { Origin: origin },
      data: {
        action: 'grant',
        change_id: changeId,
        target_user_id: viewerIdentity.actor_id,
        role: 'viewer',
      },
    });
    expect(grant.status()).toBe(200);
    const shared = await viewer.request.post(origin + '/api/dev/session', {
      headers: {
        Origin: origin,
        'X-PXEL-DEV-ACCOUNT': account,
        'X-PXEL-DEV-WORKSPACE': owner.workspace_id,
      },
    });
    expect(shared.status()).toBe(204);
    const second = await viewer.newPage();
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    const savedBefore = await (await page.request.get(`/api/documents/${documentId}`)).json();
    const grid = page.getByRole('region', { name: 'Область таблицы' });
    await grid.focus();
    await page.keyboard.press('ArrowRight');
    await expect(second.getByTestId('remote-presence')).toHaveText('Вместе в таблице: 2');
    const savedAfter = await (await page.request.get(`/api/documents/${documentId}`)).json();
    expect(savedAfter.version).toBe(savedBefore.version);
    await page.close();
    await expect(second.getByTestId('remote-presence')).toHaveCount(0);
  } finally {
    await viewer.request
      .delete(origin + '/api/dev/session', { headers: { Origin: origin } })
      .catch(() => undefined);
    await viewer.close();
  }
});
