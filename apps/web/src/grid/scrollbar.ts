import { isDefaultStyle } from '../document/styles';
import type { Sheet } from '../document/store';
import { type AxisLayout, AxisScrollMapping, robustUsedEnd } from './viewport';

export type Axis = 'row' | 'column';

type UsedEnds = { rows?: number; columns?: number };
const usedEndCache = new WeakMap<Sheet, UsedEnds>();

function usedEnds(sheet: Sheet): UsedEnds {
  const cached = usedEndCache.get(sheet);
  if (cached) return cached;
  const rowPositions = new Map(sheet.rows.map((id, index) => [id, index]));
  const columnPositions = new Map(sheet.columns.map((id, index) => [id, index]));
  const rows: number[] = [];
  const columns: number[] = [];
  for (const cell of sheet.cells) {
    if (cell.value.type === 'blank') continue;
    const row = rowPositions.get(cell.row_id);
    const column = columnPositions.get(cell.column_id);
    if (row !== undefined) rows.push(row);
    if (column !== undefined) columns.push(column);
  }
  for (const cell of sheet.styles ?? []) {
    if (isDefaultStyle(cell.style)) continue;
    const row = rowPositions.get(cell.row_id);
    const column = columnPositions.get(cell.column_id);
    if (row !== undefined) rows.push(row);
    if (column !== undefined) columns.push(column);
  }
  const rowEnd = robustUsedEnd(rows, sheet.rows.length);
  const columnEnd = robustUsedEnd(columns, sheet.columns.length);
  const result: UsedEnds = {
    ...(rowEnd === undefined ? {} : { rows: rowEnd }),
    ...(columnEnd === undefined ? {} : { columns: columnEnd }),
  };
  usedEndCache.set(sheet, result);
  return result;
}

export function axisScrollMapping(
  sheet: Sheet,
  axis: Axis,
  metrics: AxisLayout,
  viewportSize: number,
) {
  const viewport = Math.max(0, Number.isFinite(viewportSize) ? viewportSize : 0);
  const totalOffset = Math.max(0, metrics.totalSize - viewport);
  const end = usedEnds(sheet)[axis === 'row' ? 'rows' : 'columns'];
  const usedEnd = end === undefined ? 0 : metrics.offsetAt(end + 1);
  const usedOffset = Math.max(0, Math.min(totalOffset, usedEnd - viewport));
  return new AxisScrollMapping(totalOffset, usedOffset);
}
