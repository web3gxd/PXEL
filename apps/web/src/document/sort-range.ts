import type { Sheet } from './store';

export type SortBounds = {
  startRow: number;
  endRow: number;
  startColumn: number;
  endColumn: number;
};
export type SortKey = { column: number; descending: boolean };

export function validSortBounds(sheet: Sheet, bounds: SortBounds) {
  return (
    Object.values(bounds).every(Number.isInteger) &&
    bounds.startRow >= 0 &&
    bounds.startColumn >= 0 &&
    bounds.endRow >= bounds.startRow &&
    bounds.endColumn >= bounds.startColumn &&
    bounds.endRow < sheet.rows.length &&
    bounds.endColumn < sheet.columns.length &&
    (bounds.endRow - bounds.startRow + 1) * (bounds.endColumn - bounds.startColumn + 1) <= 100_000
  );
}

/** A bounded suggestion is independent of the user's selection and needs explicit confirmation. */
export function suggestSortBounds(sheet: Sheet, selected: SortBounds): SortBounds | undefined {
  if (!validSortBounds(sheet, selected) || selected.startColumn !== selected.endColumn) return;
  const rows = new Map(sheet.rows.map((id, index) => [id, index]));
  const columns = new Map(sheet.columns.map((id, index) => [id, index]));
  const occupied = new Set<string>();
  for (const cell of sheet.cells) {
    if (cell.value.type === 'blank' || (cell.value.type === 'text' && cell.value.value === ''))
      continue;
    const row = rows.get(cell.row_id),
      column = columns.get(cell.column_id);
    if (row !== undefined && column !== undefined) occupied.add(`${row}:${column}`);
  }
  const queue: [number, number][] = [];
  const visited = new Set<string>();
  const include = (row: number, column: number) => {
    const key = `${row}:${column}`;
    if (occupied.has(key) && !visited.has(key)) {
      visited.add(key);
      queue.push([row, column]);
    }
  };
  for (let row = selected.startRow; row <= selected.endRow; row++)
    include(row, selected.startColumn);
  const result = { ...selected };
  for (let index = 0; index < queue.length; index++) {
    if (queue.length > 100_000) return;
    const [row, column] = queue[index]!;
    result.startRow = Math.min(result.startRow, row);
    result.endRow = Math.max(result.endRow, row);
    result.startColumn = Math.min(result.startColumn, column);
    result.endColumn = Math.max(result.endColumn, column);
    include(row - 1, column);
    include(row + 1, column);
    include(row, column - 1);
    include(row, column + 1);
  }
  return result.startColumn !== result.endColumn && validSortBounds(sheet, result)
    ? result
    : undefined;
}
