import {
  Plus,
  Trash,
  GridFour,
  Rows,
  Columns,
  SlidersHorizontal,
  ArrowsVertical,
  ArrowsHorizontal,
  Eye,
  EyeSlash,
  PencilSimple,
  Copy,
  Palette,
  Table,
} from '@phosphor-icons/react';
import type { DocumentStore } from '../document/store';
import type { EditorAction, EditorMenuGroup } from './EditorMenus';
import type { CellsPanelKind } from './CellsPanel';

export function cellsMenu(
  document: DocumentStore,
  online: boolean,
  sizes: (axis: 'row' | 'column') => void,
  panel: (kind: CellsPanelKind) => void,
): EditorMenuGroup {
  const disabled = !document.sheet || !online || !document.canEdit;
  const busy = !online || !document.canChangeCells;
  const showLabel = (axis: 'row' | 'column') => {
    const label = axis === 'row' ? 'Показать строки' : 'Показать столбцы';
    const sheet = document.sheet;
    if (!sheet) return label;
    const hidden = (axis === 'row' ? sheet.hidden_rows : sheet.hidden_columns) ?? [];
    if (hidden.length <= 1000) return label;
    const live = new Set(axis === 'row' ? sheet.rows : sheet.columns);
    let count = 0;
    for (const id of hidden)
      if (live.has(id) && ++count > 1000)
        return axis === 'row' ? 'Показать 1000 скрытых строк' : 'Показать 1000 скрытых столбцов';
    return label;
  };
  const dialog = (
    label: string,
    icon: EditorAction['icon'],
    kind: CellsPanelKind,
  ): EditorAction => ({ label, icon, disabled: busy, run: () => panel(kind) });
  return {
    label: 'Ячейки',
    actions: [
      {
        label: 'Добавить',
        icon: Plus,
        children: [
          dialog('Вставить ячейки…', GridFour, 'insert'),
          {
            label: 'Вставить строки листа',
            icon: Rows,
            disabled,
            run: () => document.insertRowBeforeSelection(),
          },
          {
            label: 'Вставить столбцы листа',
            icon: Columns,
            disabled,
            run: () => document.insertColumnBeforeSelection(),
          },
          { label: 'Добавить лист', icon: Table, disabled, run: () => document.addSheet() },
        ],
      },
      {
        label: 'Удалить',
        icon: Trash,
        children: [
          dialog('Удалить ячейки…', GridFour, 'delete'),
          {
            label: 'Удалить строки листа',
            icon: Rows,
            disabled: disabled || (document.sheet?.rows.length ?? 0) <= 1,
            run: () => document.deleteSelectedRow(),
          },
          {
            label: 'Удалить столбцы листа',
            icon: Columns,
            disabled: disabled || (document.sheet?.columns.length ?? 0) <= 1,
            run: () => document.deleteSelectedColumn(),
          },
        ],
      },
      {
        label: 'Форматировать',
        icon: SlidersHorizontal,
        children: [
          { label: 'Высота строки…', icon: ArrowsVertical, disabled, run: () => sizes('row') },
          {
            label: 'Автоподбор высоты строки',
            icon: Rows,
            disabled: busy || disabled,
            run: () => document.autoFit('row'),
          },
          {
            label: 'Ширина столбца…',
            icon: ArrowsHorizontal,
            disabled,
            run: () => sizes('column'),
          },
          {
            label: 'Автоподбор ширины столбца',
            icon: Columns,
            disabled: busy || disabled,
            run: () => document.autoFit('column'),
          },
          dialog('Ширина по умолчанию…', Columns, 'defaultWidth'),
          {
            label: 'Видимость',
            icon: Eye,
            children: [
              {
                label: 'Скрыть строки',
                icon: EyeSlash,
                disabled: busy || disabled,
                run: () => document.setAxisVisibility('row', true),
              },
              {
                label: showLabel('row'),
                icon: Eye,
                disabled: busy || !document.sheet?.hidden_rows?.length,
                run: () => document.setAxisVisibility('row', false),
              },
              {
                label: 'Скрыть столбцы',
                icon: EyeSlash,
                disabled: busy || disabled,
                run: () => document.setAxisVisibility('column', true),
              },
              {
                label: showLabel('column'),
                icon: Eye,
                disabled: busy || !document.sheet?.hidden_columns?.length,
                run: () => document.setAxisVisibility('column', false),
              },
              {
                label: 'Скрыть лист',
                icon: EyeSlash,
                disabled:
                  busy || (document.projection?.sheets.filter((s) => !s.hidden).length ?? 0) <= 1,
                run: () => document.setSheetHidden(true),
              },
              {
                ...dialog('Показать лист…', Eye, 'showSheet'),
                disabled: busy || !document.projection?.sheets.some((s) => s.hidden),
              },
            ],
          },
          {
            label: 'Управление листами',
            icon: Table,
            children: [
              dialog('Переименовать лист…', PencilSimple, 'rename'),
              dialog('Переместить или скопировать лист…', Copy, 'moveCopy'),
              dialog('Цвет вкладки…', Palette, 'tabColor'),
            ],
          },
        ],
      },
    ],
  };
}
