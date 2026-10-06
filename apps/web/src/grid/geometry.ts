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
  const rows = axisMetrics(sheet.rows, sheet.row_heights, GRID.rowHeight, document.hiddenRows);
  const columns = axisMetrics(
    sheet.columns,
    sheet.column_widths,
    sheet.default_column_width ?? GRID.columnWidth,
    document.hiddenColumns,
  );
  const sizes = document.pending.filter((op) => op.kind === 'axisSize' && op.sheet === sheet.id);
  const pending = sizes.map((op) => op.id).join('/');
  const previous = cache.get(document);
  let geometry =
    previous?.rows === rows && previous.columns === columns && previous.pending === pending
      ? previous.geometry
      : undefined;
  if (!geometry) {
    geometry = { rows, columns };
    type Write = { axis: 'row' | 'column'; axisId: string; size: number | undefined };
    const latest = new Map<string, Write>();
    for (const op of sizes)
      if (op.kind === 'axisSize')
        for (const write of op.writes) latest.set(`${write.axis}/${write.axisId}`, write);
    const overlay = (axis: 'row' | 'column', base: AxisLayout) => {
      const writes = [...latest.values()].filter((write) => write.axis === axis);
      if (!writes.length) return base;
      const overrides = new Map(
        (axis === 'row' ? sheet.row_heights : sheet.column_widths)?.map((override) => [
          override.axis_id,
          override,
        ]),
      );
      for (const write of writes) {
        if (write.size === undefined) overrides.delete(write.axisId);
        else overrides.set(write.axisId, { axis_id: write.axisId, size_css_px: write.size });
      }
      return axisMetrics(
        axis === 'row' ? sheet.rows : sheet.columns,
        [...overrides.values()],
        axis === 'row' ? GRID.rowHeight : (sheet.default_column_width ?? GRID.columnWidth),
        axis === 'row' ? document.hiddenRows : document.hiddenColumns,
      );
    };
    geometry = { rows: overlay('row', rows), columns: overlay('column', columns) };
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
