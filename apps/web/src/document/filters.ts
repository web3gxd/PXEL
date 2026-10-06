import type { CalculatedValue, Sheet } from './store';

export type FilterValue =
  | { type: 'blank' }
  | { type: 'number'; value: number }
  | { type: 'text'; value: string }
  | { type: 'boolean'; value: boolean }
  | { type: 'error'; value: string };
export type FilterPredicate =
  | { kind: 'blank' }
  | { kind: 'nonblank' }
  | { kind: 'values'; values: FilterValue[] }
  | { kind: 'text'; operator: 'equals' | 'contains' | 'starts_with'; value: string }
  | { kind: 'number'; operator: 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte'; value: number };
export type PersonalFilter = {
  id: string;
  sheet_id: string;
  first_row_id: string;
  last_row_id: string;
  first_column_id: string;
  last_column_id: string;
  header_row_id: string | null;
  conditions: { column_id: string; predicate: FilterPredicate }[];
};
export type PersonalFilters = { filters: PersonalFilter[]; hardcode_dismissals?: string[] };

function blank(value: CalculatedValue | undefined) {
  return !value || value.type === 'blank' || (value.type === 'text' && value.value === '');
}
export function matchesFilter(
  value: CalculatedValue | undefined,
  predicate: FilterPredicate,
): boolean {
  if (predicate.kind === 'blank') return blank(value);
  if (predicate.kind === 'nonblank') return !blank(value);
  if (predicate.kind === 'values')
    return predicate.values.some((key) =>
      key.type === 'blank' ? blank(value) : value?.type === key.type && value.value === key.value,
    );
  if (predicate.kind === 'text') {
    if (value?.type !== 'text') return false;
    if (predicate.operator === 'equals') return value.value === predicate.value;
    if (predicate.operator === 'starts_with') return value.value.startsWith(predicate.value);
    return value.value.includes(predicate.value);
  }
  if (value?.type !== 'number' || !Number.isFinite(value.value)) return false;
  switch (predicate.operator) {
    case 'eq':
      return value.value === predicate.value;
    case 'ne':
      return value.value !== predicate.value;
    case 'gt':
      return value.value > predicate.value;
    case 'gte':
      return value.value >= predicate.value;
    case 'lt':
      return value.value < predicate.value;
    case 'lte':
      return value.value <= predicate.value;
  }
}

/** Confirmed scalar/results only. Deleted boundaries suspend a view rather than reinterpreting A1. */
export function filteredRows(
  sheet: Sheet,
  views: PersonalFilters,
  value: (row: number, column: number) => CalculatedValue | undefined,
) {
  const hidden = new Set<number>();
  for (const filter of views.filters) {
    if (filter.sheet_id !== sheet.id) continue;
    const first = sheet.rows.indexOf(filter.first_row_id),
      last = sheet.rows.indexOf(filter.last_row_id);
    const left = sheet.columns.indexOf(filter.first_column_id),
      right = sheet.columns.indexOf(filter.last_column_id);
    const header = filter.header_row_id ? sheet.rows.indexOf(filter.header_row_id) : first - 1;
    const conditions = filter.conditions.map((condition) => ({
      ...condition,
      column: sheet.columns.indexOf(condition.column_id),
    }));
    if (
      first < 0 ||
      last < first ||
      left < 0 ||
      right < left ||
      (last - first + 1) * (right - left + 1) > 100_000 ||
      (filter.header_row_id && (header < first || header > last)) ||
      conditions.some(({ column }) => column < left || column > right)
    )
      continue;
    for (let row = Math.max(first, header + 1); row <= last; row++)
      if (
        !conditions.every(({ column, predicate }) => matchesFilter(value(row, column), predicate))
      )
        hidden.add(row);
  }
  let adjusted_merges = 0;
  for (const merge of sheet.merges ?? []) {
    const first = sheet.rows.indexOf(merge.first_row_id),
      last = sheet.rows.indexOf(merge.last_row_id);
    if (first < 0 || last <= first) continue;
    let concealed = 0;
    for (let row = first; row <= last; row++) if (hidden.has(row)) concealed++;
    if (concealed > 0 && concealed < last - first + 1) {
      for (let row = first; row <= last; row++) hidden.delete(row);
      adjusted_merges++;
    }
  }
  return { hidden, adjusted_merges };
}
