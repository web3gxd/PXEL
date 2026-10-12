import { editorButton } from './editor-menu';
import { createBookFromUi } from './create-book';
import { cutSelection } from './clipboard';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { GRID } from '../src/grid/viewport';
import { uuidV7, type Projection } from '../src/document/store';

const grid = (page: Page) => page.getByRole('region', { name: 'Область таблицы' });
const bar = (page: Page) => page.getByRole('textbox', { name: 'Значение ячейки' });
const undo = (page: Page) => page.getByRole('button', { name: 'Отменить последнюю правку' });
const redo = (page: Page) => page.getByRole('button', { name: 'Повторить отменённую правку' });
async function select(page: Page, row: number, column: number) {
  await grid(page).click({
    position: {
      x: GRID.rowHeaderWidth + (column + 0.5) * GRID.columnWidth,
      y: GRID.columnHeaderHeight + (row + 0.5) * GRID.rowHeight,
    },
  });
}
async function write(page: Page, row: number, column: number, value: string) {
  await select(page, row, column);
  await bar(page).fill(value);
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
async function createDocument(page: Page) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await createBookFromUi(page);
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await createBookFromUi(page);
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
}
async function reload(page: Page, direction: 'undo' | 'redo') {
  await page.reload();
  await expect(page.getByTestId('connection-status')).toHaveText('Подключено');
  await expect(direction === 'undo' ? undo(page) : redo(page)).toBeEnabled();
}
async function inverse(page: Page, direction: 'undo' | 'redo', version: string) {
  const button = direction === 'undo' ? undo(page) : redo(page);
  await expect(button).toBeEnabled();
  await button.click();
  await expect.poll(async () => (await state(page)).version).toBe(String(BigInt(version) + 1n));
  await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
  return state(page);
}

for (const family of ['scalar', 'formula', 'range', 'move', 'axis'] as const) {
  test(`discovers ${family} history after original, undo and redo reloads`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await createDocument(page);
    if (family === 'move' || family === 'axis') {
      await write(page, 0, 0, '7');
      await write(page, 0, 1, '=A1+1');
    }
    const original = await state(page);
    if (family === 'scalar' || family === 'formula') {
      await write(page, 0, 0, family === 'scalar' ? '42' : '=3+4');
    } else if (family === 'range') {
      await select(page, 0, 0);
      await page.evaluate(() => navigator.clipboard.writeText('4\tTRUE\n7\tvalue'));
      await page.keyboard.press('ControlOrMeta+V');
      await expect(page.getByTestId('active-cell')).toHaveText('A1: 4');
      await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    } else if (family === 'move') {
      await select(page, 0, 0);
      await cutSelection(page);
      await expect(page.getByTestId('cut-status')).toContainText('Исходные ячейки пока сохранены');
      await select(page, 0, 2);
      await page.keyboard.press('ControlOrMeta+V');
      await expect(page.getByTestId('cut-status')).toHaveCount(0);
      await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    } else {
      await select(page, 0, 0);
      await (await editorButton(page, 'Строка −')).click();
      await expect(page.getByTestId('save-status')).toHaveText('Сохранено');
    }
    const changed = await state(page);
    expect(BigInt(changed.version)).toBe(BigInt(original.version) + 1n);
    await reload(page, 'undo');
    const undone = await inverse(page, 'undo', changed.version);
    expect(undone.sheets).toEqual(original.sheets);
    await reload(page, 'redo');
    const redone = await inverse(page, 'redo', undone.version);
    expect(redone.sheets).toEqual(changed.sheets);
    await reload(page, 'undo');
    expect(await state(page)).toEqual(redone);
    await expect(redo(page)).toBeDisabled();
    expect(errors).toEqual([]);
  });
}

test('shares one author history across tabs and clears restored redo after a new committed branch', async ({
  page,
}) => {
  await createDocument(page);
  const second = await page.context().newPage();
  try {
    await second.goto(page.url());
    await expect(second.getByTestId('connection-status')).toHaveText('Подключено');
    await page.bringToFront();
    await write(page, 0, 0, '11');
    const first = await state(page);
    await expect(undo(second)).toBeEnabled();
    await second.bringToFront();
    const undone = await inverse(second, 'undo', first.version);
    expect(undone.sheets[0]?.cells).toEqual([]);
    await expect(redo(page)).toBeEnabled();
    await reload(second, 'redo');
    await write(second, 1, 1, '22');
    const branch = await state(second);
    await reload(second, 'undo');
    await expect(redo(second)).toBeDisabled();
    await expect(redo(page)).toBeDisabled();
    const branchUndone = await inverse(second, 'undo', branch.version);
    expect(branchUndone.sheets[0]?.cells).toEqual([]);
    await reload(page, 'redo');
    expect(await state(page)).toEqual(branchUndone);
  } finally {
    await second.close();
  }
});

async function editorFor(page: Page, browser: Browser) {
  const origin = new URL(page.url()).origin;
  const document = new URL(page.url()).searchParams.get('document');
  const owner = await (
    await page.request.get('/api/dev/identity', { headers: { Origin: origin } })
  ).json();
  const context = await browser.newContext();
  const account = `history_${crypto.randomUUID().replaceAll('-', '').slice(0, 20)}`;
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
    const editor = await (
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
            target_user_id: editor.actor_id,
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
    const editorPage = await context.newPage();
    await editorPage.goto(page.url());
    await expect(editorPage.getByTestId('connection-status')).toHaveText('Подключено');
    return { editor: editorPage, close };
  } catch (error) {
    await close();
    throw error;
  }
}

test('restored own history still refuses a foreign same-value overwrite without a new version', async ({
  page,
  browser,
}) => {
  await createDocument(page);
  await write(page, 0, 0, '7');
  const { editor, close } = await editorFor(page, browser);
  try {
    await expect(undo(editor)).toBeDisabled();
    await editor.bringToFront();
    const beforeOverwrite = await state(editor);
    await select(editor, 0, 0);
    // Force an input change before restoring the same persisted value.
    await bar(editor).fill('');
    await bar(editor).fill('7');
    await bar(editor).press('Enter');
    await expect
      .poll(async () => (await state(editor)).version)
      .toBe(String(BigInt(beforeOverwrite.version) + 1n));
    await expect(editor.getByTestId('save-status')).toHaveText('Сохранено');
    const canonical = await state(editor);
    expect(canonical.sheets).toEqual(beforeOverwrite.sheets);
    expect(canonical.capabilities).toMatchObject({ can_edit: true, can_share: false });
    await page.bringToFront();
    await reload(page, 'undo');
    await undo(page).click();
    await expect(page.getByRole('alert')).toContainText('Отмена невозможна');
    await expect(undo(page)).toBeDisabled();
    const ownerState = await state(page);
    expect(ownerState.capabilities).toMatchObject({ can_edit: true, can_share: true });
    expect(workbookState(ownerState)).toEqual(workbookState(canonical));
    // A refusal is not durable history: reload rediscovers the guarded candidate.
    await reload(page, 'undo');
    const reloadedOwnerState = await state(page);
    expect(reloadedOwnerState.capabilities).toMatchObject({ can_edit: true, can_share: true });
    expect(workbookState(reloadedOwnerState)).toEqual(workbookState(canonical));
  } finally {
    await close();
  }
});
