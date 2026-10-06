import type { DocumentStore } from '../document/store';
import { GRID, axisMetrics, withAxisSize, type AxisLayout } from './viewport';

type Geometry = { rows: AxisLayout; columns: AxisLayout };
export type AxisResize = { sheetId: string; axisId: string; axis: 'row' | 'column'; size: number };
const cache = new WeakMap<
  DocumentStore,
  {
    rows: AxisLayout;
    columns: AxisLayout;
    pending: string;
    geometry: Geometry;
  }
>();

/** Confirmed axes plus a separate pending-size overlay, shared by paint and hit testing. */
export function sheetMetrics(
  document: DocumentStore | undefined,
  resize?: AxisResize,
): Geometry | undefined {
  const sheet = document?.sheet;
  if (!document || !sheet) return undefined;
  const rows = axisMetrics(
    sheet.rows,
    sheet.row_heights,
    GRID.rowHeight,
    document.filterState.hidden,
  );
  const columns = axisMetrics(sheet.columns, sheet.column_widths, GRID.columnWidth);
  const sizes = document.pending.filter((op) => op.kind === 'axisSize' && op.sheet === sheet.id);
  const pending = sizes.map((op) => op.id).join('/');
  const previous = cache.get(document);
  let geometry =
    previous?.rows === rows && previous.columns === columns && previous.pending === pending
      ? previous.geometry
      : undefined;
  if (!geometry) {
    geometry = { rows, columns };
    const latest = new Map<string, (typeof sizes)[number]>();
    for (const op of sizes) if (op.kind === 'axisSize') latest.set(`${op.axis}/${op.axisId}`, op);
    for (const op of latest.values()) {
      if (op.kind !== 'axisSize') continue;
      const ids = op.axis === 'row' ? sheet.rows : sheet.columns;
      const index = ids.indexOf(op.axisId);
      if (op.axis === 'row')
        geometry.rows = withAxisSize(geometry.rows, index, op.size ?? GRID.rowHeight);
      else geometry.columns = withAxisSize(geometry.columns, index, op.size ?? GRID.columnWidth);
    }
    cache.set(document, { rows, columns, pending, geometry });
  }
  if (resize?.sheetId === sheet.id) {
    const ids = resize.axis === 'row' ? sheet.rows : sheet.columns;
    const index = ids.indexOf(resize.axisId);
    return resize.axis === 'row'
      ? { ...geometry, rows: withAxisSize(geometry.rows, index, resize.size) }
      : { ...geometry, columns: withAxisSize(geometry.columns, index, resize.size) };
  }
  return geometry;
}
