import { expect, it } from 'vitest';
import { suggestSortBounds, validSortBounds } from './sort-range';
import type { Sheet } from './store';

it('suggests the connected table without changing the original single-column selection', () => {
  const sheet: Sheet = {
    id: 'sheet',
    name: 'Sheet',
    rows: ['r0', 'r1', 'r2', 'r3'],
    columns: ['c0', 'c1', 'c2', 'c3'],
    cells: [
      { row_id: 'r0', column_id: 'c0', value: { type: 'text', value: 'Header' } },
      { row_id: 'r0', column_id: 'c1', value: { type: 'text', value: 'Value' } },
      { row_id: 'r1', column_id: 'c0', value: { type: 'number', value: 1 } },
      { row_id: 'r1', column_id: 'c1', value: { type: 'number', value: 2 } },
      { row_id: 'r3', column_id: 'c3', value: { type: 'text', value: 'Separate' } },
    ],
  };
  const selected = { startRow: 1, endRow: 1, startColumn: 0, endColumn: 0 };
  expect(suggestSortBounds(sheet, selected)).toEqual({
    startRow: 0,
    endRow: 1,
    startColumn: 0,
    endColumn: 1,
  });
  expect(selected).toEqual({ startRow: 1, endRow: 1, startColumn: 0, endColumn: 0 });
  expect(
    suggestSortBounds(sheet, { startRow: 3, endRow: 3, startColumn: 3, endColumn: 3 }),
  ).toBeUndefined();
});

it('caps the complete rectangle and rejects invalid or reversed bounds', () => {
  const sheet: Sheet = {
    id: 'sheet',
    name: 'Sheet',
    rows: Array.from({ length: 100_000 }, (_, i) => String(i)),
    columns: ['a', 'b'],
    cells: [],
  };
  expect(
    validSortBounds(sheet, { startRow: 0, endRow: 49_999, startColumn: 0, endColumn: 1 }),
  ).toBe(true);
  expect(
    validSortBounds(sheet, { startRow: 0, endRow: 50_000, startColumn: 0, endColumn: 1 }),
  ).toBe(false);
  expect(validSortBounds(sheet, { startRow: 2, endRow: 1, startColumn: 0, endColumn: 0 })).toBe(
    false,
  );
});
