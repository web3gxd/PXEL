import type { DocumentStore } from '../document/store';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { drawGrid } from './renderer';
import { GRID, axisMetrics, columnLabel } from './viewport';
import type { ShortcutRegistry } from '../commands/registry';
import type { GridNavigation } from '../commands/spreadsheet';
import type { Position } from '../document/store';
import { parseHtmlTable } from '../clipboard/tabular';
import { readInternalClipboard, writeSelectionClipboard } from '../clipboard/outbound';
import { SmartScrollbar } from './SmartScrollbar';
import { axisScrollMapping } from './scrollbar';
import { PrivacyVeil } from './PrivacyVeil';
import { visiblePrivacyRect } from './privacy-geometry';
import { FillHandle } from './FillHandle';
import { AssistantPanel } from '../assistant/AssistantPanel';

export function CanvasGrid({
  document,
  commands,
  navigation,
  onMarkdownPaste,
  importPreview,
}: {
  document?: DocumentStore;
  commands?: ShortcutRegistry;
  navigation?: GridNavigation;
  onMarkdownPaste?: (text: string, file: boolean) => Promise<boolean>;
  importPreview?: { origin: Position; rows: number; columns: number } | undefined;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const dragRef = useRef<
    { id: number; frame: number | undefined; row: number; column: number } | undefined
  >(undefined);
  const resizeRef = useRef<
    | { id: number; axis: 'row' | 'column'; index: number; start: number; initial: number }
    | undefined
  >(undefined);
  const [resizePreview, setResizePreview] = useState<{
    axis: 'row' | 'column';
    index: number;
    size: number;
    x: number;
    y: number;
  }>();
  const [scroll, setScroll] = useState({ left: 0, top: 0 });
  const [viewportSize, setViewportSize] = useState({ width: 0, height: 0 });
  const [canvasAvailable, setCanvasAvailable] = useState(true);
  const reactRenderCount = useRef(0);
  reactRenderCount.current++;
  const rowAxis = document?.sheet
    ? axisMetrics(
        document.sheet.rows,
        document.sheet.row_heights,
        GRID.rowHeight,
        document.filterState.hidden,
      )
    : undefined;
  const columnAxis = document?.sheet
    ? axisMetrics(document.sheet.columns, document.sheet.column_widths, GRID.columnWidth)
    : undefined;
  const frozenCount = Math.min(
    document?.sheet?.rows.length ?? 0,
    document?.sheet?.frozen_rows ?? 0,
  );
  const frozenHeight = Math.min(
    Math.max(0, viewportSize.height - GRID.columnHeaderHeight),
    rowAxis?.offsetAt(frozenCount) ?? 0,
  );
  const rowY = (row: number, top = scroll.top) =>
    (rowAxis?.offsetAt(row) ?? row * GRID.rowHeight) - (row < frozenCount ? 0 : top);
  const rowAt = (y: number, top: number) => rowAxis?.indexAt(y + (y < frozenHeight ? 0 : top)) ?? 0;
  const rowScrollMapping =
    document?.sheet && rowAxis
      ? axisScrollMapping(
          document.sheet,
          'row',
          rowAxis,
          viewportSize.height - GRID.columnHeaderHeight,
        )
      : undefined;
  const columnScrollMapping =
    document?.sheet && columnAxis
      ? axisScrollMapping(
          document.sheet,
          'column',
          columnAxis,
          viewportSize.width - GRID.rowHeaderWidth,
        )
      : undefined;

  const veils: {
    position: Position;
    left: number;
    top: number;
    width: number;
    height: number;
    contentLeft: number;
    contentTop: number;
    contentWidth: number;
    contentHeight: number;
  }[] = [];
  if (document?.privacyMask && document.sheet && rowAxis && columnAxis) {
    const visible = rowAxis.visibleRange(scroll.top, viewportSize.height - GRID.columnHeaderHeight);
    const columns = columnAxis.visibleRange(scroll.left, viewportSize.width - GRID.rowHeaderWidth);
    const rows = new Set([
      ...rowAxis.indicesInRange(visible.start, visible.end),
      ...rowAxis.indicesInRange(0, Math.min(frozenCount, rowAxis.indexAt(viewportSize.height) + 1)),
    ]);
    const seen = new Set<string>();
    for (const row of rows)
      for (let column = columns.start; column < columns.end; column++) {
        const merge = document.mergeAt({ row, column });
        const position = merge
          ? { row: merge.startRow, column: merge.startColumn }
          : { row, column };
        const key = `${position.row}/${position.column}`;
        if (seen.has(key) || !document.privacyVeilAt(position)) continue;
        seen.add(key);
        const topBoundary =
          GRID.columnHeaderHeight + (position.row < frozenCount ? 0 : frozenHeight);
        const bottomBoundary =
          position.row < frozenCount ? GRID.columnHeaderHeight + frozenHeight : viewportSize.height;
        const cell = {
          left: GRID.rowHeaderWidth + columnAxis.offsetAt(position.column) - scroll.left,
          top: GRID.columnHeaderHeight + rowY(position.row),
          width: merge
            ? columnAxis.offsetAt(merge.endColumn + 1) - columnAxis.offsetAt(position.column)
            : columnAxis.sizeAt(column),
          height: merge
            ? rowAxis.offsetAt(merge.endRow + 1) - rowAxis.offsetAt(position.row)
            : rowAxis.sizeAt(row),
        };
        const rect = visiblePrivacyRect(cell, {
          left: GRID.rowHeaderWidth,
          top: topBoundary,
          width: viewportSize.width - GRID.rowHeaderWidth,
          height: bottomBoundary - topBoundary,
        });
        if (rect)
          veils.push({
            position,
            ...rect,
            contentLeft: cell.left + 1 - rect.left,
            contentTop: cell.top + 1 - rect.top,
            contentWidth: cell.width - 2,
            contentHeight: cell.height - 2,
          });
      }
  }

  useEffect(() => {
    scrollRef.current?.scrollTo(0, 0);
  }, [document?.activeSheetId]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const scroller = scrollRef.current;
    if (!canvas || !scroller) return;
    let frame: number | undefined;
    let dprQuery: MediaQueryList;

    const render = () => {
      frame = undefined;
      const startedAt = performance.now();
      canvas.style.width = `${scroller.clientWidth}px`;
      canvas.style.height = `${scroller.clientHeight}px`;
      setScroll((current) =>
        current.left === scroller.scrollLeft && current.top === scroller.scrollTop
          ? current
          : { left: scroller.scrollLeft, top: scroller.scrollTop },
      );
      const rendered = drawGrid(
        canvas,
        scroller.clientWidth,
        scroller.clientHeight,
        scroller.scrollLeft,
        scroller.scrollTop,
        window.devicePixelRatio || 1,
        document,
      );
      canvas.dataset.canvasDrawCalls = String(Number(canvas.dataset.canvasDrawCalls ?? 0) + 1);
      canvas.dataset.reactRenderCount = String(reactRenderCount.current);
      canvas.dataset.paintedViewportPixels = String(scroller.clientWidth * scroller.clientHeight);
      canvas.dataset.canvasDurationMs = (performance.now() - startedAt).toFixed(2);
      const visibleRows = document?.sheet
        ? axisMetrics(
            document.sheet.rows,
            document.sheet.row_heights,
            GRID.rowHeight,
            document.filterState.hidden,
          ).visibleRange(scroller.scrollTop, scroller.clientHeight - GRID.columnHeaderHeight)
        : { start: 0, end: 0 };
      const visibleColumns = document?.sheet
        ? axisMetrics(
            document.sheet.columns,
            document.sheet.column_widths,
            GRID.columnWidth,
          ).visibleRange(scroller.scrollLeft, scroller.clientWidth - GRID.rowHeaderWidth)
        : { start: 0, end: 0 };
      canvas.dataset.visibleCellCount = String(
        (document?.sheet
          ? new Set([
              ...axisMetrics(
                document.sheet.rows,
                document.sheet.row_heights,
                GRID.rowHeight,
                document.filterState.hidden,
              ).indicesInRange(visibleRows.start, visibleRows.end),
              ...axisMetrics(
                document.sheet.rows,
                document.sheet.row_heights,
                GRID.rowHeight,
                document.filterState.hidden,
              ).indicesInRange(
                0,
                Math.min(
                  document.sheet.frozen_rows ?? 0,
                  axisMetrics(
                    document.sheet.rows,
                    document.sheet.row_heights,
                    GRID.rowHeight,
                    document.filterState.hidden,
                  ).indexAt(Math.max(0, scroller.clientHeight - GRID.columnHeaderHeight)) + 1,
                ),
              ),
            ]).size
          : 0) *
          (visibleColumns.end - visibleColumns.start),
      );
      if (document) {
        canvas.dataset.optimisticUpdateMs =
          document.performanceMetrics.optimisticUpdateMs.toFixed(2);
        canvas.dataset.serverConfirmationMs =
          document.performanceMetrics.serverConfirmationMs.toFixed(2);
        canvas.dataset.calculationPatchMs =
          document.performanceMetrics.calculationPatchMs.toFixed(2);
        canvas.dataset.recalculatedCells = String(document.performanceMetrics.recalculatedCells);
        canvas.dataset.dirtyCells = String(document.performanceMetrics.dirtyCells);
        canvas.dataset.serverDirtyCells = String(document.performanceMetrics.dirtyCells);
      }
      if (!rendered) setCanvasAvailable(false);
      if (rendered && !canvas.dataset.ready) {
        canvas.dataset.ready = 'true';
        performance.mark('pxel:grid-ready');
      }
      const documentId = document?.projection?.document_id;
      if (
        rendered &&
        document?.sheet &&
        documentId &&
        canvas.dataset.documentReady !== documentId
      ) {
        canvas.dataset.documentReady = documentId;
        performance.mark('pxel:document-grid-ready');
      }
    };
    const schedule = () => {
      frame ??= requestAnimationFrame(render);
    };
    const updateViewportSize = () => {
      const next = { width: scroller.clientWidth, height: scroller.clientHeight };
      setViewportSize((current) =>
        current.width === next.width && current.height === next.height ? current : next,
      );
    };
    const watchDpr = () => {
      dprQuery?.removeEventListener('change', watchDpr);
      dprQuery = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dprQuery.addEventListener('change', watchDpr);
      schedule();
    };
    const observer = new ResizeObserver(() => {
      updateViewportSize();
      schedule();
    });
    observer.observe(scroller);
    updateViewportSize();
    const onScroll = () => {
      schedule();
      const sheet = document?.sheet;
      const axis = sheet
        ? axisMetrics(sheet.rows, sheet.row_heights, GRID.rowHeight, document?.filterState.hidden)
        : undefined;
      if (
        sheet &&
        (axis?.totalSize ?? sheet.rows.length * GRID.rowHeight) -
          (scroller.scrollTop + scroller.clientHeight) <=
          20 * GRID.rowHeight
      )
        document?.extendRows();
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    watchDpr();
    const unsubscribe = document?.subscribe(schedule);
    return () => {
      unsubscribe?.();
      observer.disconnect();
      scroller.removeEventListener('scroll', onScroll);
      dprQuery.removeEventListener('change', watchDpr);
      if (frame !== undefined) cancelAnimationFrame(frame);
    };
  }, [document]);

  useLayoutEffect(() => {
    if (document?.editing?.source === 'cell') {
      editorRef.current?.focus();
      const length = editorRef.current?.value.length ?? 0;
      editorRef.current?.setSelectionRange(length, length);
    }
  }, [document?.editing?.source]);
  const move = (row: number, column: number, extend = false) => {
    if (!document?.sheet || document.allRowsHidden) return;
    const next = {
      row: Math.max(0, Math.min(document.sheet.rows.length - 1, row)),
      column: Math.max(0, Math.min(document.sheet.columns.length - 1, column)),
    };
    const rows = axisMetrics(
      document.sheet.rows,
      document.sheet.row_heights,
      GRID.rowHeight,
      document.filterState.hidden,
    );
    next.row = rows.nearestVisible(next.row, next.row < document.selected.row ? -1 : 1);
    document.select(next, extend);
    if (next.row >= document.sheet.rows.length - 20) document.extendRows();
    const scroller = scrollRef.current;
    if (!scroller) return;
    const columns = axisMetrics(
      document.sheet.columns,
      document.sheet.column_widths,
      GRID.columnWidth,
    );
    const x = columns.offsetAt(next.column),
      y = rows.offsetAt(next.row);
    const cellWidth = columns.sizeAt(next.column),
      cellHeight = rows.sizeAt(next.row);
    if (x < scroller.scrollLeft) scroller.scrollLeft = x;
    if (x + cellWidth > scroller.scrollLeft + scroller.clientWidth - GRID.rowHeaderWidth)
      scroller.scrollLeft = x + cellWidth - scroller.clientWidth + GRID.rowHeaderWidth;
    const frozen = Math.min(document.sheet.rows.length, document.sheet.frozen_rows ?? 0);
    if (next.row >= frozen && y < scroller.scrollTop + rows.offsetAt(frozen))
      scroller.scrollTop = y - rows.offsetAt(frozen);
    if (
      next.row >= frozen &&
      y + cellHeight > scroller.scrollTop + scroller.clientHeight - GRID.columnHeaderHeight
    )
      scroller.scrollTop = y + cellHeight - scroller.clientHeight + GRID.columnHeaderHeight;
  };

  useEffect(() => {
    if (navigation) navigation.move = move;
    return () => {
      if (navigation?.move === move) delete navigation.move;
    };
  });

  return (
    <section className="grid-surface" aria-label={document?.displaySheetName() ?? 'Пустой лист'}>
      <div
        ref={scrollRef}
        className="grid-scroll"
        tabIndex={0}
        role="region"
        aria-label="Область таблицы"
        aria-describedby="grid-description"
        onPaste={(event) => {
          if (!document?.sheet || document.editing) return;
          event.preventDefault();
          const internal = readInternalClipboard(event.clipboardData.getData('text/html'));
          const cut =
            event.clipboardData.getData('application/x-pxel-cut+json') || internal?.cut || '';
          document.reconcileCutClipboard(cut);
          if (cut) {
            document.pasteCutRange(cut, document.pasteOrigin);
            return;
          }
          const formula =
            event.clipboardData.getData('application/x-pxel-formula+json') || internal?.formula;
          const range =
            event.clipboardData.getData('application/x-pxel-range+json') || internal?.range;
          const tsv = event.clipboardData.getData('application/x-pxel-tsv+json') || internal?.tsv;
          if (formula) void document.pasteFormulaCopy(formula, document.pasteOrigin);
          else if (range) document.pasteRangeCopy(range, document.pasteOrigin);
          else if (tsv) {
            try {
              const internal = JSON.parse(tsv) as { schema_version?: number; text?: string };
              if (internal.schema_version === 1 && typeof internal.text === 'string')
                document.pasteTsv(internal.text, document.pasteOrigin, true);
              else
                document.pasteTsv(event.clipboardData.getData('text/plain'), document.pasteOrigin);
            } catch {
              document.pasteTsv(event.clipboardData.getData('text/plain'), document.pasteOrigin);
            }
          } else {
            try {
              const html = event.clipboardData.getData('text/html');
              const plain = event.clipboardData.getData('text/plain');
              const table = html ? parseHtmlTable(html) : undefined;
              if (table) document.pasteMatrix(table, document.pasteOrigin);
              else if (onMarkdownPaste && plain) {
                void onMarkdownPaste(plain, false)
                  .then((markdown) => {
                    if (markdown) return;
                    document.pasteTsv(plain, document.pasteOrigin);
                  })
                  .catch((error) => {
                    document.error =
                      error instanceof Error ? error.message : 'Не удалось прочитать буфер обмена.';
                    document.notify();
                  });
              } else document.pasteTsv(plain, document.pasteOrigin);
            } catch (error) {
              document.error =
                error instanceof Error ? error.message : 'Не удалось разобрать вставку.';
              document.notify();
            }
          }
        }}
        onCopy={(event) => {
          if (!document?.sheet || document.editing) return;
          event.preventDefault();
          void writeSelectionClipboard(document, 'copy');
        }}
        onCut={(event) => {
          if (!document?.sheet || document.editing) return;
          event.preventDefault();
          void writeSelectionClipboard(document, 'cut');
        }}
        onPointerDown={(event) => {
          if (!document?.sheet) return;
          document.commit();
          const box = event.currentTarget.getBoundingClientRect();
          const x = event.clientX - box.left - GRID.rowHeaderWidth,
            y = event.clientY - box.top - GRID.columnHeaderHeight;
          if (x >= 0 && y < 0) {
            const column =
              columnAxis?.indexAt(Math.max(0, x + event.currentTarget.scrollLeft - 1)) ?? 0;
            const edge = (columnAxis?.offsetAt(column + 1) ?? 0) - event.currentTarget.scrollLeft;
            if (Math.abs(x - edge) <= 5 && columnAxis) {
              resizeRef.current = {
                id: event.pointerId,
                axis: 'column',
                index: column,
                start: event.clientX,
                initial: columnAxis.sizeAt(column),
              };
              setResizePreview({
                axis: 'column',
                index: column,
                size: columnAxis.sizeAt(column),
                x: event.clientX - box.left,
                y: event.clientY - box.top,
              });
              event.currentTarget.setPointerCapture(event.pointerId);
              event.currentTarget.style.cursor = 'col-resize';
              return;
            }
            document.selectColumn(column);
            event.currentTarget.focus();
            return;
          }
          if (x < 0 && y >= 0 && rowAxis) {
            const row = rowAt(Math.max(0, y - 1), event.currentTarget.scrollTop);
            const edge = rowY(row, event.currentTarget.scrollTop) + rowAxis.sizeAt(row);
            if (Math.abs(y - edge) <= 5) {
              resizeRef.current = {
                id: event.pointerId,
                axis: 'row',
                index: row,
                start: event.clientY,
                initial: rowAxis.sizeAt(row),
              };
              setResizePreview({
                axis: 'row',
                index: row,
                size: rowAxis.sizeAt(row),
                x: event.clientX - box.left,
                y: event.clientY - box.top,
              });
              event.currentTarget.setPointerCapture(event.pointerId);
              event.currentTarget.style.cursor = 'row-resize';
            }
            return;
          }
          if (x < 0 || y < 0) return;
          const row = rowAt(y, event.currentTarget.scrollTop);
          const column = columnAxis?.indexAt(x + event.currentTarget.scrollLeft) ?? 0;
          move(row, column);
          dragRef.current = { id: event.pointerId, frame: undefined, row, column };
          event.currentTarget.setPointerCapture(event.pointerId);
          event.currentTarget.focus();
        }}
        onPointerMove={(event) => {
          const resize = resizeRef.current;
          if (resize && resize.id === event.pointerId) {
            const raw =
              resize.initial +
              (resize.axis === 'column'
                ? event.clientX - resize.start
                : event.clientY - resize.start);
            const size = Math.round(
              Math.max(
                resize.axis === 'row' ? 12 : 24,
                Math.min(resize.axis === 'row' ? 1000 : 2000, raw),
              ),
            );
            const box = event.currentTarget.getBoundingClientRect();
            setResizePreview({
              axis: resize.axis,
              index: resize.index,
              size,
              x: event.clientX - box.left,
              y: event.clientY - box.top,
            });
            return;
          }
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) {
            const box = event.currentTarget.getBoundingClientRect();
            const x = event.clientX - box.left - GRID.rowHeaderWidth;
            const y = event.clientY - box.top - GRID.columnHeaderHeight;
            const nearColumnEdge =
              x >= 0 &&
              y < 0 &&
              columnAxis &&
              Math.abs(
                x -
                  (columnAxis.offsetAt(
                    columnAxis.indexAt(x + event.currentTarget.scrollLeft - 1) + 1,
                  ) -
                    event.currentTarget.scrollLeft),
              ) <= 5;
            const nearRowEdge =
              x < 0 &&
              y >= 0 &&
              rowAxis &&
              Math.abs(
                y -
                  (rowY(
                    rowAt(Math.max(0, y - 1), event.currentTarget.scrollTop),
                    event.currentTarget.scrollTop,
                  ) +
                    rowAxis.sizeAt(rowAt(Math.max(0, y - 1), event.currentTarget.scrollTop))),
              ) <= 5;
            event.currentTarget.style.cursor = nearColumnEdge
              ? 'col-resize'
              : nearRowEdge
                ? 'row-resize'
                : '';
            return;
          }
          const box = event.currentTarget.getBoundingClientRect();
          drag.row = rowAt(
            event.clientY - box.top - GRID.columnHeaderHeight,
            event.currentTarget.scrollTop,
          );
          drag.column =
            columnAxis?.indexAt(
              event.clientX - box.left - GRID.rowHeaderWidth + event.currentTarget.scrollLeft,
            ) ?? 0;
          drag.frame ??= requestAnimationFrame(() => {
            drag.frame = undefined;
            move(drag.row, drag.column, true);
          });
        }}
        onPointerUp={(event) => {
          const resize = resizeRef.current;
          if (resize && resize.id === event.pointerId) {
            const raw =
              resize.initial +
              (resize.axis === 'column'
                ? event.clientX - resize.start
                : event.clientY - resize.start);
            const size = Math.round(
              Math.max(
                resize.axis === 'row' ? 12 : 24,
                Math.min(resize.axis === 'row' ? 1000 : 2000, raw),
              ),
            );
            document?.setAxisSize(resize.axis, resize.index, size);
            resizeRef.current = undefined;
            setResizePreview(undefined);
            event.currentTarget.style.cursor = '';
            event.currentTarget.releasePointerCapture(event.pointerId);
            return;
          }
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) return;
          if (drag.frame !== undefined) cancelAnimationFrame(drag.frame);
          move(drag.row, drag.column, true);
          dragRef.current = undefined;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={(event) => {
          if (resizeRef.current?.id === event.pointerId) {
            resizeRef.current = undefined;
            setResizePreview(undefined);
            event.currentTarget.style.cursor = '';
            return;
          }
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) return;
          if (drag.frame !== undefined) cancelAnimationFrame(drag.frame);
          dragRef.current = undefined;
        }}
        onDragOver={(event) => {
          if (
            [...event.dataTransfer.types].includes('Files') ||
            [...event.dataTransfer.types].includes('text/plain')
          )
            event.preventDefault();
        }}
        onDrop={(event) => {
          if (!document) return;
          const targetDocument = document;
          const file = event.dataTransfer.files[0];
          if (file) {
            if (!file.name.toLocaleLowerCase().endsWith('.md') && file.type !== 'text/markdown')
              return;
            event.preventDefault();
            void file
              .text()
              .then((text) => onMarkdownPaste?.(text, true))
              .then((handled) => {
                if (!handled) {
                  targetDocument.error = 'Файл не содержит распознаваемой Markdown-разметки.';
                  targetDocument.notify();
                }
              })
              .catch((error) => {
                targetDocument.error =
                  error instanceof Error ? error.message : 'Не удалось прочитать Markdown-файл.';
                targetDocument.notify();
              });
          } else {
            const text = event.dataTransfer.getData('text/plain');
            if (text && onMarkdownPaste) {
              event.preventDefault();
              void onMarkdownPaste(text, false).then((handled) => {
                if (!handled) targetDocument.pasteTsv(text, targetDocument.pasteOrigin);
              });
            }
          }
        }}
        onDoubleClick={() => document?.begin()}
        onKeyDown={(event) => {
          if (
            !document?.sheet ||
            event.target !== event.currentTarget ||
            event.nativeEvent.isComposing
          )
            return;
          if (
            (event.ctrlKey || event.metaKey) &&
            !event.altKey &&
            !event.shiftKey &&
            (event.code === 'KeyD' || event.key.toLowerCase() === 'd') &&
            !document.editing
          ) {
            event.preventDefault();
            document.fillDown();
            return;
          }
          if (commands?.handle(event.nativeEvent, 'grid')) {
            event.stopPropagation();
            return;
          }
          if (!event.ctrlKey && !event.metaKey && !event.altKey && event.key.length === 1) {
            event.preventDefault();
            document.begin(event.key);
          }
        }}
      >
        <div
          className="grid-extent"
          aria-hidden="true"
          style={{
            width:
              GRID.rowHeaderWidth + (columnAxis?.totalSize ?? GRID.columnCount * GRID.columnWidth),
            height:
              GRID.columnHeaderHeight + (rowAxis?.totalSize ?? GRID.rowCount * GRID.rowHeight),
          }}
        />
      </div>
      {document?.sheet && rowAxis && rowScrollMapping && (
        <SmartScrollbar
          axis="row"
          metrics={rowAxis}
          mapping={rowScrollMapping}
          offset={scroll.top}
          viewportSize={viewportSize.height - GRID.columnHeaderHeight}
          onOffset={(offset) => {
            if (scrollRef.current) scrollRef.current.scrollTop = offset;
          }}
        />
      )}
      {document?.sheet && columnAxis && columnScrollMapping && (
        <SmartScrollbar
          axis="column"
          metrics={columnAxis}
          mapping={columnScrollMapping}
          offset={scroll.left}
          viewportSize={viewportSize.width - GRID.rowHeaderWidth}
          onOffset={(offset) => {
            if (scrollRef.current) scrollRef.current.scrollLeft = offset;
          }}
        />
      )}
      {resizePreview && (
        <output
          className="grid-size-tooltip"
          style={{
            left: Math.min(resizePreview.x + 14, 260),
            top: Math.max(2, resizePreview.y - 30),
          }}
        >
          {resizePreview.axis === 'column'
            ? `${columnLabel(resizePreview.index)} · ширина`
            : `Строка ${resizePreview.index + 1} · высота`}{' '}
          {resizePreview.size} px
        </output>
      )}
      {importPreview && (
        <div
          className="markdown-grid-preview"
          aria-hidden="true"
          style={{
            left:
              GRID.rowHeaderWidth +
              (columnAxis?.offsetAt(importPreview.origin.column) ?? 0) -
              scroll.left,
            top: GRID.columnHeaderHeight + rowY(importPreview.origin.row),
            width:
              (columnAxis?.offsetAt(importPreview.origin.column + importPreview.columns) ?? 0) -
              (columnAxis?.offsetAt(importPreview.origin.column) ?? 0),
            height:
              (rowAxis?.offsetAt(importPreview.origin.row + importPreview.rows) ?? 0) -
              (rowAxis?.offsetAt(importPreview.origin.row) ?? 0),
          }}
        />
      )}
      <canvas
        ref={canvasRef}
        className="grid-canvas"
        aria-hidden="true"
        data-confirmed-state-hash={document?.projection?.state_hash}
        data-preview-authored-hash={document?.previewAuthoredHash}
      />
      {document &&
        veils.map((veil) => (
          <PrivacyVeil
            key={`${document.activeSheetId}/${document.projection?.version}/${document.privacyReport?.policy_revision}/${veil.position.row}/${veil.position.column}`}
            document={document}
            {...veil}
          />
        ))}
      {document?.canAutofill &&
        rowAxis &&
        columnAxis &&
        (() => {
          const bounds = document.selectionBounds;
          const left =
            GRID.rowHeaderWidth + columnAxis.offsetAt(bounds.endColumn + 1) - scroll.left;
          const top = GRID.columnHeaderHeight + rowY(bounds.endRow) + rowAxis.sizeAt(bounds.endRow);
          const topBoundary =
            GRID.columnHeaderHeight + (bounds.endRow < frozenCount ? 0 : frozenHeight);
          const bottomBoundary =
            bounds.endRow < frozenCount
              ? GRID.columnHeaderHeight + frozenHeight
              : viewportSize.height;
          return (
            <FillHandle
              document={document}
              left={left}
              top={top}
              visible={
                left > GRID.rowHeaderWidth &&
                left < viewportSize.width - 14 &&
                top > topBoundary &&
                top <= bottomBoundary - 2
              }
              locate={(x, y, autoScroll) => {
                const scroller = scrollRef.current;
                if (!scroller) return document.selected;
                const box = scroller.getBoundingClientRect();
                if (autoScroll) {
                  if (x > box.right - 24) scroller.scrollLeft += 12;
                  if (x < box.left + GRID.rowHeaderWidth + 12) scroller.scrollLeft -= 12;
                  if (y > box.bottom - 24) scroller.scrollTop += 8;
                  if (y < box.top + GRID.columnHeaderHeight + frozenHeight + 12)
                    scroller.scrollTop -= 8;
                }
                return {
                  row: rowAt(y - box.top - GRID.columnHeaderHeight, scroller.scrollTop),
                  column: columnAxis.indexAt(
                    x - box.left - GRID.rowHeaderWidth + scroller.scrollLeft,
                  ),
                };
              }}
            />
          );
        })()}
      {document?.autofillPreview &&
        rowAxis &&
        columnAxis &&
        (() => {
          const bounds = document.autofillPreview;
          const parts = [
            { start: bounds.startRow, end: Math.min(bounds.endRow, frozenCount - 1), frozen: true },
            { start: Math.max(bounds.startRow, frozenCount), end: bounds.endRow, frozen: false },
          ];
          return parts
            .filter((part) => part.start <= part.end)
            .map((part) => {
              const rect = visiblePrivacyRect(
                {
                  left: GRID.rowHeaderWidth + columnAxis.offsetAt(bounds.startColumn) - scroll.left,
                  top: GRID.columnHeaderHeight + rowY(part.start),
                  width:
                    columnAxis.offsetAt(bounds.endColumn + 1) -
                    columnAxis.offsetAt(bounds.startColumn),
                  height: rowAxis.offsetAt(part.end + 1) - rowAxis.offsetAt(part.start),
                },
                {
                  left: GRID.rowHeaderWidth,
                  top: GRID.columnHeaderHeight + (part.frozen ? 0 : frozenHeight),
                  width: viewportSize.width - GRID.rowHeaderWidth,
                  height: part.frozen
                    ? frozenHeight
                    : viewportSize.height - GRID.columnHeaderHeight - frozenHeight,
                },
              );
              return rect ? (
                <div
                  key={String(part.frozen)}
                  className="autofill-preview"
                  aria-hidden="true"
                  style={{ left: rect.left, top: rect.top, width: rect.width, height: rect.height }}
                />
              ) : null;
            });
        })()}
      {document?.sheet && <AssistantPanel document={document} commands={commands} />}
      {document?.cutStatus && (
        <output className="cut-status" aria-live="polite" data-testid="cut-status">
          {document.cutStatus === 'pending'
            ? 'Перемещение ожидает подтверждения сервера.'
            : document.cutStatus === 'stale'
              ? 'Таблица изменилась. Вырежьте диапазон ещё раз.'
              : 'Диапазон вырезан. Выберите место и нажмите Ctrl+V. Исходные ячейки пока сохранены.'}
        </output>
      )}
      {document?.editing?.source === 'cell' && (
        <textarea
          ref={editorRef}
          className="cell-editor"
          aria-label="Редактор ячейки"
          value={document.editing.text}
          style={{
            left:
              GRID.rowHeaderWidth +
              (columnAxis?.offsetAt(document.editing.position.column) ?? 0) -
              scroll.left,
            top: GRID.columnHeaderHeight + rowY(document.editing.position.row),
            width: columnAxis?.sizeAt(document.editing.position.column) ?? GRID.columnWidth,
            height: rowAxis?.sizeAt(document.editing.position.row) ?? GRID.rowHeight,
          }}
          onChange={(event) => document.draft(event.target.value)}
          onBlur={() => {
            if (!document.navigationGuardActive) document.commit();
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === 'Enter' && (event.altKey || event.metaKey)) return;
            if (event.key === 'Escape') {
              event.preventDefault();
              document.cancel();
              scrollRef.current?.focus();
            }
            if (event.key === 'Enter' || event.key === 'Tab') {
              event.preventDefault();
              document.commit();
              const { row, column } = document.selected;
              move(
                row + (event.key === 'Enter' ? (event.shiftKey ? -1 : 1) : 0),
                column + (event.key === 'Tab' ? (event.shiftKey ? -1 : 1) : 0),
              );
              scrollRef.current?.focus();
            }
          }}
        />
      )}
      {document?.sheet && (
        <output className="sr-only" aria-live="polite" data-testid="active-cell">
          {document.allRowsHidden
            ? 'Фильтр скрыл все строки.'
            : `${columnLabel(document.selected.column)}${document.selected.row + 1}: ${document.display(document.selected)}`}
        </output>
      )}
      {document?.sheet &&
        !document.allRowsHidden &&
        (document.selectionBounds.startRow !== document.selectionBounds.endRow ||
          document.selectionBounds.startColumn !== document.selectionBounds.endColumn) && (
          <output className="sr-only" aria-live="polite" data-testid="selection-range">
            Выделено {columnLabel(document.selectionBounds.startColumn)}
            {document.selectionBounds.startRow + 1}:
            {columnLabel(document.selectionBounds.endColumn)}
            {document.selectionBounds.endRow + 1}
          </output>
        )}
      <p id="grid-description" className="sr-only">
        {document?.sheet
          ? 'Стрелки и Tab перемещают выделение. Shift со стрелками выделяет диапазон, Ctrl+C и Ctrl+V копируют и вставляют значения. Ctrl+D заполняет выделение вниз из его верхней строки. Ctrl+X подготавливает перемещение в пределах листа; исходные ячейки меняются после подтверждения сервера. Ctrl+Z отменяет последнюю сохраненную правку, включая перемещение; Ctrl+Y повторяет её. F2 или двойной щелчок открывают редактор. Enter сохраняет, Escape отменяет ввод.'
          : 'Пустой лист: 1000 строк и 26 столбцов. Режим просмотра. Для прокрутки используйте клавиши со стрелками или Page Down. Создайте таблицу для редактирования.'}
      </p>
      {!canvasAvailable && (
        <p className="canvas-error" role="alert">
          Браузер не поддерживает отрисовку таблицы.
        </p>
      )}
    </section>
  );
}
