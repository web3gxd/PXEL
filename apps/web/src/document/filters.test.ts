import { describe, expect, it } from 'vitest';
import { filteredRows, matchesFilter, type PersonalFilters } from './filters';
import type { CalculatedValue, Sheet } from './store';

const sheet: Sheet = {
  id: 's',
  name: 'Лист',
  rows: ['r0', 'r1', 'r2', 'r3', 'r4'],
  columns: ['a', 'b'],
  cells: [],
};
const filters: PersonalFilters = {
  filters: [
    {
      id: 'view',
      sheet_id: 's',
      first_row_id: 'r0',
      last_row_id: 'r4',
      first_column_id: 'a',
      last_column_id: 'b',
      header_row_id: 'r0',
      conditions: [
        { column_id: 'a', predicate: { kind: 'number', operator: 'gte', value: 10 } },
        { column_id: 'b', predicate: { kind: 'text', operator: 'contains', value: 'Москва' } },
      ],
    },
  ],
};

describe('personal filters on confirmed values', () => {
  it('combines columns with AND and keeps the header and rows outside the range', () => {
    const values: CalculatedValue[][] = [
      [
        { type: 'text', value: 'Число' },
        { type: 'text', value: 'Город' },
      ],
      [
        { type: 'number', value: 12 },
        { type: 'text', value: 'Москва' },
      ],
      [
        { type: 'number', value: 9 },
        { type: 'text', value: 'Москва' },
      ],
      [
        { type: 'number', value: 30 },
        { type: 'text', value: 'Пермь' },
      ],
      [
        { type: 'error', value: '#VALUE!' },
        { type: 'text', value: 'Москва' },
      ],
    ];
    const result = filteredRows(sheet, filters, (row, column) => values[row]?.[column]);
    expect([...result.hidden]).toEqual([2, 3, 4]);
    expect(result.adjusted_merges).toBe(0);
    expect(filteredRows(sheet, { filters: [] }, () => undefined).hidden.size).toBe(0);
  });

  it('distinguishes numbers, text, blank, nonblank, and selected values', () => {
    expect(
      matchesFilter({ type: 'number', value: 10 }, { kind: 'number', operator: 'eq', value: 10 }),
    ).toBe(true);
    expect(
      matchesFilter({ type: 'text', value: '10' }, { kind: 'number', operator: 'eq', value: 10 }),
    ).toBe(false);
    expect(matchesFilter(undefined, { kind: 'blank' })).toBe(true);
    expect(matchesFilter({ type: 'text', value: '' }, { kind: 'blank' })).toBe(true);
    expect(matchesFilter({ type: 'error', value: '#REF!' }, { kind: 'nonblank' })).toBe(true);
    expect(
      matchesFilter(
        { type: 'boolean', value: true },
        { kind: 'values', values: [{ type: 'boolean', value: true }] },
      ),
    ).toBe(true);
    expect(
      matchesFilter(
        { type: 'number', value: 1 },
        { kind: 'values', values: [{ type: 'text', value: '1' }] },
      ),
    ).toBe(false);
  });

  it('never partially hides a vertical merge and ignores retired filter boundaries', () => {
    const merged: Sheet = {
      ...sheet,
      merges: [
        { first_row_id: 'r1', last_row_id: 'r2', first_column_id: 'a', last_column_id: 'a' },
      ],
    };
    const result = filteredRows(merged, filters, (row, column) =>
      column === 0
        ? { type: 'number', value: row === 1 ? 12 : 9 }
        : { type: 'text', value: 'Москва' },
    );
    expect([...result.hidden]).toEqual([3, 4]);
    expect(result.adjusted_merges).toBe(1);
    const retired = { filters: [{ ...filters.filters[0]!, last_row_id: 'deleted' }] };
    expect(filteredRows(sheet, retired, () => undefined).hidden.size).toBe(0);
  });
});
