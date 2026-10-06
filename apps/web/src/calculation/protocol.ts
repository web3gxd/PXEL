import type { CalculatedValue, Sheet, Value } from '../document/store';
import type { CellStyle } from '../document/styles';

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
        bound_formula_wire_hex?: string;
      };
    };
/** Only calculation fields cross this boundary; source is rendered for current axes. */
export function toPreviewValue(value: Value, exactBound = false): PreviewValue {
  if (value.type !== 'formula') return value;
  const formula = value.value;
  const descriptor: Extract<PreviewValue, { type: 'formula' }>['value'] = {
    source: exactBound
      ? (formula.authored_source ?? formula.source)
      : (formula.current_text ?? formula.source),
  };
  if (
    formula.canonical !== undefined &&
    (exactBound || formula.current_text === undefined || formula.current_text === formula.canonical)
  )
    descriptor.canonical = formula.canonical;
  if (formula.ast_schema !== undefined) descriptor.ast_schema = formula.ast_schema;
  if (formula.compatibility_profile !== undefined)
    descriptor.compatibility_profile = formula.compatibility_profile;
  if (formula.bound_at_version !== undefined)
    descriptor.bound_at_version = formula.bound_at_version;
  if (exactBound && formula.bound_formula_wire_hex !== undefined)
    descriptor.bound_formula_wire_hex = formula.bound_formula_wire_hex;
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
    | {
        schema_version: 4 | 5 | 6;
        calculation_context?: PreviewContext;
        last_axis_change_version: string;
        retired_row_ids: string[];
        retired_column_ids: string[];
        row_activation_floors: { row_id: string; version: string }[];
        column_activation_floors: { column_id: string; version: string }[];
      }
  );
export type PreviewPatch = Stamp & {
  schema_version: 1;
  expected_input_version: string;
  expected_pending_revision: string;
  cells: PreviewCell[];
};
export type PreviewStyleMutation = Omit<PreviewCell, 'value'> & {
  changes: { property: keyof CellStyle; value: CellStyle[keyof CellStyle] | null }[];
};
export type PreviewAuthoredPatch = Omit<PreviewPatch, 'schema_version'> & {
  schema_version: 2;
  styles: PreviewStyleMutation[];
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
  schema_version: 1 | 2;
  preview: true;
  complete: true;
  full: boolean;
  formula_engine_version: string;
  compatibility_profile: string;
  normalized_result_hash: string;
  normalized_authored_state_hash?: string;
  evaluated_cells: number;
  dirty_cells: number;
  results: (Omit<PreviewCell, 'value'> & { value: CalculatedValue })[];
  cleared: Omit<PreviewCell, 'value'>[];
  move_writes?: PreviewCell[];
};
export type Request =
  | { id: number; kind: 'init'; data: PreviewInit }
  | { id: number; kind: 'apply'; data: PreviewPatch }
  | { id: number; kind: 'patch'; data: PreviewAuthoredPatch }
  | { id: number; kind: 'move'; data: PreviewMove };
export type Reply =
  | { id: number; result: PreviewResult }
  | { id: number; error: string; phase: 'init' | 'apply' | 'patch' | 'move' | 'calculate' };
export const addressKey = (cell: Omit<PreviewCell, 'value'>) =>
  `${cell.sheet_id}/${cell.row_id}/${cell.column_id}`;
