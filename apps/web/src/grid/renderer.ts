import type { DocumentStore } from '../document/store';
import { GRID, type AxisLayout, backingSize, columnLabel } from './viewport';
import { sheetMetrics, type AxisResize } from './geometry';
import { canvasFont, colorCss, isDefaultStyle } from '../document/styles';
import { drawCell, drawCellBorders } from './cell-style';

type CanvasSnapshot = {
  geometry: string;
  overlay: string;
  cells: Map<string, string>;
  overflowSpans: Map<string, number>;
};
const snapshots = new WeakMap<HTMLCanvasElement, CanvasSnapshot>();

function cellOverflowSpan(
  context: CanvasRenderingContext2D,
  document: DocumentStore,
  row: number,
  column: number,
  visibleEnd: number,
  columnAxis: AxisLayout,
  style: ReturnType<DocumentStore['style']>,
  text: string,
  content: ReturnType<DocumentStore['displayContent']>,
): { width: number; endColumn: number } {
  const ownWidth = columnAxis.sizeAt(column);
  if (
    style.text_overflow !== 'overflow' ||
    (style.horizontal !== 'general' && style.horizontal !== 'left') ||
    document.mergeAt({ row, column })
  )
    return { width: ownWidth, endColumn: column };
  if (content?.type !== 'text' || !text) return { width: ownWidth, endColumn: column };

  context.save();
  context.font = canvasFont(style);
  const desiredWidth =
    Math.max(...text.split(/\r?\n/).map((line) => context.measureText(line).width)) + 14;
  context.restore();

  let width = ownWidth;
  let endColumn = column;
  for (let next = column + 1; next < visibleEnd && width < desiredWidth; next++) {
    const nextPosition = { row, column: next };
    if (
      document.value(nextPosition) !== undefined ||
      document.display(nextPosition) !== '' ||
      !isDefaultStyle(document.style(nextPosition)) ||
      document.mergeAt(nextPosition)
    )
      break;
    width += columnAxis.sizeAt(next);
    endColumn = next;
  }
  return { width, endColumn };
}

export function drawGrid(
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  scrollLeft: number,
  scrollTop: number,
  dpr: number,
  document?: DocumentStore,
  resize?: AxisResize,
): boolean {
  const rendered = drawScrollableGrid(
    canvas,
    width,
    height,
    scrollLeft,
    scrollTop,
    dpr,
    document,
    resize,
  );
  const sheet = document?.sheet;
  if (!rendered || !sheet) return rendered;
  const rows = sheetMetrics(document, resize)!.rows;
  const frozenHeight = Math.min(
    Math.max(0, height - GRID.columnHeaderHeight),
    rows.offsetAt(Math.min(sheet.rows.length, sheet.frozen_rows ?? 0)),
  );
  canvas.dataset.frozenHeight = String(frozenHeight);
  canvas.dataset.hiddenRows = String(document.hiddenRows.size);
  canvas.dataset.hiddenColumns = String(document.hiddenColumns.size);
  if (frozenHeight > 0) {
    let layer = frozenLayers.get(canvas);
    if (!layer) {
      layer = globalThis.document.createElement('canvas');
      frozenLayers.set(canvas, layer);
    }
    if (
      !drawScrollableGrid(
        layer,
        width,
        frozenHeight + GRID.columnHeaderHeight,
        scrollLeft,
        0,
        dpr,
        document,
        resize,
      )
    )
      return false;
    const context = canvas.getContext('2d');
    if (!context) return false;
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.drawImage(
      layer,
      0,
      0,
      layer.width,
      layer.height,
      0,
      0,
      width,
      frozenHeight + GRID.columnHeaderHeight,
    );
    context.strokeStyle = '#17664b';
    context.lineWidth = 2;
    context.beginPath();
    context.moveTo(0, frozenHeight + GRID.columnHeaderHeight);
    context.lineTo(width, frozenHeight + GRID.columnHeaderHeight);
    context.stroke();
  }
  return true;
}
const frozenLayers = new WeakMap<HTMLCanvasElement, HTMLCanvasElement>();
function drawScrollableGrid(
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  scrollLeft: number,
  scrollTop: number,
  dpr: number,
  document?: DocumentStore,
  resize?: AxisResize,
): boolean {
  const context = canvas.getContext('2d', { alpha: false });
  if (!context) return false;
  const backingWidth = backingSize(width, dpr);
  const backingHeight = backingSize(height, dpr);
  if (canvas.width !== backingWidth || canvas.height !== backingHeight) {
    canvas.width = backingWidth;
    canvas.height = backingHeight;
  }
  context.setTransform(dpr, 0, 0, dpr, 0, 0);
  const sheet = document?.sheet;
  const metrics = sheetMetrics(document, resize);
  const rowAxis = metrics?.rows;
  const columnAxis = metrics?.columns;
  const rows = rowAxis?.visibleRange(scrollTop, height - GRID.columnHeaderHeight) ?? {
    start: 0,
    end: 0,
  };
  const columns = columnAxis?.visibleRange(scrollLeft, width - GRID.rowHeaderWidth) ?? {
    start: 0,
    end: 0,
  };
  const visibleColumns = [...(columnAxis?.indicesInRange(columns.start, columns.end) ?? [])];
  const x = (column: number) =>
    GRID.rowHeaderWidth + (columnAxis?.offsetAt(column) ?? column * GRID.columnWidth) - scrollLeft;
  const y = (row: number) =>
    GRID.columnHeaderHeight + (rowAxis?.offsetAt(row) ?? row * GRID.rowHeight) - scrollTop;
  const pixel = (value: number) => (Math.round(value * dpr) + 0.5) / dpr;
  const selection = document?.selectionBounds;
  const overlay =
    document && selection
      ? JSON.stringify([
          document.selected,
          selection,
          document.remotePresence,
          document.sortRangePreview,
          document.privacyFindingRevision,
        ])
      : '';
  const filtering = document?.personalView.settings.filters.some((f) => f.sheet_id === sheet?.id);
  const geometry = `${document?.projection?.document_id ?? ''}/${sheet?.id ?? ''}/${JSON.stringify([sheet?.merges ?? [], sheet?.row_heights ?? [], sheet?.column_widths ?? [], sheet?.frozen_rows ?? 0, sheet?.default_column_width, sheet?.hidden_rows, sheet?.hidden_columns, document?.pending.filter((op) => op.kind === 'axisSize' && op.sheet === sheet?.id).map((op) => op.id), resize && [resize.sheetId, resize.axisId, resize.axis, resize.size]])}/${filtering ? `${document?.personalView.revision}/${document?.projection?.version}/${document?.calculatedVersion}` : ''}/${width}/${height}/${scrollLeft}/${scrollTop}/${dpr}`;
  const cellSignatures = new Map<string, string>();
  const overflowSpans = new Map<string, number>();
  if (document?.sheet)
    for (const row of rowAxis!.indicesInRange(rows.start, rows.end))
      for (const column of visibleColumns) {
        const position = { row, column };
        const key = `${row}/${column}`;
        const text = document.privacyVeilAt(position) ? '' : document.display(position),
          content = document.displayContent(position),
          style = document.style(position);
        cellSignatures.set(key, `${JSON.stringify([text, content])}\u0000${JSON.stringify(style)}`);
        const span = cellOverflowSpan(
          context,
          document,
          row,
          column,
          columns.end,
          columnAxis!,
          style,
          text,
          content,
        );
        if (span.endColumn > column) overflowSpans.set(key, span.endColumn);
      }
  const previous = snapshots.get(canvas);
  canvas.dataset.overflowSpanCount = String(overflowSpans.size);
  const changed = new Map(
    previous
      ? [...cellSignatures].filter(([cell, signature]) => previous.cells.get(cell) !== signature)
      : [],
  );
  if (previous) {
    const affectedSources = new Set<string>();
    for (const [cell] of changed) {
      const [row, column] = cell.split('/').map(Number);
      if (row === undefined || column === undefined) continue;
      if (previous.overflowSpans.has(cell) || overflowSpans.has(cell)) affectedSources.add(cell);
      for (const spans of [previous.overflowSpans, overflowSpans])
        for (const [source, end] of spans) {
          const [sourceRow, sourceColumn] = source.split('/').map(Number);
          if (
            sourceRow === row &&
            sourceColumn !== undefined &&
            sourceColumn < column &&
            column <= end
          )
            affectedSources.add(source);
        }
    }
    for (const source of affectedSources) {
      const [row, column] = source.split('/').map(Number);
      if (row === undefined || column === undefined) continue;
      const end = Math.max(
        previous.overflowSpans.get(source) ?? column,
        overflowSpans.get(source) ?? column,
      );
      for (let downstream = column + 1; downstream <= end; downstream++) {
        const cell = `${row}/${downstream}`;
        const signature = cellSignatures.get(cell);
        if (signature !== undefined) changed.set(cell, signature);
      }
      const signature = cellSignatures.get(source);
      if (signature !== undefined) changed.set(source, signature);
    }
  }
  const dirty = [...changed].sort(([left], [right]) => {
    const [leftRow = 0, leftColumn = 0] = left.split('/').map(Number);
    const [rightRow = 0, rightColumn = 0] = right.split('/').map(Number);
    return leftRow - rightRow || rightColumn - leftColumn;
  });
  if (
    previous?.geometry === geometry &&
    previous.overlay === overlay &&
    !sheet?.merges?.length &&
    selection?.startRow === selection?.endRow &&
    selection?.startColumn === selection?.endColumn
  ) {
    if (!dirty.length) return true;
    context.save();
    context.beginPath();
    context.rect(
      GRID.rowHeaderWidth,
      GRID.columnHeaderHeight,
      width - GRID.rowHeaderWidth,
      height - GRID.columnHeaderHeight,
    );
    context.clip();
    for (const [cell, signature] of dirty) {
      const [row, column] = cell.split('/').map(Number);
      if (row === undefined || column === undefined) continue;
      const pos = { row, column },
        px = x(column),
        py = y(row),
        cellWidth = columnAxis!.sizeAt(column),
        cellHeight = rowAxis!.sizeAt(row),
        style = document!.style(pos),
        value = document!.privacyVeilAt(pos) ? '' : document!.display(pos);
      context.fillStyle = colorCss(style.fill);
      context.fillRect(px, py, cellWidth, cellHeight);
      if (
        selection &&
        row >= selection.startRow &&
        row <= selection.endRow &&
        column >= selection.startColumn &&
        column <= selection.endColumn
      ) {
        context.fillStyle = 'rgba(35, 143, 98, 0.12)';
        context.fillRect(px, py, cellWidth, cellHeight);
      }
      const content = document!.displayContent(pos);
      if (value)
        drawCell(
          context,
          style,
          value,
          px,
          py,
          cellWidth,
          cellHeight,
          content?.type === 'number',
          cellOverflowSpan(
            context,
            document!,
            row,
            column,
            columns.end,
            columnAxis!,
            style,
            value,
            content,
          ).width,
        );
      context.lineWidth = 1 / dpr;
      context.strokeStyle = '#e6e9e5';
      context.beginPath();
      context.moveTo(pixel(px), py);
      context.lineTo(pixel(px), py + cellHeight);
      context.moveTo(px, pixel(py));
      context.lineTo(px + cellWidth, pixel(py));
      context.stroke();
      drawCellBorders(context, style, px, py, cellWidth, cellHeight);

      if (row === document!.selected.row && column === document!.selected.column) {
        context.strokeStyle = '#17664b';
        context.lineWidth = 2;
        context.strokeRect(px + 1, py + 1, cellWidth - 2, cellHeight - 2);
      }
      for (const peer of document!.remotePresence) {
        const inside =
          row >= Math.min(peer.anchor.row, peer.focus.row) &&
          row <= Math.max(peer.anchor.row, peer.focus.row) &&
          column >= Math.min(peer.anchor.column, peer.focus.column) &&
          column <= Math.max(peer.anchor.column, peer.focus.column);
        if (!inside) continue;
        context.strokeStyle =
          ['#bb4b69', '#7062b0', '#267da0', '#aa672c'][peer.actorId.charCodeAt(0) % 4] ?? '#7062b0';
        context.lineWidth = 2;
        context.strokeRect(px + 2, py + 2, cellWidth - 4, cellHeight - 4);
      }
      previous.cells.set(cell, signature);
    }
    previous.overflowSpans = overflowSpans;
    context.restore();
    canvas.dataset.dirtyCellCount = String(dirty.length);
    canvas.dataset.lastPaintedArea = String(
      dirty.reduce((area, [cell]) => {
        const [r, c] = cell.split('/').map(Number);
        return area + rowAxis!.sizeAt(r ?? 0) * columnAxis!.sizeAt(c ?? 0);
      }, 0),
    );
    return true;
  }
  context.fillStyle = '#ffffff';
  context.fillRect(0, 0, width, height);
  if (document?.sheet) {
    context.save();
    context.beginPath();
    context.rect(
      GRID.rowHeaderWidth,
      GRID.columnHeaderHeight,
      width - GRID.rowHeaderWidth,
      height - GRID.columnHeaderHeight,
    );
    context.clip();
    for (const row of rowAxis!.indicesInRange(rows.start, rows.end))
      for (const column of visibleColumns) {
        context.fillStyle = colorCss(document.style({ row, column }).fill);
        context.fillRect(x(column), y(row), columnAxis!.sizeAt(column), rowAxis!.sizeAt(row));
      }
    for (const merge of document.sheet.merges ?? []) {
      const firstRow = document.sheet.rows.indexOf(merge.first_row_id),
        lastRow = document.sheet.rows.indexOf(merge.last_row_id),
        firstColumn = document.sheet.columns.indexOf(merge.first_column_id),
        lastColumn = document.sheet.columns.indexOf(merge.last_column_id);
      if (
        lastRow < rows.start ||
        firstRow >= rows.end ||
        lastColumn < columns.start ||
        firstColumn >= columns.end
      )
        continue;
      context.fillStyle = colorCss(document.style({ row: firstRow, column: firstColumn }).fill);
      context.fillRect(
        x(firstColumn),
        y(firstRow),
        columnAxis!.offsetAt(lastColumn + 1) - columnAxis!.offsetAt(firstColumn),
        rowAxis!.offsetAt(lastRow + 1) - rowAxis!.offsetAt(firstRow),
      );
    }
    context.restore();
  }
  context.lineWidth = 1 / dpr;
  context.strokeStyle = '#e6e9e5';
  context.beginPath();
  for (let column = columns.start; column <= columns.end; column++)
    for (const row of rowAxis?.indicesInRange(rows.start, rows.end) ?? []) {
      const merge = document?.mergeAt({
        row,
        column: Math.min(column, (sheet?.columns.length ?? 1) - 1),
      });
      if (merge && column > merge.startColumn && column <= merge.endColumn) continue;
      context.moveTo(pixel(x(column)), pixel(y(row)));
      context.lineTo(pixel(x(column)), pixel(y(row + 1)));
    }
  for (const row of [...(rowAxis?.indicesInRange(rows.start, rows.end) ?? []), rows.end])
    for (const column of visibleColumns) {
      const merge = document?.mergeAt({
        row: Math.min(row, (sheet?.rows.length ?? 1) - 1),
        column,
      });
      if (merge && row > merge.startRow && row <= merge.endRow) continue;
      context.moveTo(pixel(x(column)), pixel(y(row)));
      context.lineTo(pixel(x(column + 1)), pixel(y(row)));
    }
  context.stroke();

  if (document?.sheet) {
    context.save();
    context.beginPath();
    context.rect(
      GRID.rowHeaderWidth,
      GRID.columnHeaderHeight,
      width - GRID.rowHeaderWidth,
      height - GRID.columnHeaderHeight,
    );
    context.clip();
    const selection = document.selectionBounds;
    const sortPreview = document.sortRangePreview;
    if (sortPreview) {
      context.save();
      context.strokeStyle = '#238f62';
      context.lineWidth = 3;
      context.setLineDash([6, 4]);
      context.strokeRect(
        x(sortPreview.startColumn) + 1,
        y(sortPreview.startRow) + 1,
        columnAxis!.offsetAt(sortPreview.endColumn + 1) -
          columnAxis!.offsetAt(sortPreview.startColumn) -
          2,
        rowAxis!.offsetAt(sortPreview.endRow + 1) - rowAxis!.offsetAt(sortPreview.startRow) - 2,
      );
      context.restore();
    }
    if (selection.startRow !== selection.endRow || selection.startColumn !== selection.endColumn) {
      context.fillStyle = 'rgba(35, 143, 98, 0.12)';
      context.fillRect(
        x(selection.startColumn),
        y(selection.startRow),
        columnAxis!.offsetAt(selection.endColumn + 1) - columnAxis!.offsetAt(selection.startColumn),
        rowAxis!.offsetAt(selection.endRow + 1) - rowAxis!.offsetAt(selection.startRow),
      );
      context.strokeStyle = '#17664b';
      context.lineWidth = 2;
      context.strokeRect(
        x(selection.startColumn) + 1,
        y(selection.startRow) + 1,
        columnAxis!.offsetAt(selection.endColumn + 1) -
          columnAxis!.offsetAt(selection.startColumn) -
          2,
        rowAxis!.offsetAt(selection.endRow + 1) - rowAxis!.offsetAt(selection.startRow) - 2,
      );
    }
    for (const row of rowAxis!.indicesInRange(rows.start, rows.end))
      for (const column of visibleColumns) {
        const merge = document.mergeAt({ row, column });
        if (merge && (row !== merge.startRow || column !== merge.startColumn)) continue;
        const value = document.privacyVeilAt({ row, column })
          ? ''
          : document.display({ row, column });
        const style = document.style({ row, column });
        const cellX = x(column),
          cellY = y(row);
        const cellWidth = merge
          ? columnAxis!.offsetAt(merge.endColumn + 1) - columnAxis!.offsetAt(merge.startColumn)
          : columnAxis!.sizeAt(column);
        const cellHeight = merge
          ? rowAxis!.offsetAt(merge.endRow + 1) - rowAxis!.offsetAt(merge.startRow)
          : rowAxis!.sizeAt(row);
        const content = document.displayContent({ row, column });

        if (value)
          drawCell(
            context,
            style,
            value,
            cellX,
            cellY,
            cellWidth,
            cellHeight,
            content?.type === 'number',
            cellOverflowSpan(
              context,
              document,
              row,
              column,
              columns.end,
              columnAxis!,
              style,
              value,
              content,
            ).width,
          );
      }
    for (const row of rowAxis!.indicesInRange(rows.start, rows.end))
      for (const column of visibleColumns) {
        const merge = document.mergeAt({ row, column });
        if (merge && (row !== merge.startRow || column !== merge.startColumn)) continue;
        drawCellBorders(
          context,
          document.style({ row, column }),
          x(column),
          y(row),
          merge
            ? columnAxis!.offsetAt(merge.endColumn + 1) - columnAxis!.offsetAt(merge.startColumn)
            : columnAxis!.sizeAt(column),
          merge
            ? rowAxis!.offsetAt(merge.endRow + 1) - rowAxis!.offsetAt(merge.startRow)
            : rowAxis!.sizeAt(row),
        );
      }
    context.strokeStyle = '#17664b';
    context.lineWidth = 2;
    const multiple =
      selection.startRow !== selection.endRow || selection.startColumn !== selection.endColumn;
    const activeMerge = multiple ? undefined : document.mergeAt(document.selected);
    if (multiple) context.fillStyle = 'rgba(35, 143, 98, 0.18)';
    if (
      !multiple ||
      (selection.endRow - selection.startRow + 1 < document.sheet.rows.length &&
        selection.endColumn - selection.startColumn + 1 < document.sheet.columns.length)
    )
      context[multiple ? 'fillRect' : 'strokeRect'](
        x(activeMerge?.startColumn ?? document.selected.column) + 1,
        y(activeMerge?.startRow ?? document.selected.row) + 1,
        (activeMerge
          ? columnAxis!.offsetAt(activeMerge.endColumn + 1) -
            columnAxis!.offsetAt(activeMerge.startColumn)
          : columnAxis!.sizeAt(document.selected.column)) - 2,
        (activeMerge
          ? rowAxis!.offsetAt(activeMerge.endRow + 1) - rowAxis!.offsetAt(activeMerge.startRow)
          : rowAxis!.sizeAt(document.selected.row)) - 2,
      );
    for (const peer of document.remotePresence) {
      const firstRow = Math.min(peer.anchor.row, peer.focus.row);
      const lastRow = Math.max(peer.anchor.row, peer.focus.row);
      const firstColumn = Math.min(peer.anchor.column, peer.focus.column);
      const lastColumn = Math.max(peer.anchor.column, peer.focus.column);
      if (
        lastRow < rows.start ||
        firstRow >= rows.end ||
        lastColumn < columns.start ||
        firstColumn >= columns.end
      )
        continue;
      const color = ['#bb4b69', '#7062b0', '#267da0', '#aa672c'][peer.actorId.charCodeAt(0) % 4];
      context.strokeStyle = color ?? '#7062b0';
      context.lineWidth = 2;
      context.strokeRect(
        x(firstColumn) + 2,
        y(firstRow) + 2,
        columnAxis!.offsetAt(lastColumn + 1) - columnAxis!.offsetAt(firstColumn) - 4,
        rowAxis!.offsetAt(lastRow + 1) - rowAxis!.offsetAt(firstRow) - 4,
      );
    }
    context.restore();
  }

  context.fillStyle = '#f5f6f3';
  context.fillRect(0, 0, width, GRID.columnHeaderHeight);
  context.fillRect(0, 0, GRID.rowHeaderWidth, height);
  context.font = '12px "Segoe UI", sans-serif';
  context.fillStyle = '#66736b';
  context.textAlign = 'center';
  context.textBaseline = 'middle';
  context.strokeStyle = '#dfe4dd';
  context.beginPath();
  for (const column of visibleColumns) {
    const left = x(column);
    if (left < GRID.rowHeaderWidth) continue;
    const isWholeColumn =
      selection?.startRow === 0 &&
      selection.endRow === (document?.sheet?.rows.length ?? 0) - 1 &&
      selection.startColumn === column &&
      selection.endColumn === column;
    if (isWholeColumn) {
      context.fillStyle = '#e3f0e6';
      context.fillRect(left, 0, columnAxis!.sizeAt(column), GRID.columnHeaderHeight);
      context.fillStyle = '#17664b';
      context.font = '600 12px "Segoe UI", sans-serif';
    } else {
      context.fillStyle = '#66736b';
      context.font = '12px "Segoe UI", sans-serif';
    }
    context.fillText(
      columnLabel(column),
      left + columnAxis!.sizeAt(column) / 2,
      GRID.columnHeaderHeight / 2,
    );
    context.font = '12px "Segoe UI", sans-serif';
    context.fillStyle = '#66736b';
    context.moveTo(pixel(left), 0);
    context.lineTo(pixel(left), GRID.columnHeaderHeight);
  }
  for (const row of rowAxis?.indicesInRange(rows.start, rows.end) ?? []) {
    const top = y(row);
    if (top < GRID.columnHeaderHeight) continue;
    context.fillText(String(row + 1), GRID.rowHeaderWidth / 2, top + rowAxis!.sizeAt(row) / 2);
    context.moveTo(0, pixel(top));
    context.lineTo(GRID.rowHeaderWidth, pixel(top));
  }
  context.moveTo(0, pixel(GRID.columnHeaderHeight));
  context.lineTo(width, pixel(GRID.columnHeaderHeight));
  context.moveTo(pixel(GRID.rowHeaderWidth), 0);
  context.lineTo(pixel(GRID.rowHeaderWidth), height);
  context.stroke();
  context.fillStyle = '#b6c0b6';
  context.beginPath();
  context.moveTo(31, 8);
  context.lineTo(31, 20);
  context.lineTo(19, 20);
  context.closePath();
  context.fill();
  snapshots.set(canvas, { geometry, overlay, cells: cellSignatures, overflowSpans });
  canvas.dataset.dirtyCellCount = String(cellSignatures.size);
  canvas.dataset.lastPaintedArea = String(width * height);
  return true;
}
