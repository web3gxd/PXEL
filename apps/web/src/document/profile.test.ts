import { expect, it } from 'vitest';
import { decodeProfile, dependencyChain, type ProfileReport } from './profile';
const id = '01990000-0000-7000-8000-000000000001';
const fixture = (): ProfileReport => ({
  schema_version: 1,
  document_version: '5',
  context_id: null,
  calculation_epoch: null,
  engine_version: 'pxel-formula-0.16.0',
  compatibility_profile: 'pxel-excel-subset-v1',
  result_hash: 'ab'.repeat(32),
  preparation_us: 2,
  calculation_us: 3,
  total_us: 8,
  preparation_work: 5,
  graph_work: 5,
  evaluation_work: 4,
  formula_count: 2,
  evaluated_cells: 2,
  result_bytes: 128,
  formulas: [],
  formula_rows_truncated: false,
  graph: {
    nodes: [0, 1, 2].map((row) => ({
      address: { sheet_id: id, row_id: id, column_id: id, row, column: 0, sheet_name: 'Лист' },
      formula: true,
    })),
    edges: [
      [0, 1],
      [1, 0],
      [1, 2],
    ],
    potential: true,
    truncated: false,
  },
});
it('validates independently versioned bounded reports and indexes atomically', () => {
  expect(decodeProfile(fixture()).document_version).toBe('5');
  expect(() => decodeProfile({ ...fixture(), schema_version: 2 })).toThrow();
  expect(() => decodeProfile({ ...fixture(), total_us: NaN })).toThrow();
  const report = fixture();
  report.graph.edges.push([0, 3]);
  expect(() => decodeProfile(report)).toThrow();
});
it('walks potential dependency chains through cycles without duplicating nodes', () => {
  expect(dependencyChain(fixture(), 0)).toEqual([0, 1, 2]);
  expect(dependencyChain(fixture(), 2)).toEqual([2]);
  expect(dependencyChain(fixture(), 99)).toEqual([]);
});
