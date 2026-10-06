import { isUuid } from '../sharing/api';
export type ProfileAddress = {
  sheet_id: string;
  row_id: string;
  column_id: string;
  sheet_name: string;
  row: number;
  column: number;
};
export type ProfileFormula = {
  address: ProfileAddress;
  start_us: number;
  duration_us: number;
  work: number;
  cycle_or_blocked: boolean;
};
export type ProfileReport = {
  schema_version: 1;
  document_version: string;
  context_id: string | null;
  calculation_epoch: string | null;
  engine_version: string;
  compatibility_profile: string;
  result_hash: string;
  preparation_us: number;
  calculation_us: number;
  total_us: number;
  preparation_work: number;
  graph_work: number;
  evaluation_work: number;
  formula_count: number;
  evaluated_cells: number;
  result_bytes: number;
  formulas: ProfileFormula[];
  formula_rows_truncated: boolean;
  graph: {
    nodes: { address: ProfileAddress; formula: boolean }[];
    edges: [number, number][];
    truncated: boolean;
    potential: true;
  };
};
const invalid = () => new Error('Некорректный отчёт профайлера.');
const integer = (value: unknown, maximum = Number.MAX_SAFE_INTEGER): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= maximum;
function address(value: ProfileAddress) {
  return (
    value &&
    [value.sheet_id, value.row_id, value.column_id].every(isUuid) &&
    typeof value.sheet_name === 'string' &&
    value.sheet_name.length <= 1024 &&
    integer(value.row, 99999) &&
    integer(value.column, 999)
  );
}
export function decodeProfile(raw: unknown): ProfileReport {
  const value = raw as ProfileReport;
  if (
    !value ||
    value.schema_version !== 1 ||
    !/^\d+$/.test(value.document_version) ||
    (value.context_id !== null && !isUuid(value.context_id)) ||
    (value.calculation_epoch !== null && !/^\d+$/.test(value.calculation_epoch)) ||
    typeof value.engine_version !== 'string' ||
    value.engine_version.length > 128 ||
    typeof value.compatibility_profile !== 'string' ||
    value.compatibility_profile.length > 128 ||
    !/^[0-9a-f]{64}$/.test(value.result_hash) ||
    ![
      'preparation_us',
      'calculation_us',
      'total_us',
      'preparation_work',
      'graph_work',
      'evaluation_work',
      'formula_count',
      'evaluated_cells',
      'result_bytes',
    ].every((key) => integer(value[key as keyof ProfileReport])) ||
    value.formula_count > 100000 ||
    value.evaluated_cells > value.formula_count ||
    typeof value.formula_rows_truncated !== 'boolean' ||
    !Array.isArray(value.formulas) ||
    value.formulas.length > 1000 ||
    value.formulas.some(
      (formula) =>
        !formula ||
        !address(formula.address) ||
        !integer(formula.start_us) ||
        !integer(formula.duration_us) ||
        !integer(formula.work, 1000000) ||
        typeof formula.cycle_or_blocked !== 'boolean',
    ) ||
    !value.graph ||
    value.graph.potential !== true ||
    typeof value.graph.truncated !== 'boolean' ||
    !Array.isArray(value.graph.nodes) ||
    value.graph.nodes.length > 500 ||
    !Array.isArray(value.graph.edges) ||
    value.graph.edges.length > 2000 ||
    value.graph.nodes.some(
      (node) => !node || !address(node.address) || typeof node.formula !== 'boolean',
    ) ||
    value.graph.edges.some(
      (edge) =>
        !Array.isArray(edge) ||
        edge.length !== 2 ||
        !edge.every((index) => integer(index, value.graph.nodes.length - 1)),
    )
  )
    throw invalid();
  return value;
}
/** A bounded iterative closure handles cycles without recursive graph traversal. */
export function dependencyChain(report: ProfileReport, root: number): number[] {
  if (!report.graph.nodes[root]) return [];
  const edges = new Map<number, number[]>();
  for (const [from, to] of report.graph.edges) edges.set(from, [...(edges.get(from) ?? []), to]);
  const seen = new Set<number>([root]),
    queue = [root];
  for (let index = 0; index < queue.length; index++)
    for (const target of edges.get(queue[index]!) ?? [])
      if (!seen.has(target)) {
        seen.add(target);
        queue.push(target);
      }
  return queue;
}
