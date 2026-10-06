import { expect, test, type Page } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';
import { GRID } from '../src/grid/viewport';

const recognitionTypes = ['Телефоны РФ', 'Паспорта', 'СНИЛС', 'ИНН', 'Email', 'Банковские карты'];

async function fixture(page: Page) {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
}

async function privacyDialog(page: Page) {
  await (await editorButton(page, 'Персональные данные')).click();
  const dialog = page.getByRole('dialog', { name: 'Персональные данные', exact: true });
  await expect(dialog.getByRole('checkbox', { name: 'Включить распознавание' })).toBeVisible();
  return dialog;
}

async function syntheticEmail(page: Page) {
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + GRID.columnWidth / 2,
      y: GRID.columnHeaderHeight + GRID.rowHeight / 2,
    },
  });
  const bar = page.getByRole('textbox', { name: 'Значение ячейки', exact: true });
  await bar.fill('long.synthetic.address@example.invalid');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const dialog = await privacyDialog(page);
  await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).check();
  await dialog.getByRole('checkbox', { name: 'Email', exact: true }).check();
  await dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }).click();
  await expect(dialog.getByRole('status')).toContainText('Совпадений: 1');
  await page.keyboard.press('Escape');
}

test('cell edits keep the last safe masked view while analysis refreshes', async ({ page }) => {
  await fixture(page);
  await syntheticEmail(page);
  await expect(page.getByText('Обновляем фильтр ПД…', { exact: true })).toBeHidden();
  let releaseAnalysis: (() => void) | undefined;
  const analysisHeld = new Promise<void>((resolve) => {
    releaseAnalysis = resolve;
  });
  await page.route('**/privacy/analyze', async (route) => {
    await analysisHeld;
    await route.continue();
  });
  const id = new URL(page.url()).searchParams.get('document');
  const original = await (await page.request.get(`/api/documents/${id}`)).json();
  const before = original.version;
  const address = page.getByRole('textbox', { name: 'Ячейка или диапазон', exact: true });
  await address.fill('B1');
  await address.press('Enter');
  const bar = page.getByRole('textbox', { name: 'Значение ячейки', exact: true });
  await bar.fill('new.synthetic@example.invalid');
  await bar.press('Enter');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe(String(Number(before) + 1));
  await expect(page.getByText('Обновляем фильтр ПД…', { exact: true })).toBeVisible();
  await expect(bar).toHaveValue('');
  await expect(page.getByRole('button', { name: 'Раскрыть данные A1', exact: true })).toBeVisible();
  releaseAnalysis?.();
  await expect(bar).toHaveValue('[СКРЫТО]');
  await expect(page.getByRole('button', { name: 'Раскрыть данные B1', exact: true })).toBeVisible();
});

test('enabling recognition selects every type and persists that selection', async ({ page }) => {
  await fixture(page);
  const dialog = await privacyDialog(page);
  await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).uncheck();
  for (const name of recognitionTypes)
    await dialog.getByRole('checkbox', { name, exact: true }).uncheck();
  await dialog.getByRole('checkbox', { name: 'Включить распознавание' }).check();
  for (const name of recognitionTypes)
    await expect(dialog.getByRole('checkbox', { name, exact: true })).toBeChecked();
  await dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }).click();
  await expect(
    dialog.getByRole('button', { name: 'Сохранить настройки', exact: true }),
  ).toBeEnabled();
  await page.reload();
  const reopened = await privacyDialog(page);
  await expect(reopened.getByRole('checkbox', { name: 'Включить распознавание' })).toBeChecked();
  for (const name of recognitionTypes)
    await expect(reopened.getByRole('checkbox', { name, exact: true })).toBeChecked();
});

test('revealing a long value keeps its veil inside the cell, including partial scrolling', async ({
  page,
}) => {
  await page.setViewportSize({ width: 681, height: 705 });
  await fixture(page);
  await syntheticEmail(page);
  await page.mouse.move(20, 20);
  const veil = page.getByRole('button', { name: 'Раскрыть данные A1', exact: true });
  await expect(veil).toBeVisible();
  const geometry = (target: typeof veil) =>
    target.evaluate((element) => {
      const cell = element.getBoundingClientRect();
      const area = document.querySelector('.grid-scroll')!.getBoundingClientRect();
      return { x: cell.x - area.x, y: cell.y - area.y, width: cell.width, height: cell.height };
    });
  const hidden = await geometry(veil);
  // Keyboard activation keeps the reveal focused even if an async banner
  // changes the viewport origin while policy analysis finishes.
  await veil.press('Enter');
  const revealed = page.getByRole('button', { name: 'Скрыть данные A1', exact: true });
  await expect(revealed.locator('.privacy-veil-value')).toHaveText(
    'long.synthetic.address@example.invalid',
  );
  const visible = await geometry(revealed);
  expect(visible.width).toBeCloseTo(hidden.width, 1);
  expect(visible.height).toBeCloseTo(hidden.height, 1);
  expect(visible.x).toBeCloseTo(hidden.x, 1);
  expect(visible.y).toBeCloseTo(hidden.y, 1);
  await revealed.press('Escape');
  await page.locator('.grid-scroll').focus();
  await page.mouse.move(20, 20);
  await page.locator('.grid-scroll').evaluate((element) => {
    element.scrollLeft = 56;
  });
  const partial = page.getByRole('button', { name: 'Раскрыть данные A1', exact: true });
  await expect.poll(async () => (await partial.boundingBox())?.width).toBeLessThan(60);
  const clipped = await partial.boundingBox();
  await partial.press('Enter');
  const clippedReveal = await page
    .getByRole('button', { name: 'Скрыть данные A1', exact: true })
    .boundingBox();
  expect(clippedReveal).not.toBeNull();
  expect(clippedReveal!.width).toBeCloseTo(clipped!.width, 1);
  expect(clippedReveal!.x).toBeCloseTo(clipped!.x, 1);
});

test('double-clicking the active sheet tab renames the sheet and persists after reload', async ({
  page,
}) => {
  await fixture(page);
  const active = page.locator('.sheet-tab.active');
  await expect(active).toHaveCount(1);
  await active.dblclick();
  const dialog = page.getByRole('dialog', { name: 'Переименовать лист', exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('textbox').fill('Демонстрация — тест');
  await dialog.getByRole('button', { name: 'Применить', exact: true }).click();
  const renamed = page.getByRole('button', {
    name: 'Перейти на лист Демонстрация — тест',
    exact: true,
  });
  await expect(renamed).toBeVisible();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await page.reload();
  await expect(renamed).toBeVisible();
  await expect(page.locator('.sheet-tab.active')).toHaveText('Демонстрация — тест');
});

type DrawCall = {
  kind: 'fill' | 'stroke';
  color: string;
  x: number;
  y: number;
  width: number;
  height: number;
};

test('range selection fills the active cell and column selection does not outline an intersected merge', async ({
  page,
}) => {
  await page.addInitScript(() => {
    type Call = {
      kind: 'fill' | 'stroke';
      color: string;
      x: number;
      y: number;
      width: number;
      height: number;
    };
    type RecordedCanvas = HTMLCanvasElement & { selectionDrawCalls?: Call[] };
    const prototype = CanvasRenderingContext2D.prototype;
    for (const [method, kind] of [
      ['fillRect', 'fill'],
      ['strokeRect', 'stroke'],
    ] as const) {
      const original = prototype[method];
      prototype[method] = function (x, y, width, height) {
        const canvas = this.canvas as RecordedCanvas;
        if (kind === 'fill' && x === 0 && y === 0 && width > 200 && height > 200)
          canvas.selectionDrawCalls = [];
        const calls = (canvas.selectionDrawCalls ??= []);
        if (calls.length < 1_024)
          calls.push({
            kind,
            color: String(kind === 'fill' ? this.fillStyle : this.strokeStyle),
            x,
            y,
            width,
            height,
          });
        original.call(this, x, y, width, height);
      };
    }
  });
  await fixture(page);
  const area = page.getByRole('region', { name: 'Область таблицы' });
  const calls = () =>
    page
      .locator('canvas')
      .first()
      .evaluate(
        (canvas) =>
          (canvas as HTMLCanvasElement & { selectionDrawCalls?: DrawCall[] }).selectionDrawCalls ??
          [],
      );
  await area.click({
    position: {
      x: GRID.rowHeaderWidth + GRID.columnWidth / 2,
      y: GRID.columnHeaderHeight + GRID.rowHeight / 2,
    },
  });
  for (let i = 0; i < 4; i++) await page.keyboard.press('Shift+ArrowRight');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено A1:E1');
  await page.getByRole('button', { name: 'Объединить ячейки', exact: true }).click();
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await area.click({
    position: { x: GRID.rowHeaderWidth + GRID.columnWidth * 1.5, y: GRID.columnHeaderHeight / 2 },
  });
  await expect(page.getByTestId('selection-range')).toContainText('B1:B1000');
  await expect
    .poll(
      async () =>
        (await calls()).filter(
          (call) =>
            call.kind === 'stroke' &&
            call.color === '#17664b' &&
            call.width > GRID.columnWidth * 4 &&
            call.height < GRID.rowHeight,
        ).length,
    )
    .toBe(0);
  await area.click({
    position: {
      x: GRID.rowHeaderWidth + GRID.columnWidth * 2.5,
      y: GRID.columnHeaderHeight + GRID.rowHeight * 7.5,
    },
  });
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('Shift+ArrowDown');
  await page.keyboard.press('Shift+ArrowDown');
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено C8:D10');
  await expect
    .poll(
      async () =>
        (await calls()).filter((call) => call.kind === 'stroke' && call.color === '#17664b').length,
    )
    .toBe(1);
  const draw = await calls();
  expect(
    draw.some(
      (call) =>
        call.kind === 'fill' &&
        call.width === GRID.columnWidth - 2 &&
        call.height === GRID.rowHeight - 2 &&
        call.color === 'rgba(35, 143, 98, 0.18)',
    ),
  ).toBe(true);
  expect(draw.find((call) => call.kind === 'stroke' && call.color === '#17664b')).toMatchObject({
    width: GRID.columnWidth * 2 - 2,
    height: GRID.rowHeight * 3 - 2,
  });
});

test('privacy overlay and canvas stay in the same scroll frame and selected masks stay green', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const original = CanvasRenderingContext2D.prototype.fillRect;
    CanvasRenderingContext2D.prototype.fillRect = function (x, y, width, height) {
      if (
        x === 0 &&
        y === 0 &&
        width > 200 &&
        height > 200 &&
        this.canvas.classList.contains('grid-canvas')
      ) {
        const scroller = document.querySelector('.grid-scroll');
        if (scroller) {
          this.canvas.dataset.testPaintedLeft = String(scroller.scrollLeft);
          this.canvas.dataset.testPaintedTop = String(scroller.scrollTop);
        }
      }
      original.call(this, x, y, width, height);
    };
  });
  await page.setViewportSize({ width: 681, height: 705 });
  await fixture(page);
  await syntheticEmail(page);
  const area = page.getByRole('region', { name: 'Область таблицы' });
  await area.click({
    position: { x: GRID.rowHeaderWidth + GRID.columnWidth / 2, y: GRID.columnHeaderHeight / 2 },
  });
  await page.mouse.move(20, 20);
  const veil = page.locator('.privacy-veil');
  await expect(veil).toHaveClass(/privacy-veil-selected/);
  await expect(veil).toHaveCSS('background-color', 'rgb(207, 229, 217)');
  for (const left of [20, 40, 60, 80, 60, 40, 20, 80]) {
    const frame = await page.evaluate(async (offset) => {
      const scroller = document.querySelector('.grid-scroll');
      if (!scroller) throw new Error('Missing grid scroller');
      scroller.scrollLeft = offset;
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      );
      const canvas = document.querySelector<HTMLCanvasElement>('.grid-canvas');
      const veil = document.querySelector<HTMLElement>('.privacy-veil');
      const content = veil?.querySelector<HTMLElement>('.privacy-veil-content');
      if (!canvas || !veil || !content) throw new Error('Missing grid paint or privacy overlay');
      const area = scroller.getBoundingClientRect();
      const bounds = veil.getBoundingClientRect();
      return {
        paintedLeft: Number(canvas.dataset.testPaintedLeft),
        paintedTop: Number(canvas.dataset.testPaintedTop),
        areaX: area.x,
        areaY: area.y,
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        contentX: content.getBoundingClientRect().x,
      };
    }, left);
    expect(frame.paintedLeft).toBe(left);
    expect(frame.paintedTop).toBe(0);
    expect(frame.contentX).toBeCloseTo(
      frame.areaX + GRID.rowHeaderWidth + 1 - frame.paintedLeft,
      1,
    );
    expect(frame.x).toBeCloseTo(frame.areaX + GRID.rowHeaderWidth, 1);
    expect(frame.width).toBeCloseTo(GRID.columnWidth - 1 - frame.paintedLeft, 1);
    expect(frame.y).toBeCloseTo(frame.areaY + GRID.columnHeaderHeight + 1, 1);
  }
  await expect(veil).toHaveCSS('background-color', 'rgb(207, 229, 217)');
});
