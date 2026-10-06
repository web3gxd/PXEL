export type FillBounds = {
  startRow: number;
  endRow: number;
  startColumn: number;
  endColumn: number;
};
export type FillPlan = {
  source: FillBounds;
  destination: FillBounds;
  axis: 'row' | 'column';
  range: FillBounds;
};

export function fillPlan(
  source: FillBounds,
  target: { row: number; column: number },
  rows: number,
  columns: number,
  series = false,
): FillPlan | undefined {
  if (
    Object.values(source).some((n) => !Number.isInteger(n) || n < 0) ||
    !Number.isInteger(target.row) ||
    !Number.isInteger(target.column) ||
    source.startRow > source.endRow ||
    source.startColumn > source.endColumn ||
    source.endRow >= rows ||
    source.endColumn >= columns
  )
    return;
  const row = Math.max(0, Math.min(rows - 1, target.row)),
    column = Math.max(0, Math.min(columns - 1, target.column));
  const dr = row < source.startRow ? row - source.startRow : Math.max(0, row - source.endRow);
  const dc =
    column < source.startColumn
      ? column - source.startColumn
      : Math.max(0, column - source.endColumn);
  if (!dr && !dc) return;
  const axis = Math.abs(dr) >= Math.abs(dc) ? 'row' : 'column';
  const destination = { ...source };
  const start = axis === 'row' ? 'startRow' : 'startColumn',
    end = axis === 'row' ? 'endRow' : 'endColumn';
  const delta = axis === 'row' ? dr : dc;
  const block = series ? 1 : source[end] - source[start] + 1;
  const available = delta > 0 ? (axis === 'row' ? rows : columns) - source[end] - 1 : source[start];
  const count = Math.min(Math.ceil(Math.abs(delta) / block), Math.floor(available / block)) * block;
  if (count < 1) return;
  destination[start] = delta > 0 ? source[end] + 1 : source[start] - count;
  destination[end] = delta > 0 ? source[end] + count : source[start] - 1;
  if (
    (destination.endRow - destination.startRow + 1) *
      (destination.endColumn - destination.startColumn + 1) >
    1000
  )
    return;
  return {
    source,
    destination,
    axis,
    range: {
      startRow: Math.min(source.startRow, destination.startRow),
      endRow: Math.max(source.endRow, destination.endRow),
      startColumn: Math.min(source.startColumn, destination.startColumn),
      endColumn: Math.max(source.endColumn, destination.endColumn),
    },
  };
}

export function numericSeries(
  seed: readonly number[],
  offset: number,
  count: number,
): number[] | undefined {
  if (
    seed.length < 2 ||
    !seed.every(Number.isFinite) ||
    !Number.isInteger(count) ||
    count < 1 ||
    count > 1000
  )
    return;
  const step = seed[1]! - seed[0]!;
  if (
    !Number.isFinite(step) ||
    seed.some(
      (n, i) =>
        Math.abs(n - (seed[0]! + step * i)) > Math.max(1, Math.abs(n), Math.abs(step)) * 1e-10,
    )
  )
    return;
  const result = Array.from({ length: count }, (_, i) =>
    Number((seed[0]! + step * (offset + i)).toPrecision(15)),
  );
  return result.every(Number.isFinite) ? result : undefined;
}
