import { editorButton } from './editor-menu';
import { createBookFromUi } from './create-book';
import { copySelection, readClipboardText } from './clipboard';
import { expect, test } from '@playwright/test';

async function createDocument(page: import('@playwright/test').Page) {
  await createBookFromUi(page);
  await expect
    .poll(async () => {
      const id = new URL(page.url()).searchParams.get('document');
      if (!id) return false;
      const response = await page.request.get(`/api/documents/${id}`);
      return response.ok() && (await response.json()).sheets?.length > 0;
    })
    .toBe(true);
  await expect(page.getByRole('region', { name: 'Лист 1' })).toBeVisible();
}

test('quoted multiline cells paste and copy as one rectangular HTML/TSV range', async ({
  page,
}) => {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData(
      'text/plain',
      '"Вопрос\r\nСтрока 1\r\nСтрока 2"\t"Ответ\nПункт 1\nПункт 2"',
    );
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: Вопрос\nСтрока 1\nСтрока 2');
  await grid.focus();
  await page.keyboard.press('Shift+ArrowRight');
  await copySelection(page);
  expect(await readClipboardText(page)).toBe(
    '"Вопрос\nСтрока 1\nСтрока 2"\t"Ответ\nПункт 1\nПункт 2"',
  );
  const formats = await page.evaluate(async () =>
    (await navigator.clipboard.read()).flatMap((item) => item.types),
  );
  expect(formats).toContain('text/html');
});

test('clicking a column header selects the full column', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const drawsBefore = Number(await page.locator('canvas').getAttribute('data-canvas-draw-calls'));
  await grid.click({ position: { x: 46 + 112 + 56, y: 14 } });
  await expect(page.getByTestId('selection-range')).toHaveText('Выделено B1:B1000');
  await expect(page.getByTestId('active-cell')).toHaveText('B1: ');
  await expect
    .poll(async () => Number(await page.locator('canvas').getAttribute('data-canvas-draw-calls')))
    .toBeGreaterThan(drawsBefore);
});

test('row heights and column widths resize, persist, and can be entered accessibly', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const box = await grid.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.move(box!.x + 46 + 112, box!.y + 14);
  await page.mouse.down();
  await page.mouse.move(box!.x + 46 + 132, box!.y + 14);
  await page.mouse.up();
  await expect(page.locator('.grid-size-tooltip')).toHaveCount(0);
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0].column_widths?.[0]
          ?.size_css_px,
    )
    .toBe(132);

  const afterWidth = await grid.boundingBox();
  expect(afterWidth).not.toBeNull();
  await page.mouse.move(afterWidth!.x + 20, afterWidth!.y + 28 + 28);
  await page.mouse.down();
  await page.mouse.move(afterWidth!.x + 20, afterWidth!.y + 28 + 42);
  await page.mouse.up();
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0].row_heights?.[0]
          ?.size_css_px,
    )
    .toBe(42);

  await (await editorButton(page, 'Размеры')).click();
  const panel = page.getByRole('dialog', { name: 'Размеры ячеек' });
  await expect(panel).toBeVisible();
  await panel.getByRole('spinbutton', { name: 'Ширина столбца в пикселях' }).fill('150');
  await panel.getByRole('button', { name: 'Применить' }).nth(1).click();
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0].column_widths?.[0]
          ?.size_css_px,
    )
    .toBe(150);
});

test('smart row scrollbar maps drag positions and keeps keyboard paging linear', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData(
      'text/plain',
      Array.from({ length: 20 }, (_, index) => `row ${index + 1}`).join('\n'),
    );
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });

  const scroller = grid;
  const scrollbar = page.getByRole('scrollbar', { name: 'Прокрутка строк' });
  await expect(scrollbar).toHaveAttribute('data-scroll-mode', 'linear');
  const thumb = await page.getByTestId('smart-scrollbar-thumb-row').boundingBox();
  expect(thumb?.height).toBeGreaterThanOrEqual(32);
  const track = await scrollbar.boundingBox();
  expect(track).not.toBeNull();
  const startY = track!.y + track!.height * 0.15;
  await page.mouse.move(track!.x + track!.width / 2, startY);
  await page.mouse.down();
  await page.mouse.move(track!.x + track!.width / 2, track!.y + track!.height * 0.7, { steps: 4 });
  await expect(page.locator('.smart-scrollbar-tooltip')).toContainText('Строка');
  await page.mouse.up();
  const afterDrag = await scroller.evaluate((element) => element.scrollTop);
  expect(afterDrag).toBeGreaterThan(0);
  await scrollbar.focus();
  const beforePage = await scroller.evaluate((element) => element.scrollTop);
  await page.keyboard.press('PageDown');
  await expect
    .poll(() => scroller.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(beforePage);
  const afterPage = await scroller.evaluate((element) => element.scrollTop);
  expect(afterPage - beforePage).toBeLessThanOrEqual(track!.height);

  const beforeWheel = await scroller.evaluate((element) => element.scrollTop);
  const gridBox = await grid.boundingBox();
  expect(gridBox).not.toBeNull();
  await page.mouse.move(gridBox!.x + 110, gridBox!.y + 120);
  await page.mouse.wheel(0, 123);
  await expect
    .poll(() => scroller.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(beforeWheel);
  const afterWheel = await scroller.evaluate((element) => element.scrollTop);
  expect(afterWheel - beforeWheel).toBe(123);

  const horizontal = page.getByRole('scrollbar', { name: 'Прокрутка столбцов' });
  const horizontalTrack = await horizontal.boundingBox();
  expect(horizontalTrack).not.toBeNull();
  const horizontalThumb = await page.getByTestId('smart-scrollbar-thumb-column').boundingBox();
  expect(horizontalThumb?.width).toBeGreaterThanOrEqual(32);
  await page.mouse.move(horizontalTrack!.x + horizontalTrack!.width * 0.2, horizontalTrack!.y + 6);
  await page.mouse.down();
  await page.mouse.move(horizontalTrack!.x + horizontalTrack!.width * 0.8, horizontalTrack!.y + 6, {
    steps: 4,
  });
  await expect(page.locator('.smart-scrollbar-tooltip')).toContainText('Колонка');
  await page.mouse.up();
  expect(await scroller.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);
});

test('adaptive scrollbar gives a used range finer control and keeps the empty tail reachable', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData(
      'text/plain',
      Array.from({ length: 1_000 }, (_, index) => `data ${index + 1}`).join('\n'),
    );
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('2');
  await grid.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/documents/${id}`)).json()).sheets[0].rows.length,
    )
    .toBeGreaterThan(1_000);

  const scrollbar = page.getByRole('scrollbar', { name: 'Прокрутка строк' });
  await expect(scrollbar).toHaveAttribute('data-scroll-mode', 'adaptive');
  const track = await scrollbar.boundingBox();
  expect(track).not.toBeNull();
  await page.mouse.move(track!.x + track!.width / 2, track!.y + track!.height * 0.99);
  await page.mouse.down();
  await page.mouse.move(track!.x + track!.width / 2, track!.y + track!.height * 0.99, { steps: 3 });
  await expect(page.locator('.smart-scrollbar-tooltip')).toContainText(/Строка [1-9][0-9]{3,}/);
  await page.mouse.up();
  const offset = await grid.evaluate((element) => element.scrollTop);
  expect(offset).toBeGreaterThan(Number(await scrollbar.getAttribute('data-used-scroll-offset')));
  await expect(scrollbar).toHaveAttribute('aria-valuetext', /Строка [1-9][0-9]{3,}/);
});

test('scrollbar mapping stays monotone and reversible for 20, 1000 and 100000 used rows in Chromium', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  const results = await page.evaluate(async () => {
    const moduleUrl = new URL('/src/grid/viewport.ts', location.origin).href;
    const { AxisScrollMapping } = (await import(/* @vite-ignore */ moduleUrl)) as {
      AxisScrollMapping: new (
        totalOffset: number,
        usedOffset: number,
      ) => {
        offsetAt(position: number): number;
        positionAt(offset: number): number;
      };
    };
    return [20, 1_000, 100_000].map((usedRows) => {
      const total = 2_800_000;
      const mapping = new AxisScrollMapping(total, Math.max(0, usedRows * 28 - 560));
      const positions = Array.from({ length: 101 }, (_, index) => index / 100);
      const offsets = positions.map((position) => mapping.offsetAt(position));
      return {
        usedRows,
        monotone: offsets.every((offset, index) => index === 0 || offset >= offsets[index - 1]!),
        reversible: positions.every(
          (position) => Math.abs(mapping.positionAt(mapping.offsetAt(position)) - position) < 1e-9,
        ),
        start: offsets[0],
        end: offsets.at(-1),
      };
    });
  });
  for (const result of results) {
    expect(result.monotone).toBe(true);
    expect(result.reversible).toBe(true);
    expect(result.start).toBe(0);
    expect(result.end).toBe(2_800_000);
  }
});

test('Markdown table preview uses the chosen target and commits a single batch', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const markdown = '# Inventory\n\n| Item | Count |\n| --- | --- |\n| Apples | 3 |\n| Pears | 4 |';
  const worker = await page.evaluate(
    (text) =>
      new Promise<{ result?: { tables: unknown[] }; error?: string; filename?: string }>(
        (resolve) => {
          const instance = new Worker(
            new URL('/src/clipboard/markdown.worker.ts', location.origin),
            { type: 'module' },
          );
          instance.onmessage = (event) => {
            instance.terminate();
            resolve(event.data);
          };
          instance.onerror = (event) => {
            instance.terminate();
            resolve({ error: event.message, filename: event.filename });
          };
          instance.postMessage({ id: 1, text });
        },
      ),
    markdown,
  );
  expect(worker).toMatchObject({ result: { tables: [{ number: 1 }] } });
  await grid.focus();
  await grid.evaluate((element, text) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', text);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, markdown);
  const preview = page.getByTestId('markdown-import');
  await expect(preview).toBeVisible();
  await preview.getByRole('textbox', { name: 'Начальная ячейка вставки' }).fill('C3');
  await expect(page.getByTestId('markdown-target-range')).toHaveText('Диапазон: C3:D5');
  await expect(page.locator('.markdown-grid-preview')).toHaveCSS('left', '270px');
  await preview.getByRole('button', { name: 'Вставить таблицу' }).click();
  await expect(preview).toHaveCount(0);
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('2');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.sheets[0].cells).toHaveLength(6);
});

test('GFM alignment imports in the same atomic batch and one undo restores values and styles', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const markdown = '| Left | Center | Right |\n| :--- | :---: | ---: |\n| a | b | c |';
  await grid.focus();
  await grid.evaluate((element, text) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', text);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, markdown);
  const preview = page.getByTestId('markdown-import');
  await expect(preview).toBeVisible();
  await preview.getByRole('button', { name: 'Вставить таблицу' }).click();
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('2');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByRole('button', { name: 'Отменить последнюю правку' })).toBeEnabled();
  const state = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(state.sheets[0].cells).toHaveLength(6);
  expect(
    state.sheets[0].styles.map((cell: { style: { horizontal: string } }) => cell.style.horizontal),
  ).toEqual(['left', 'center', 'right', 'left', 'center', 'right']);
  await page.getByRole('button', { name: 'Отменить последнюю правку' }).click();
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('3');
  const undone = await (await page.request.get(`/api/documents/${id}`)).json();
  expect(undone.sheets[0].cells).toEqual([]);
  expect(undone.sheets[0].styles).toEqual([]);
});

test('multiple Markdown tables can be selected and previewed at a new target', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  const markdown =
    '# Inventory\n\n| Item | Count |\n| --- | --- |\n| Apples | 3 |\n\n## Cities\n\n| City | Code |\n| --- | --- |\n| Oslo | 47 |';
  await grid.focus();
  await grid.evaluate((element, text) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', text);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, markdown);
  const preview = page.getByTestId('markdown-import');
  await expect(preview).toBeVisible();
  await expect(preview.getByRole('button', { name: /Таблица 1/ })).toBeVisible();
  await expect(preview.getByRole('button', { name: /Таблица 2/ })).toContainText('Cities');
  await expect(preview.getByRole('button', { name: 'Вставить весь текст' })).toBeVisible();
  await preview.getByRole('button', { name: /Таблица 2/ }).click();
  await expect(preview.locator('.markdown-preview-heading')).toContainText('Cities');
  await preview.getByRole('textbox', { name: 'Начальная ячейка вставки' }).fill('C4');
  await expect(page.getByTestId('markdown-target-range')).toHaveText('Диапазон: C4:D5');
  await expect(page.locator('.markdown-grid-preview')).toHaveCSS('left', '270px');
  await preview.getByRole('button', { name: 'Вставить таблицу' }).click();
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('2');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.sheets[0].cells).toHaveLength(4);
  expect(
    state.sheets[0].cells.map((cell: { value: { value?: string } }) => cell.value.value),
  ).toEqual(['City', 'Code', 'Oslo', '47']);
});

test('a single formula edit invalidates only its cell and records calculation timings', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('=2+3');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toContainText('5');
  await bar.fill('=3+4');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  await expect(page.getByTestId('active-cell')).toContainText('7');
  await expect
    .poll(async () => page.locator('canvas').getAttribute('data-recalculated-cells'))
    .toBe('1');
  await expect
    .poll(async () => page.locator('canvas').getAttribute('data-dirty-cell-count'))
    .toBe('1');
  expect(Number(await page.locator('canvas').getAttribute('data-last-painted-area'))).toBe(
    112 * 28,
  );
  expect(
    Number(await page.locator('canvas').getAttribute('data-optimistic-update-ms')),
  ).toBeGreaterThanOrEqual(0);
  expect(
    Number(await page.locator('canvas').getAttribute('data-server-confirmation-ms')),
  ).toBeGreaterThan(0);
  expect(
    Number(await page.locator('canvas').getAttribute('data-calculation-patch-ms')),
  ).toBeGreaterThan(0);
});

test('Ctrl+/ opens searchable shortcuts and bindings can be reassigned', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  await page.getByRole('region', { name: 'Область таблицы' }).focus();
  await page.keyboard.press('ControlOrMeta+/');
  const dialog = page.locator('dialog.command-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: 'Найти команду' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Настройки' }).click();
  const assign = dialog.getByRole('button', { name: 'Назначить: Полужирный' });
  await assign.click();
  await page.keyboard.press('Control+Shift+K');
  await expect(
    dialog.locator('.command-row').filter({ hasText: 'Полужирный' }).locator('kbd'),
  ).toHaveText('Ctrl+Shift+K');
});

test('Markdown overwrite requires confirmation and is undoable as one operation', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
  await bar.fill('existing');
  await bar.press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  const markdown = '| Name | Value |\n| --- | --- |\n| Pear | 7 |';
  await grid.evaluate((element, text) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', text);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, markdown);
  const preview = page.getByTestId('markdown-import');
  await expect(
    preview.getByRole('checkbox', { name: 'Разрешить перезапись непустых ячеек' }),
  ).toBeVisible();
  await expect(preview.getByRole('button', { name: 'Вставить таблицу' })).toBeDisabled();
  await preview.getByRole('checkbox', { name: 'Разрешить перезапись непустых ячеек' }).check();
  await preview.getByRole('button', { name: 'Вставить таблицу' }).click();
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('3');
  await page.getByRole('button', { name: 'Отменить последнюю правку' }).click();
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('4');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.sheets[0].cells).toHaveLength(1);
  expect(state.sheets[0].cells[0].value).toMatchObject({ type: 'text', value: 'existing' });
});

test('HTML tables take precedence over plain fallback and grid focus has no container outline', async ({
  page,
}) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  expect(await grid.evaluate((element) => getComputedStyle(element).outlineStyle)).toBe('none');
  await grid.evaluate((element) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', 'fallback\tvalue');
    clipboardData.setData(
      'text/html',
      '<table><tr><td>First<br>line</td><td></td></tr><tr><td>😀</td><td>&quot;quoted&quot;</td></tr></table>',
    );
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  });
  await expect(page.getByTestId('active-cell')).toHaveText('A1: First\nline');
  const id = new URL(page.url()).searchParams.get('document');
  await expect
    .poll(async () => (await (await page.request.get(`/api/documents/${id}`)).json()).version)
    .toBe('2');
  const state = await page.evaluate(
    async (documentId) => (await fetch(`/api/documents/${documentId}`)).json(),
    id,
  );
  expect(state.sheets[0].cells).toHaveLength(3);
  expect(
    state.sheets[0].cells.some(
      (cell: { value: { value?: string } }) => cell.value.value === 'fallback',
    ),
  ).toBe(false);
  expect(
    state.sheets[0].cells.some((cell: { value: { value?: string } }) => cell.value.value === '😀'),
  ).toBe(true);
});

test('ordinary Markdown including fenced code stays together in one cell', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.focus();
  const text = '# Notes\n\nParagraph\n\n- first\n- second\n\n```text\na | b\t c\n```';
  await grid.evaluate((element, value) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/plain', value);
    element.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }),
    );
  }, text);
  await expect
    .poll(
      async () =>
        (
          await (
            await page.request.get(
              `/api/documents/${new URL(page.url()).searchParams.get('document')}`,
            )
          ).json()
        ).version,
    )
    .toBe('2');
  const state = await page.evaluate(async () =>
    (await fetch(`/api/documents/${new URL(location.href).searchParams.get('document')}`)).json(),
  );
  expect(state.sheets[0].cells).toHaveLength(1);
  expect(state.sheets[0].cells[0].value).toMatchObject({ type: 'text', value: text });
});

test('dropping a Markdown file opens the table importer', async ({ page }) => {
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createDocument(page);
  const grid = page.getByRole('region', { name: 'Область таблицы' });
  await grid.evaluate((element) => {
    const transfer = new DataTransfer();
    transfer.items.add(
      new File(['| City | Code |\n| --- | --- |\n| Oslo | 47 |'], 'cities.md', {
        type: 'text/markdown',
      }),
    );
    element.dispatchEvent(
      new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: transfer }),
    );
  });
  await expect(page.getByTestId('markdown-import')).toBeVisible();
  await expect(page.getByTestId('markdown-import').getByText('2 строк × 2 столбцов')).toBeVisible();
  await page.getByRole('button', { name: 'Закрыть предпросмотр' }).click();
  await page.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>('input[type="file"]');
    if (!input) throw new Error('Markdown file input missing');
    const file = new File(
      ['# Imported\n\n| Key | Value |\n| --- | --- |\n| A | B |'],
      'import.md',
      { type: 'text/markdown' },
    );
    Object.defineProperty(input, 'files', { configurable: true, value: [file] });
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await expect(page.getByTestId('markdown-import')).toBeVisible();
});
