import { boundedJson, isUuid } from '../sharing/api';
import type { FilterPredicate, FilterValue, PersonalFilters } from './filters';

export type PersonalView = { schema_version: 1; revision: number; settings: PersonalFilters };
const invalid = () => new Error('Некорректный ответ личного представления.');
const object = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid();
  return value as Record<string, unknown>;
};
const text = (value: unknown): value is string =>
  typeof value === 'string' &&
  !value.includes('\0') &&
  new TextEncoder().encode(value).length <= 1024;
function decodeValue(raw: unknown): FilterValue {
  const value = object(raw);
  if (value.type === 'blank') return { type: 'blank' };
  if (value.type === 'number' && typeof value.value === 'number' && Number.isFinite(value.value))
    return { type: 'number', value: value.value };
  if (value.type === 'text' && text(value.value)) return { type: 'text', value: value.value };
  if (value.type === 'error' && text(value.value)) return { type: 'error', value: value.value };
  if (value.type === 'boolean' && typeof value.value === 'boolean')
    return { type: 'boolean', value: value.value };
  throw invalid();
}
function predicate(raw: unknown): FilterPredicate {
  const value = object(raw);
  if (value.kind === 'blank' || value.kind === 'nonblank') return { kind: value.kind };
  if (value.kind === 'values' && Array.isArray(value.values) && value.values.length <= 500)
    return { kind: 'values', values: value.values.map(decodeValue) };
  if (
    value.kind === 'text' &&
    ['equals', 'contains', 'starts_with'].includes(String(value.operator)) &&
    text(value.value)
  )
    return {
      kind: 'text',
      operator: value.operator as 'equals' | 'contains' | 'starts_with',
      value: value.value,
    };
  if (
    value.kind === 'number' &&
    ['eq', 'ne', 'gt', 'gte', 'lt', 'lte'].includes(String(value.operator)) &&
    typeof value.value === 'number' &&
    Number.isFinite(value.value)
  )
    return {
      kind: 'number',
      operator: value.operator as 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte',
      value: value.value,
    };
  throw invalid();
}
export function decodePersonalView(raw: unknown): PersonalView {
  const view = object(raw);
  const settings = object(view.settings);
  if (
    view.schema_version !== 1 ||
    !Number.isSafeInteger(view.revision) ||
    (view.revision as number) < 0 ||
    !Array.isArray(settings.filters) ||
    settings.filters.length > 5
  )
    throw invalid();
  const identifiers = new Set<string>();
  const dismissals = settings.hardcode_dismissals;
  if (
    dismissals !== undefined &&
    (!Array.isArray(dismissals) ||
      dismissals.length > 1000 ||
      new Set(dismissals).size !== dismissals.length ||
      dismissals.some((value) => typeof value !== 'string' || !/^[0-9a-f]{64}$/.test(value)))
  )
    throw invalid();
  const filters = settings.filters.map((raw) => {
    const filter = object(raw);
    const keys = [
      'id',
      'sheet_id',
      'first_row_id',
      'last_row_id',
      'first_column_id',
      'last_column_id',
    ] as const;
    if (
      keys.some((key) => !isUuid(filter[key])) ||
      identifiers.has(filter.id as string) ||
      (filter.header_row_id !== null && !isUuid(filter.header_row_id)) ||
      !Array.isArray(filter.conditions) ||
      filter.conditions.length > 50
    )
      throw invalid();
    identifiers.add(filter.id as string);
    const columns = new Set<string>();
    const conditions = filter.conditions.map((raw) => {
      const condition = object(raw);
      if (!isUuid(condition.column_id) || columns.has(condition.column_id)) throw invalid();
      columns.add(condition.column_id);
      return { column_id: condition.column_id, predicate: predicate(condition.predicate) };
    });
    return {
      id: filter.id as string,
      sheet_id: filter.sheet_id as string,
      first_row_id: filter.first_row_id as string,
      last_row_id: filter.last_row_id as string,
      first_column_id: filter.first_column_id as string,
      last_column_id: filter.last_column_id as string,
      header_row_id: filter.header_row_id as string | null,
      conditions,
    };
  });
  return {
    schema_version: 1,
    revision: view.revision as number,
    settings: {
      filters,
      ...(dismissals === undefined ? {} : { hardcode_dismissals: dismissals as string[] }),
    },
  };
}
export async function readPersonalView(path: string) {
  const response = await fetch(path, { signal: AbortSignal.timeout(10_000) });
  if (!response.ok) throw new Error('Не удалось загрузить личные фильтры.');
  return decodePersonalView(await boundedJson(response, 128 * 1024 + 1024));
}
export async function savePersonalView(
  path: string,
  revision: number,
  version: number,
  settings: PersonalFilters,
) {
  const response = await fetch(path, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      schema_version: 1,
      expected_revision: revision,
      document_version: version,
      settings,
    }),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok)
    throw new Error(
      response.status === 409
        ? 'Представление или книга изменились. Откройте фильтры заново.'
        : 'Не удалось сохранить личные фильтры.',
    );
  return decodePersonalView(await boundedJson(response, 128 * 1024 + 1024));
}
