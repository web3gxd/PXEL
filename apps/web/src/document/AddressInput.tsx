import { useState } from 'react';
import type { DocumentStore } from './store';
import type { GridNavigation } from '../commands/spreadsheet';
import { columnLabel } from '../grid/viewport';

export function AddressInput({
  document,
  navigation,
}: {
  document: DocumentStore;
  navigation: GridNavigation;
}) {
  const [draft, setDraft] = useState<string>();
  const [invalid, setInvalid] = useState(false);
  const bounds = document.selectionBounds;
  const first = `${columnLabel(bounds.startColumn)}${bounds.startRow + 1}`;
  const last = `${columnLabel(bounds.endColumn)}${bounds.endRow + 1}`;
  const address =
    document.sheet && !document.allRowsHidden ? (first === last ? first : `${first}:${last}`) : '—';
  return (
    <input
      className="cell-address"
      aria-label="Ячейка или диапазон"
      aria-invalid={invalid}
      title={
        invalid
          ? 'Введите адрес внутри листа, например B9 или A3:E5'
          : 'Введите ячейку или диапазон и нажмите Enter'
      }
      value={draft ?? address}
      disabled={!document.sheet || document.allRowsHidden}
      onFocus={(event) => {
        setDraft(address);
        event.currentTarget.select();
      }}
      onChange={(event) => {
        setDraft(event.target.value);
        setInvalid(false);
      }}
      onBlur={() => {
        setDraft(undefined);
        setInvalid(false);
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          setDraft(undefined);
          setInvalid(false);
          event.currentTarget.blur();
          return;
        }
        if (event.key !== 'Enter') return;
        event.preventDefault();
        const sheet = document.sheet;
        const match = (draft ?? address)
          .trim()
          .match(/^\$?([a-z]+)\$?([1-9]\d*)(?:\s*:\s*\$?([a-z]+)\$?([1-9]\d*))?$/i);
        if (!sheet || !match) {
          setInvalid(true);
          return;
        }
        const position = (letters: string, number: string) => ({
          column:
            [...letters.toUpperCase()].reduce(
              (value, letter) => value * 26 + letter.charCodeAt(0) - 64,
              0,
            ) - 1,
          row: Number(number) - 1,
        });
        const start = position(match[1]!, match[2]!);
        const end = position(match[3] ?? match[1]!, match[4] ?? match[2]!);
        if (
          [start, end].some(
            (p) =>
              !Number.isSafeInteger(p.row) ||
              !Number.isSafeInteger(p.column) ||
              p.row >= sheet.rows.length ||
              p.column >= sheet.columns.length,
          )
        ) {
          setInvalid(true);
          return;
        }
        if (document.editing) document.commit();
        if (document.editing) return;
        navigation.move?.(start.row, start.column);
        navigation.move?.(end.row, end.column, true);
        event.currentTarget.blur();
        event.currentTarget.ownerDocument.querySelector<HTMLElement>('.grid-scroll')?.focus();
      }}
    />
  );
}
