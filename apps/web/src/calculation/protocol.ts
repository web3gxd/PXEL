import type { CalculatedValue, Sheet, Value } from '../document/store';

export type Stamp = { document_id: string; input_version: string; pending_revision: string };
export type PreviewCell = { sheet_id: string; row_id: string; column_id: string; value: Value };
export type PreviewInit = Stamp & { schema_version: 1 | 2; sheets: Sheet[] };
export type PreviewPatch = Stamp & {
  schema_version: 1;
  expected_input_version: string;
  expected_pending_revision: string;
  cells: PreviewCell[];
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
};
export type Request =
  | { id: number; kind: 'init'; data: PreviewInit }
  | { id: number; kind: 'apply'; data: PreviewPatch };
export type Reply =
  | { id: number; result: PreviewResult }
  | { id: number; error: string; phase: 'init' | 'apply' | 'calculate' };
export const addressKey = (cell: Omit<PreviewCell, 'value'>) =>
  `${cell.sheet_id}/${cell.row_id}/${cell.column_id}`;
