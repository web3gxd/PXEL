import type { DocumentStore } from '../document/store';
import { GRID, backingSize, columnLabel, visibleRange } from './viewport';
import { colorCss } from '../document/styles';
import { drawCell, drawCellBorders } from './cell-style';

export function drawGrid(
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  scrollLeft: number,
  scrollTop: number,
  dpr: number,
  document?: DocumentStore,
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
  context.fillStyle = '#ffffff';
  context.fillRect(0, 0, width, height);
  const rows = visibleRange(
    scrollTop,
    height - GRID.columnHeaderHeight,
    GRID.rowHeight,
    document?.sheet?.rows.length ?? GRID.rowCount,
  );
  const columns = visibleRange(
    scrollLeft,
    width - GRID.rowHeaderWidth,
    GRID.columnWidth,
    document?.sheet?.columns.length ?? GRID.columnCount,
  );
  const x = (column: number) => GRID.rowHeaderWidth + column * GRID.columnWidth - scrollLeft;
  const y = (row: number) => GRID.columnHeaderHeight + row * GRID.rowHeight - scrollTop;
  const pixel = (value: number) => (Math.round(value * dpr) + 0.5) / dpr;
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
    for (let row = rows.start; row < rows.end; row++)
      for (let column = columns.start; column < columns.end; column++) {
        context.fillStyle = colorCss(document.style({ row, column }).fill);
        context.fillRect(x(column), y(row), GRID.columnWidth, GRID.rowHeight);
      }
    context.restore();
  }
  context.lineWidth = 1 / dpr;
  context.strokeStyle = '#e6e9e5';
  context.beginPath();
  for (let column = columns.start; column <= columns.end; column++) {
    context.moveTo(pixel(x(column)), GRID.columnHeaderHeight);
    context.lineTo(pixel(x(column)), height);
  }
  for (let row = rows.start; row <= rows.end; row++) {
    context.moveTo(GRID.rowHeaderWidth, pixel(y(row)));
    context.lineTo(width, pixel(y(row)));
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
    if (selection.startRow !== selection.endRow || selection.startColumn !== selection.endColumn) {
      context.fillStyle = 'rgba(35, 143, 98, 0.12)';
      context.fillRect(
        x(selection.startColumn),
        y(selection.startRow),
        (selection.endColumn - selection.startColumn + 1) * GRID.columnWidth,
        (selection.endRow - selection.startRow + 1) * GRID.rowHeight,
      );
      context.strokeStyle = '#17664b';
      context.lineWidth = 2;
      context.strokeRect(
        x(selection.startColumn) + 1,
        y(selection.startRow) + 1,
        (selection.endColumn - selection.startColumn + 1) * GRID.columnWidth - 2,
        (selection.endRow - selection.startRow + 1) * GRID.rowHeight - 2,
      );
    }
    for (let row = rows.start; row < rows.end; row++)
      for (let column = columns.start; column < columns.end; column++) {
        const value = document.display({ row, column });
        const style = document.style({ row, column });
        if (value)
          drawCell(
            context,
            style,
            value,
            x(column),
            y(row),
            GRID.columnWidth,
            GRID.rowHeight,
            document.displayContent({ row, column })?.type === 'number',
          );
      }
    for (let row = rows.start; row < rows.end; row++)
      for (let column = columns.start; column < columns.end; column++)
        drawCellBorders(
          context,
          document.style({ row, column }),
          x(column),
          y(row),
          GRID.columnWidth,
          GRID.rowHeight,
        );
    context.strokeStyle = '#17664b';
    context.lineWidth = 2;
    context.strokeRect(
      x(document.selected.column) + 1,
      y(document.selected.row) + 1,
      GRID.columnWidth - 2,
      GRID.rowHeight - 2,
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
        (lastColumn - firstColumn + 1) * GRID.columnWidth - 4,
        (lastRow - firstRow + 1) * GRID.rowHeight - 4,
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
  for (let column = columns.start; column < columns.end; column++) {
    const left = x(column);
    if (left < GRID.rowHeaderWidth) continue;
    context.fillText(columnLabel(column), left + GRID.columnWidth / 2, GRID.columnHeaderHeight / 2);
    context.moveTo(pixel(left), 0);
    context.lineTo(pixel(left), GRID.columnHeaderHeight);
  }
  for (let row = rows.start; row < rows.end; row++) {
    const top = y(row);
    if (top < GRID.columnHeaderHeight) continue;
    context.fillText(String(row + 1), GRID.rowHeaderWidth / 2, top + GRID.rowHeight / 2);
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
  return true;
}
