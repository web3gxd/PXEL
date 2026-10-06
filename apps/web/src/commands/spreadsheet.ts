import type { DocumentStore, Position } from '../document/store';
import { DEFAULT_STYLE, type CellStyle } from '../document/styles';
import { isMac, ShortcutRegistry, type Command, type CommandId } from './registry';

export type GridNavigation = {
  move?: (row: number, column: number, extend?: boolean) => void;
  paste?: () => Promise<void>;
};
export function createSpreadsheetRegistry(document: DocumentStore, navigation: GridNavigation) {
  const mod = isMac() ? 'Cmd' : 'Ctrl';
  let registry: ShortcutRegistry<CommandId>;
  const commands: Command<CommandId>[] = [];
  const add = (
    id: CommandId,
    label: string,
    group: string,
    bindings: string[],
    execute: () => void,
    options: Partial<Pick<Command, 'contexts' | 'enabled' | 'native'>> = {},
  ) => commands.push({ id, label, group, bindings, execute, contexts: ['grid'], ...options });
  const active = () => Boolean(document.sheet);
  const editable = () => active() && document.canEdit;
  const format = () => document.canFormat;
  const toggle = (property: 'font_bold' | 'font_italic' | 'font_strike') =>
    document.setStyle([{ property, value: document.selectionStyle(property) !== true }]);
  add('help', 'Все горячие клавиши', 'Справка', [`${mod}+/`], () => registry.open('help'), {
    contexts: ['grid', 'chrome', 'editor'],
  });
  add('undo', 'Отменить', 'Правка', [`${mod}+Z`], () => document.undo(), {
    enabled: () => document.canUndo,
  });
  add('redo', 'Повторить', 'Правка', [`${mod}+Y`, `${mod}+Shift+Z`], () => document.redo(), {
    enabled: () => document.canRedo,
  });
  for (const [id, label, key] of [
    ['copy', 'Копировать', 'C'],
    ['cut', 'Вырезать', 'X'],
    ['paste', 'Вставить', 'V'],
  ] as const)
    add(
      id,
      label,
      'Правка',
      [`${mod}+${key}`],
      () => {
        if (id === 'paste') void navigation.paste?.();
        else globalThis.document.execCommand(id);
      },
      {
        native: true,
        enabled:
          id === 'copy'
            ? () => active() && document.canCopy
            : id === 'cut'
              ? () => editable() && document.canCopy
              : editable,
      },
    );
  add('find', 'Найти на листе', 'Правка', [`${mod}+F`], () => registry.open('find'), {
    enabled: active,
  });
  add('bold', 'Полужирный', 'Оформление', [`${mod}+B`], () => toggle('font_bold'), {
    enabled: format,
  });
  add('italic', 'Курсив', 'Оформление', [`${mod}+I`], () => toggle('font_italic'), {
    enabled: format,
  });
  add(
    'underline',
    'Подчёркивание',
    'Оформление',
    [`${mod}+U`],
    () =>
      document.setStyle([
        {
          property: 'font_underline',
          value: document.selectionStyle('font_underline') === 'single' ? 'none' : 'single',
        },
      ]),
    { enabled: format },
  );
  add(
    'strike',
    'Зачёркнутый',
    'Оформление',
    [isMac() ? 'Cmd+Shift+X' : 'Alt+Shift+5'],
    () => toggle('font_strike'),
    { enabled: format },
  );
  for (const [align, key, label] of [
    ['left', 'L', 'Слева'],
    ['center', 'E', 'По центру'],
    ['right', 'R', 'Справа'],
  ] as const)
    add(
      `align-${align}`,
      `Выравнивание: ${label}`,
      'Оформление',
      [`${mod}+Shift+${key}`],
      () => document.setStyle([{ property: 'horizontal', value: align }]),
      { enabled: format },
    );
  add(
    'clear-format',
    'Очистить оформление',
    'Оформление',
    [`${mod}+\\`],
    () =>
      document.setStyle(
        (Object.keys(DEFAULT_STYLE) as (keyof CellStyle)[]).map((property) => ({
          property,
          reset: true,
        })),
      ),
    { enabled: format },
  );
  add(
    'select-row',
    'Выделить строку',
    'Выделение',
    ['Shift+Space'],
    () => {
      if (!document.sheet) return;
      const row = document.selected.row;
      document.select({ row, column: 0 });
      document.select({ row, column: document.sheet.columns.length - 1 }, true);
    },
    { enabled: active },
  );
  add(
    'select-column',
    'Выделить столбец',
    'Выделение',
    ['Ctrl+Space'],
    () => {
      if (!document.sheet) return;
      const column = document.selected.column;
      document.select({ row: 0, column });
      document.select({ row: document.sheet.rows.length - 1, column }, true);
    },
    { enabled: active },
  );
  add(
    'select-all',
    'Выделить лист',
    'Выделение',
    [`${mod}+A`],
    () => {
      if (!document.sheet) return;
      document.select({ row: 0, column: 0 });
      document.select(
        { row: document.sheet.rows.length - 1, column: document.sheet.columns.length - 1 },
        true,
      );
    },
    { enabled: active },
  );
  add(
    'insert',
    'Вставить строку или столбец',
    'Структура',
    [`${mod}+Alt+=`],
    () => registry.open('insert'),
    { enabled: editable },
  );
  add(
    'delete-axis',
    'Удалить строку или столбец',
    'Структура',
    [`${mod}+Alt+-`],
    () => registry.open('delete'),
    { enabled: editable },
  );
  for (const [key, dr, dc] of [
    ['ArrowDown', 1, 0],
    ['ArrowUp', -1, 0],
    ['ArrowLeft', 0, -1],
    ['ArrowRight', 0, 1],
    ['Enter', 1, 0],
    ['Tab', 0, 1],
  ] as const) {
    for (const shift of [false, true]) {
      if (shift && key === 'Tab') continue;
      if (shift && key === 'Enter') {
        add(
          'move-Enter-true',
          'Shift+Enter',
          'Навигация',
          ['Shift+Enter'],
          () => navigation.move?.(document.selected.row - 1, document.selected.column),
          { enabled: active },
        );
        continue;
      }
      const reverse = shift && !key.startsWith('Arrow') ? -1 : 1;
      add(
        `move-${key}-${shift}`,
        `${shift ? 'Shift+' : ''}${key}`,
        'Навигация',
        [`${shift ? 'Shift+' : ''}${key}`],
        () =>
          navigation.move?.(
            document.selected.row + dr * reverse,
            document.selected.column + dc * reverse,
            shift && key.startsWith('Arrow'),
          ),
        { enabled: active },
      );
    }
  }
  for (const [key, destination] of [
    ['Home', () => ({ row: document.selected.row, column: 0 })],
    [
      'End',
      () => ({ row: document.selected.row, column: (document.sheet?.columns.length ?? 1) - 1 }),
    ],
    [`${mod}+Home`, () => ({ row: 0, column: 0 })],
    [
      `${mod}+End`,
      () => ({
        row: (document.sheet?.rows.length ?? 1) - 1,
        column: (document.sheet?.columns.length ?? 1) - 1,
      }),
    ],
  ] as const)
    add(
      `move-${key}`,
      key,
      'Навигация',
      [key],
      () => {
        const p: Position = destination();
        navigation.move?.(p.row, p.column);
      },
      { enabled: active },
    );
  add('edit', 'Редактировать ячейку', 'Правка', ['F2'], () => document.begin(), {
    enabled: editable,
  });
  for (const key of ['Delete', 'Backspace'] as const)
    add(`clear-${key}`, 'Очистить содержимое', 'Правка', [key], () => document.clearSelection(), {
      enabled: editable,
    });
  registry = new ShortcutRegistry(commands);
  return registry;
}
