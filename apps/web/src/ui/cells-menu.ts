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

const pending = (label: string, icon: EditorAction['icon']): EditorAction => ({
  label,
  icon,
  disabled: true,
  unavailable: 'Эта операция пока не поддерживается. Изменения не будут применены.',
});

export function cellsMenu(
  document: DocumentStore,
  online: boolean,
  sizes: (axis: 'row' | 'column') => void,
): EditorMenuGroup {
  const disabled = !document.sheet || !online || !document.canEdit;
  return {
    label: 'Ячейки',
    actions: [
      {
        label: 'Добавить',
        icon: Plus,
        children: [
          pending('Вставить ячейки…', GridFour),
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
          pending('Удалить ячейки…', GridFour),
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
          pending('Автоподбор высоты строки', Rows),
          {
            label: 'Ширина столбца…',
            icon: ArrowsHorizontal,
            disabled,
            run: () => sizes('column'),
          },
          pending('Автоподбор ширины столбца', Columns),
          pending('Ширина по умолчанию…', Columns),
          {
            label: 'Видимость',
            icon: Eye,
            children: [
              pending('Скрыть строки', EyeSlash),
              pending('Показать строки', Eye),
              pending('Скрыть столбцы', EyeSlash),
              pending('Показать столбцы', Eye),
              pending('Скрыть лист', EyeSlash),
              pending('Показать лист…', Eye),
            ],
          },
          {
            label: 'Управление листами',
            icon: Table,
            children: [
              pending('Переименовать лист…', PencilSimple),
              pending('Переместить или скопировать лист…', Copy),
              pending('Цвет вкладки…', Palette),
            ],
          },
        ],
      },
    ],
  };
}
