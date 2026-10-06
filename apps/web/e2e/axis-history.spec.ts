import { createBookFromUi } from './create-book';
import { editorButton } from './editor-menu';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { GRID, columnLabel } from '../src/grid/viewport';
import { uuidV7, type Projection } from '../src/document/store';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const active = (page: Page) => page.getByTestId('active-cell');
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
  await expect(active(page)).toContainText(`${columnLabel(column)}${row + 1}:`);
}
async function write(page: Page, row: number, column: number, text: string) {
  await select(page, row, column);
  await bar(page).fill(text);
  await bar(page).press('Enter');
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function state(page: Page): Promise<Projection> {
  const id = new URL(page.url()).searchParams.get('document');
  const response = await page.request.get(`/api/documents/${id}`);
  expect(response.ok()).toBe(true);
  return response.json();
}
function workbookState(projection: Projection) {
  const result = { ...projection };
  delete result.capabilities;
  return result;
}
async function editorFor(page: Page, browser: Browser) {
  const origin = new URL(page.url()).origin;
  const id = new URL(page.url()).searchParams.get('document');
  const owner = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const context = await browser.newContext();
  const account = `axis_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
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
    const editorIdentity = await (
      await context.request.get(origin + '/api/dev/identity', { headers: { Origin: origin } })
    ).json();
    await context.request.delete(origin + '/api/dev/session', { headers: { Origin: origin } });
    expect(
      (
        await page.request.post(`/api/documents/${id}/permissions`, {
          headers: { Origin: origin },
          data: {
            action: 'grant',
            change_id: uuidV7(),
            target_user_id: editorIdentity.actor_id,
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
    return { editor, close };
  } catch (error) {
    await close();
    throw error;
  }
}

for (const axis of ['row', 'column'] as const) {
  for (const deletion of [false, true]) {
    test(`own ${axis} ${deletion ? 'deletion' : 'insertion'} undo and redo preserve foreign edits, formulas and stable selection`, async ({
      page,
      browser,
    }) => {
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.goto('/');
      await createBookFromUi(page);
      await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
      await createBookFromUi(page);
      await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
      const { editor, close } = await editorFor(page, browser);
      editor.on('pageerror', (error) => errors.push(error.message));
      try {
        await page.bringToFront();
        await write(page, 0, 0, '7');
        await write(page, 1, 1, '=A1+1');
        const before = await state(page);
        const original = before.sheets[0];
        if (!original) throw new Error('Missing initial sheet');
        const formulaCell = original.cells.find(
          (cell) => cell.row_id === original.rows[1] && cell.column_id === original.columns[1],
        );
        if (!formulaCell || formulaCell.value.type !== 'formula')
          throw new Error('Missing initial formula');
        await select(page, 0, 0);
        await (
          await editorButton(
            page,
            `${axis === 'row' ? 'Строка' : 'Столбец'} ${deletion ? '−' : '↑'}`,
          )
        ).click();
        await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
        const changed = await state(page);
        const changedSheet = changed.sheets[0];
        if (!changedSheet) throw new Error('Missing changed sheet');
        expect(BigInt(changed.version)).toBe(BigInt(before.version) + 1n);
        expect(axis === 'row' ? changedSheet.rows.length : changedSheet.columns.length).toBe(
          (axis === 'row' ? original.rows.length : original.columns.length) + (deletion ? -1 : 1),
        );
        const formulaPosition = {
          row: changedSheet.rows.indexOf(formulaCell.row_id),
          column: changedSheet.columns.indexOf(formulaCell.column_id),
        };
        await select(page, formulaPosition.row, formulaPosition.column);
        await expect(active(page)).toHaveText(
          `${columnLabel(formulaPosition.column)}${formulaPosition.row + 1}: ${deletion ? '#REF!' : '8'}`,
        );
        await expect(page.getByRole('button', { name: 'Отменить последнюю правку' })).toBeEnabled();

        await editor.bringToFront();
        await expect.poll(async () => (await state(editor)).version).toBe(changed.version);
        await write(editor, 4, 7, '99');
        const withForeignEdit = await state(editor);
        const foreignCell = withForeignEdit.sheets[0]?.cells.find(
          (cell) => cell.value.type === 'number' && cell.value.value === 99,
        );
        if (!foreignCell) throw new Error('Missing foreign edit');
        await page.bringToFront();
        await grid(page).focus();
        await page.keyboard.press('ControlOrMeta+Z');
        await expect
          .poll(async () => (await state(page)).version)
          .toBe(String(BigInt(withForeignEdit.version) + 1n));
        await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
        const undone = await state(page);
        expect(undone.sheets[0]?.rows).toEqual(original.rows);
        expect(undone.sheets[0]?.columns).toEqual(original.columns);
        expect(undone.sheets[0]?.cells).toEqual(
          expect.arrayContaining([...original.cells, foreignCell]),
        );
        expect(undone.sheets[0]?.cells).toHaveLength(original.cells.length + 1);
        await expect(active(page)).toHaveText('B2: 8');
        await expect(bar(page)).toHaveValue('=A1+1');
        await expect(
          page.getByRole('button', { name: 'Повторить отменённую правку' }),
        ).toBeEnabled();

        await page.keyboard.press('ControlOrMeta+Y');
        await expect
          .poll(async () => (await state(page)).version)
          .toBe(String(BigInt(undone.version) + 1n));
        await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
        const redone = await state(page);
        expect(redone.sheets[0]?.rows).toEqual(changedSheet.rows);
        expect(redone.sheets[0]?.columns).toEqual(changedSheet.columns);
        expect(redone.sheets[0]?.cells).toEqual(
          expect.arrayContaining([...changedSheet.cells, foreignCell]),
        );
        expect(redone.sheets[0]?.cells).toHaveLength(changedSheet.cells.length + 1);
        await expect(active(page)).toHaveText(
          `${columnLabel(formulaPosition.column)}${formulaPosition.row + 1}: ${deletion ? '#REF!' : '8'}`,
        );
        await expect(
          page.getByRole('button', { name: 'Повторить отменённую правку' }),
        ).toBeDisabled();
        await editor.reload();
        await expect(editor.getByTestId('connection-status')).toHaveText('Подключено');
        const editorReloaded = await state(editor);
        expect(editorReloaded.capabilities).toMatchObject({ can_edit: true, can_share: false });
        expect(workbookState(editorReloaded)).toEqual(workbookState(redone));
        await page.reload();
        await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
        expect(workbookState(await state(page))).toEqual(workbookState(redone));
        expect(errors).toEqual([]);
      } finally {
        await close();
      }
    });
  }
}
