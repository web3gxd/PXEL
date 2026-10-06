import { isUuid } from '../sharing/api';
import { decodeProjection, type Projection } from './store';
export const diffCategories = [
  'value',
  'formula',
  'calculated',
  'style',
  'size',
  'merge',
  'sheet',
  'structure',
  'layout',
] as const;
export type DiffCategory = (typeof diffCategories)[number];
export type Difference = {
  category: DiffCategory;
  sheet_id: string;
  entity_id: string;
  property: string;
  before: unknown;
  after: unknown;
  structural_context: boolean;
};
export type DiffReport = {
  schema_version: 1;
  document_id: string;
  from_version: string;
  to_version: string;
  structural_context: boolean;
  changes: Difference[];
};
export type Footprint = {
  sheet_id: string;
  addresses: [string, string][];
  row_ids: string[];
  column_ids: string[];
};
export type Activity = {
  operation_id: string;
  actor_id: string | null;
  created_at: number;
  version: string;
  operation_type: string;
  structural: boolean;
  footprints: Footprint[];
};
export type ActivityPage = {
  schema_version: 1;
  through_version: string;
  events: Activity[];
  next_before: string | null;
};
export type Historical = {
  schema_version: 1;
  read_only: true;
  projection: Projection;
  calculated: { sheet_id: string; row_id: string; column_id: string; value: unknown }[];
};
const invalid = () => new Error('Некорректный ответ истории версий.');
const version = (v: unknown): v is string =>
  typeof v === 'string' && /^[1-9]\d{0,18}$/.test(v) && BigInt(v) <= 9223372036854775807n;
function record(v: unknown, keys: string[]): Record<string, unknown> {
  if (
    !v ||
    typeof v !== 'object' ||
    Array.isArray(v) ||
    Object.keys(v).length !== keys.length ||
    keys.some((k) => !Object.hasOwn(v, k))
  )
    throw invalid();
  return v as Record<string, unknown>;
}
const types = new Set([
  'create_workbook',
  'add_sheet',
  'frozen_rows',
  'axis_size',
  'cell_write',
  'undo_redo',
  'range_write',
  'copy_range',
  'move_range',
  'style_write',
  'sort_range',
  'cell_batch',
  'merge_write',
  'rows',
  'columns',
  'axis_edit',
  'calculation_epoch',
]);
export function decodeActivity(raw: unknown): ActivityPage {
  const p = record(raw, ['schema_version', 'through_version', 'events', 'next_before']);
  if (
    p.schema_version !== 1 ||
    !version(p.through_version) ||
    !Array.isArray(p.events) ||
    p.events.length > 100 ||
    (p.next_before !== null && !version(p.next_before))
  )
    throw invalid();
  let previous = BigInt(p.through_version) + 1n;
  const ids = new Set<string>();
  for (const rawEvent of p.events) {
    const e = record(rawEvent, [
      'operation_id',
      'actor_id',
      'created_at',
      'version',
      'operation_type',
      'structural',
      'footprints',
    ]);
    if (
      !isUuid(e.operation_id) ||
      ids.has(e.operation_id) ||
      (e.actor_id !== null && !isUuid(e.actor_id)) ||
      !Number.isSafeInteger(e.created_at) ||
      !version(e.version) ||
      BigInt(e.version) >= previous ||
      typeof e.operation_type !== 'string' ||
      !types.has(e.operation_type) ||
      typeof e.structural !== 'boolean' ||
      !Array.isArray(e.footprints) ||
      e.footprints.length > 64
    )
      throw invalid();
    ids.add(e.operation_id);
    previous = BigInt(e.version);
    for (const rawFoot of e.footprints) {
      const f = record(rawFoot, ['sheet_id', 'addresses', 'row_ids', 'column_ids']);
      if (
        !isUuid(f.sheet_id) ||
        !Array.isArray(f.addresses) ||
        f.addresses.length > 100000 ||
        f.addresses.some((a) => !Array.isArray(a) || a.length !== 2 || !a.every(isUuid)) ||
        ![f.row_ids, f.column_ids].every(
          (a) => Array.isArray(a) && a.length <= 1000 && a.every(isUuid),
        )
      )
        throw invalid();
    }
  }
  if (
    p.next_before !== null &&
    (!p.events.length || p.next_before !== (p.events.at(-1) as Activity).version)
  )
    throw invalid();
  return p as ActivityPage;
}
export function decodeDiff(raw: unknown): DiffReport {
  const p = record(raw, [
    'schema_version',
    'document_id',
    'from_version',
    'to_version',
    'structural_context',
    'changes',
  ]);
  if (
    p.schema_version !== 1 ||
    !isUuid(p.document_id) ||
    !version(p.from_version) ||
    !version(p.to_version) ||
    typeof p.structural_context !== 'boolean' ||
    !Array.isArray(p.changes) ||
    p.changes.length > 100000
  )
    throw invalid();
  const seen = new Set<string>();
  for (const item of p.changes) {
    const d = record(item, [
      'category',
      'sheet_id',
      'entity_id',
      'property',
      'before',
      'after',
      'structural_context',
    ]);
    if (
      !diffCategories.includes(d.category as DiffCategory) ||
      !isUuid(d.sheet_id) ||
      typeof d.entity_id !== 'string' ||
      (d.entity_id !== '' && !d.entity_id.split('/').every(isUuid)) ||
      typeof d.property !== 'string' ||
      d.property.length > 64 ||
      typeof d.structural_context !== 'boolean' ||
      (d.structural_context && (!p.structural_context || d.category !== 'formula'))
    )
      throw invalid();
    const key = JSON.stringify([d.sheet_id, d.category, d.entity_id, d.property]);
    if (seen.has(key)) throw invalid();
    seen.add(key);
  }
  return p as DiffReport;
}
export function decodeHistorical(raw: unknown): Historical {
  const p = record(raw, ['schema_version', 'read_only', 'projection', 'calculated']);
  if (
    p.schema_version !== 1 ||
    p.read_only !== true ||
    !Array.isArray(p.calculated) ||
    p.calculated.length > 100000
  )
    throw invalid();
  // The existing version reader has no live capabilities. This container is
  // explicitly read-only and never enters DocumentStore.load or a Worker.
  const authored = { ...(p.projection as Record<string, unknown>) };
  if (authored.capabilities === null) delete authored.capabilities;
  const projection = decodeProjection(authored);
  if (projection.representation || projection.capabilities?.can_edit) throw invalid();
  const seen = new Set<string>();
  const geometry = new Map(
    projection.sheets.map((s) => [s.id, { rows: new Set(s.rows), columns: new Set(s.columns) }]),
  );
  for (const rawCell of p.calculated) {
    const c = record(rawCell, ['sheet_id', 'row_id', 'column_id', 'value']);
    const sheet = geometry.get(c.sheet_id as string);
    if (
      !sheet ||
      typeof c.row_id !== 'string' ||
      typeof c.column_id !== 'string' ||
      !sheet.rows.has(c.row_id) ||
      !sheet.columns.has(c.column_id)
    )
      throw invalid();
    const key = `${c.sheet_id}/${c.row_id}/${c.column_id}`;
    if (seen.has(key)) throw invalid();
    seen.add(key);
    const v = c.value as { type?: unknown; value?: unknown };
    if (
      !v ||
      !['blank', 'number', 'text', 'boolean', 'error', 'datetime', 'array'].includes(
        v.type as string,
      )
    )
      throw invalid();
    if (v.type === 'number' && (typeof v.value !== 'number' || !Number.isFinite(v.value)))
      throw invalid();
    if (['text', 'error'].includes(v.type as string) && typeof v.value !== 'string')
      throw invalid();
    if (v.type === 'boolean' && typeof v.value !== 'boolean') throw invalid();
  }
  return {
    schema_version: 1,
    read_only: true,
    projection,
    calculated: p.calculated as Historical['calculated'],
  };
}
export function describeHistorical(value: unknown): string {
  if (value === null || value === undefined) return '∅';
  if (typeof value === 'object') {
    const v = value as {
      type?: string;
      value?: unknown;
      current_text?: string;
      authored_source?: string;
    };
    if (v.current_text) return v.current_text;
    if (v.type === 'blank') return '';
    if (v.type && v.value !== undefined)
      return typeof v.value === 'object' ? JSON.stringify(v.value) : String(v.value);
  }
  return typeof value === 'object' ? JSON.stringify(value) : String(value);
}
