import { expect, test, type Page } from '@playwright/test';
import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';
import type { Projection, Sheet } from '../src/document/store';
import { GRID } from '../src/grid/viewport';
import { selectValue } from './select';

const failures = new WeakMap<Page, string[]>();
test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  failures.set(page, errors);
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  await page.goto('/');
  await createBookFromUi(page);
});
test.afterEach(async ({ page }) => expect(failures.get(page)).toEqual([]));

async function state(page: Page): Promise<Projection> {
  const id = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.status()).toBe(200);
  return response.json();
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
async function menu(page: Page, ...path: string[]) {
  const trigger = page.getByRole('button', { name: 'Ячейки', exact: true });
  if ((await trigger.getAttribute('aria-expanded')) !== 'true') await trigger.click();
  for (const label of path) {
    const button = page
      .locator('.editor-menu-popup')
      .getByRole('button', { name: label, exact: true });
    await expect(button).toBeEnabled();
    await button.click();
  }
}
async function select(page: Page, row = 0, column = 0) {
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
}
async function write(page: Page, row: number, column: number, text: string) {
  await select(page, row, column);
  await committed(page, async () => {
    const bar = page.getByRole('textbox', { name: 'Значение ячейки' });
    await bar.fill(text);
    await bar.press('Enter');
  });
}
async function persisted(page: Page, expected: Projection) {
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  const after = await state(page);
  expect(after.version).toBe(expected.version);
  expect(after.sheets).toEqual(expected.sheets);
  expect(after.state_hash).toBe(expected.state_hash);
  await expect(page.locator('canvas')).toHaveAttribute(
    'data-confirmed-state-hash',
    after.state_hash,
  );
  await expect(page.locator('canvas')).toHaveAttribute(
    'data-preview-authored-hash',
    after.state_hash,
  );
}
function cell(sheet: Sheet, row: number, column: number) {
  return sheet.cells.find(
    (entry) => entry.row_id === sheet.rows[row] && entry.column_id === sheet.columns[column],
  )?.value;
}
const undo = (page: Page) => page.getByRole('button', { name: 'Отменить последнюю правку' });

test('both autofits use fresh websocket writes without a metadata refresh', async ({ page }) => {
  await write(page, 0, 0, 'Fresh websocket text needing a wider column');
  let changed = await committed(page, () =>
    menu(page, 'Форматировать', 'Автоподбор ширины столбца'),
  );
  expect(changed.sheets[0]?.column_widths?.[0]?.size_css_px).toBeGreaterThan(GRID.columnWidth);
  await write(page, 1, 0, 'first line\nsecond line');
  changed = await committed(page, () => menu(page, 'Форматировать', 'Автоподбор высоты строки'));
  expect(changed.sheets[0]?.row_heights?.[0]?.size_css_px).toBeGreaterThan(GRID.rowHeight);
  await persisted(page, changed);
});
test('default width preserves explicit overrides and both autofits use authored text', async ({
  page,
}) => {
  await write(page, 0, 0, 'A very long authored text for measuring the actual column width');
  await select(page, 0, 1);
  await menu(page, 'Форматировать', 'Ширина столбца…');
  const sizes = page.getByRole('dialog', { name: 'Размеры ячеек' });
  await sizes.getByRole('spinbutton', { name: 'Ширина столбца в пикселях' }).fill('211');
  await committed(page, () =>
    sizes
      .locator('label')
      .filter({ has: page.getByRole('spinbutton', { name: 'Ширина столбца в пикселях' }) })
      .getByRole('button', { name: 'Применить', exact: true })
      .click(),
  );
  const override = (await state(page)).sheets[0]?.column_widths;
  await menu(page, 'Форматировать', 'Ширина по умолчанию…');
  const defaults = page.getByRole('dialog', { name: 'Ширина по умолчанию' });
  await defaults.getByRole('spinbutton').fill('140');
  let changed = await committed(page, () =>
    defaults.getByRole('button', { name: 'Применить', exact: true }).click(),
  );
  expect(changed.sheets[0]?.default_column_width).toBe(140);
  expect(changed.sheets[0]?.column_widths).toEqual(override);
  await select(page);
  changed = await committed(page, () => menu(page, 'Форматировать', 'Автоподбор ширины столбца'));
  const width = changed.sheets[0]?.column_widths?.find(
    (entry) => entry.axis_id === changed.sheets[0]?.columns[0],
  )?.size_css_px;
  expect(width).toBeGreaterThan(140);
  await write(page, 0, 0, 'line one\nline two\nline three');
  changed = await committed(page, () => menu(page, 'Форматировать', 'Автоподбор высоты строки'));
  const height = changed.sheets[0]?.row_heights?.find(
    (entry) => entry.axis_id === changed.sheets[0]?.rows[0],
  )?.size_css_px;
  expect(height).toBeGreaterThan(GRID.rowHeight);
  await persisted(page, changed);
});

test('sheet rename, RGB tab color, hide and show persist with authored hash', async ({ page }) => {
  await menu(page, 'Форматировать', 'Управление листами', 'Переименовать лист…');
  const rename = page.getByRole('dialog', { name: 'Переименовать лист' });
  await rename.getByRole('textbox').fill('Отчёт');
  let changed = await committed(page, () =>
    rename.getByRole('button', { name: 'Применить', exact: true }).click(),
  );
  const original = changed.sheets[0]!;
  expect(original.name).toBe('Отчёт');
  await expect(
    page.getByRole('button', { name: 'Перейти на лист Отчёт', exact: true }),
  ).toBeVisible();
  await menu(page, 'Форматировать', 'Управление листами', 'Цвет вкладки…');
  const color = page.getByRole('dialog', { name: 'Цвет вкладки' });
  await color.getByLabel('Цвет вкладки').fill('#2f8ac4');
  changed = await committed(page, () =>
    color.getByRole('button', { name: 'Применить', exact: true }).click(),
  );
  expect(changed.sheets.find((sheet) => sheet.id === original.id)?.tab_color).toBe(0x2f8ac4);
  await persisted(page, changed);
  changed = await committed(page, () => menu(page, 'Добавить', 'Добавить лист'));
  expect(changed.sheets).toHaveLength(2);
  await page.getByRole('button', { name: 'Перейти на лист Отчёт', exact: true }).click();
  changed = await committed(page, () => menu(page, 'Форматировать', 'Видимость', 'Скрыть лист'));
  expect(changed.sheets.find((sheet) => sheet.id === original.id)?.hidden).toBe(true);
  await expect(
    page.getByRole('button', { name: 'Перейти на лист Отчёт', exact: true }),
  ).toHaveCount(0);
  await persisted(page, changed);
  await menu(page, 'Форматировать', 'Видимость', 'Показать лист…');
  const show = page.getByRole('dialog', { name: 'Показать лист' });
  await selectValue(show.getByRole('combobox'), original.id);
  changed = await committed(page, () =>
    show.getByRole('button', { name: 'Применить', exact: true }).click(),
  );
  expect(changed.sheets.find((sheet) => sheet.id === original.id)?.hidden).toBe(false);
  await page.getByRole('button', { name: 'Перейти на лист Отчёт', exact: true }).click();
  await menu(page, 'Форматировать', 'Управление листами', 'Цвет вкладки…');
  changed = await committed(page, () =>
    page
      .getByRole('dialog', { name: 'Цвет вкладки' })
      .getByRole('button', { name: 'Без цвета', exact: true })
      .click(),
  );
  expect(changed.sheets.find((sheet) => sheet.id === original.id)?.tab_color ?? null).toBeNull();
  await persisted(page, changed);
});

test('sheet copy preserves values, bound formula and styles and move changes ordering once', async ({
  page,
}) => {
  await write(page, 0, 0, '7');
  await write(page, 0, 1, '=A1+1');
  await select(page, 0, 1);
  await committed(page, () =>
    page.getByRole('button', { name: 'Полужирный', exact: true }).click(),
  );
  const original = (await state(page)).sheets[0]!;
  await menu(page, 'Форматировать', 'Управление листами', 'Переместить или скопировать лист…');
  const copy = page.getByRole('dialog', { name: 'Переместить или скопировать лист' });
  await copy.getByRole('checkbox', { name: 'Создать копию', exact: true }).check();
  await selectValue(copy.getByRole('combobox', { name: 'Позиция листа', exact: true }), '1');
  await copy.getByRole('textbox', { name: 'Имя копии', exact: true }).fill('Копия');
  let changed = await committed(page, () =>
    copy.getByRole('button', { name: 'Применить', exact: true }).click(),
  );
  const duplicate = changed.sheets.find((sheet) => sheet.id !== original.id)!;
  expect(duplicate.name).toBe('Копия');
  expect(cell(duplicate, 0, 0)).toEqual(cell(original, 0, 0));
  expect(cell(duplicate, 0, 1)?.type).toBe('formula');
  expect(duplicate.styles).toHaveLength(original.styles?.length ?? 0);
  expect(duplicate.styles?.[0]?.style).toEqual(original.styles?.[0]?.style);
  await page.getByRole('button', { name: 'Перейти на лист Копия', exact: true }).click();
  await select(page, 0, 1);
  await expect(page.getByTestId('active-cell')).toHaveText('B1: 8');
  const peer = await page.context().newPage();
  peer.on('pageerror', (error) => failures.get(page)?.push(error.message));
  peer.on('console', (message) => {
    if (message.type() === 'error') failures.get(page)?.push(message.text());
  });
  try {
    await peer.goto(page.url());
    await expect(peer.getByTestId('connection-status')).toHaveText('Подключено');
    await menu(page, 'Форматировать', 'Управление листами', 'Переместить или скопировать лист…');
    const move = page.getByRole('dialog', { name: 'Переместить или скопировать лист' });
    await selectValue(move.getByRole('combobox', { name: 'Позиция листа', exact: true }), '0');
    changed = await committed(page, () =>
      move.getByRole('button', { name: 'Применить', exact: true }).click(),
    );
    expect(changed.sheets.map((sheet) => sheet.id)).toEqual([duplicate.id, original.id]);
    await expect(peer.locator('.sheet-tab')).toHaveCount(2);
    await expect
      .poll(async () =>
        peer
          .locator('.sheet-tab')
          .evaluateAll((tabs) => tabs.map((tab) => tab.getAttribute('aria-label'))),
      )
      .toEqual(['Перейти на лист Копия', `Перейти на лист ${original.name}`]);
    await expect(peer.locator('canvas')).toHaveAttribute(
      'data-confirmed-state-hash',
      changed.state_hash,
    );
    await expect(peer.locator('canvas')).toHaveAttribute(
      'data-preview-authored-hash',
      changed.state_hash,
    );
    await persisted(page, changed);
    await (await editorButton(page, 'Журнал и версии')).click();
    const history = page.getByRole('dialog', { name: 'Журнал и версии' });
    await expect(history).toContainText('Настройки листа');
    await expect(history).toContainText('Копирование листа');
    await expect(history.getByRole('alert')).toHaveCount(0);
    await history.getByRole('button', { name: 'Закрыть окно', exact: true }).click();
  } finally {
    await peer.close();
  }
});

test('row and column hiding retains stable axes and uses visible canvas geometry', async ({
  page,
}) => {
  await write(page, 1, 1, 'visible target');
  await select(page);
  const before = await state(page);
  let changed = await committed(page, () =>
    menu(page, 'Форматировать', 'Видимость', 'Скрыть строки'),
  );
  expect(changed.sheets[0]?.rows).toEqual(before.sheets[0]?.rows);
  expect(changed.sheets[0]?.hidden_rows).toContain(before.sheets[0]?.rows[0]);
  await persisted(page, changed);
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + 1.5 * GRID.columnWidth,
      y: GRID.columnHeaderHeight + 0.5 * GRID.rowHeight,
    },
  });
  await expect(page.getByTestId('active-cell')).toContainText('B2: visible target');
  changed = await committed(page, () =>
    menu(page, 'Форматировать', 'Видимость', 'Показать строки'),
  );
  expect(changed.sheets[0]?.hidden_rows ?? []).toEqual([]);
  await select(page);
  changed = await committed(page, () => menu(page, 'Форматировать', 'Видимость', 'Скрыть столбцы'));
  expect(changed.sheets[0]?.columns).toEqual(before.sheets[0]?.columns);
  expect(changed.sheets[0]?.hidden_columns).toContain(before.sheets[0]?.columns[0]);
  await persisted(page, changed);
  await page.getByRole('region', { name: 'Область таблицы' }).click({
    position: {
      x: GRID.rowHeaderWidth + 0.5 * GRID.columnWidth,
      y: GRID.columnHeaderHeight + 1.5 * GRID.rowHeight,
    },
  });
  await expect(page.getByTestId('active-cell')).toContainText('B2: visible target');
  changed = await committed(page, () =>
    menu(page, 'Форматировать', 'Видимость', 'Показать столбцы'),
  );
  expect(changed.sheets[0]?.hidden_columns ?? []).toEqual([]);
  await persisted(page, changed);
});

for (const axis of ['column', 'row'] as const) {
  test(`${axis} cell insertion and deletion preserve styles and Undo after reload`, async ({
    page,
  }) => {
    const vertical = axis === 'row';
    // Reserve one write for the inbound formula within the 1000-write stripe budget.
    const startRow = vertical ? 1 : 0;
    await write(page, startRow, 0, '7');
    await write(page, vertical ? startRow + 1 : 0, vertical ? 0 : 1, '9');
    const formulaRow = vertical ? 0 : 3,
      formulaColumn = vertical ? 3 : 0;
    await write(page, formulaRow, formulaColumn, vertical ? '=A3+1' : '=B1+1');
    await select(page, startRow);
    await committed(page, () =>
      page.getByRole('button', { name: 'Полужирный', exact: true }).click(),
    );
    const before = await state(page);
    await menu(page, 'Добавить', 'Вставить ячейки…');
    const dialog = page.getByRole('dialog', { name: 'Вставить ячейки' });
    await selectValue(dialog.getByRole('combobox', { name: 'Сдвиг ячеек', exact: true }), axis);
    let changed = await committed(page, () =>
      dialog.getByRole('button', { name: 'Вставить', exact: true }).click(),
    );
    expect(cell(changed.sheets[0]!, startRow, 0)).toBeUndefined();
    expect(cell(changed.sheets[0]!, vertical ? startRow + 1 : 0, vertical ? 0 : 1)).toEqual(
      cell(before.sheets[0]!, startRow, 0),
    );
    expect(changed.sheets[0]?.styles).toHaveLength(1);
    expect(changed.sheets[0]?.styles?.[0]?.row_id).toBe(
      changed.sheets[0]?.rows[vertical ? startRow + 1 : 0],
    );
    expect(changed.sheets[0]?.styles?.[0]?.column_id).toBe(
      changed.sheets[0]?.columns[vertical ? 0 : 1],
    );
    const insertedFormula = cell(changed.sheets[0]!, formulaRow, formulaColumn);
    if (insertedFormula?.type !== 'formula') throw new Error('Missing shifted inbound formula');
    expect(insertedFormula.value.current_text).toContain(vertical ? 'A4' : 'C1');
    await select(page, formulaRow, formulaColumn);
    await expect(page.getByTestId('active-cell')).toHaveText(`${vertical ? 'D1' : 'A4'}: 10`);
    await persisted(page, changed);
    await committed(page, () => undo(page).click());
    expect((await state(page)).sheets).toEqual(before.sheets);
    await select(page, startRow);
    await menu(page, 'Удалить', 'Удалить ячейки…');
    const deletion = page.getByRole('dialog', { name: 'Удалить ячейки' });
    await selectValue(deletion.getByRole('combobox', { name: 'Сдвиг ячеек', exact: true }), axis);
    changed = await committed(page, () =>
      deletion.getByRole('button', { name: 'Удалить', exact: true }).click(),
    );
    expect(cell(changed.sheets[0]!, startRow, 0)).toEqual(
      cell(before.sheets[0]!, vertical ? startRow + 1 : 0, vertical ? 0 : 1),
    );
    expect(changed.sheets[0]?.styles ?? []).toHaveLength(0);
    const deletedFormula = cell(changed.sheets[0]!, formulaRow, formulaColumn);
    if (deletedFormula?.type !== 'formula') throw new Error('Missing shifted inbound formula');
    expect(deletedFormula.value.current_text).toContain(vertical ? 'A2' : 'A1');
    await persisted(page, changed);
    const restored = await committed(page, () => undo(page).click());
    expect(restored.sheets).toEqual(before.sheets);
  });
}
