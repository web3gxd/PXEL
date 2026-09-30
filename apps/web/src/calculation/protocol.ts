import type { CalculatedValue, Sheet, Value } from '../document/store';

export type Stamp = { document_id: string; input_version: string; pending_revision: string };
export type PreviewValue =
  | Exclude<Value, { type: 'formula' }>
  | {
      type: 'formula';
      value: {
        source: string;
        canonical?: string;
        ast_schema?: number;
        compatibility_profile?: string;
        bound_at_version?: string;
      };
    };
/** Only calculation fields cross this boundary; source is rendered for current axes. */
export function toPreviewValue(value: Value): PreviewValue {
  if (value.type !== 'formula') return value;
  const formula = value.value;
  const descriptor: Extract<PreviewValue, { type: 'formula' }>['value'] = {
    source: formula.current_text ?? formula.source,
  };
  if (
    formula.canonical !== undefined &&
    (formula.current_text === undefined || formula.current_text === formula.canonical)
  )
    descriptor.canonical = formula.canonical;
  if (formula.ast_schema !== undefined) descriptor.ast_schema = formula.ast_schema;
  if (formula.compatibility_profile !== undefined)
    descriptor.compatibility_profile = formula.compatibility_profile;
  if (formula.bound_at_version !== undefined)
    descriptor.bound_at_version = formula.bound_at_version;
  return { type: 'formula', value: descriptor };
}
export type PreviewCell = {
  sheet_id: string;
  row_id: string;
  column_id: string;
  value: PreviewValue;
};
export type PreviewContext = {
  schema_version: 1;
  context_id: string;
  epoch: string;
  effective_timestamp_utc_ms: string;
  utc_offset_minutes: number;
  civil_day_since_unix_epoch: number;
  random_seed: string;
  engine_version: string;
  compatibility_profile: string;
  reason: 'open' | 'explicit' | 'timer' | 'volatile_dependency' | 'engine_upgrade';
};
export type PreviewInit = Stamp & { sheets: Sheet[] } & (
    | { schema_version: 1 | 2; calculation_context?: never }
    | { schema_version: 3; calculation_context: PreviewContext }
  );
export type PreviewPatch = Stamp & {
  schema_version: 1;
  expected_input_version: string;
  expected_pending_revision: string;
  cells: PreviewCell[];
};
export type PreviewMoveGeometry = {
  source: Omit<PreviewCell, 'value'>;
  destination: Omit<PreviewCell, 'value'>;
  rows: number;
  columns: number;
};
export type PreviewMove = Stamp &
  PreviewMoveGeometry & {
    schema_version: 1;
    expected_pending_revision: string;
  };
export type PreviewResult = Stamp & {
  schema_version: 1;
  preview: true;
  complete: true;
  full: boolean;
  formula_engine_version: string;
  compatibility_profile: string;
  normalized_result_hash: string;
  evaluated_cells: number;
  dirty_cells: number;
  results: (Omit<PreviewCell, 'value'> & { value: CalculatedValue })[];
  cleared: Omit<PreviewCell, 'value'>[];
  move_writes?: PreviewCell[];
};
export type Request =
  | { id: number; kind: 'init'; data: PreviewInit }
  | { id: number; kind: 'apply'; data: PreviewPatch }
  | { id: number; kind: 'move'; data: PreviewMove };
export type Reply =
  | { id: number; result: PreviewResult }
  | { id: number; error: string; phase: 'init' | 'apply' | 'move' | 'calculate' };
export const addressKey = (cell: Omit<PreviewCell, 'value'>) =>
  `${cell.sheet_id}/${cell.row_id}/${cell.column_id}`;
