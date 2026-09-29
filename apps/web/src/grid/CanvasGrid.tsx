import type { DocumentStore } from '../document/store';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { drawGrid } from './renderer';
import { GRID, columnLabel } from './viewport';

export function CanvasGrid({ document }: { document?: DocumentStore }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const dragRef = useRef<
    { id: number; frame: number | undefined; row: number; column: number } | undefined
  >(undefined);
  const [scroll, setScroll] = useState({ left: 0, top: 0 });
  const [canvasAvailable, setCanvasAvailable] = useState(true);

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
      canvas.style.width = `${scroller.clientWidth}px`;
      canvas.style.height = `${scroller.clientHeight}px`;
      setScroll({ left: scroller.scrollLeft, top: scroller.scrollTop });
      const rendered = drawGrid(
        canvas,
        scroller.clientWidth,
        scroller.clientHeight,
        scroller.scrollLeft,
        scroller.scrollTop,
        window.devicePixelRatio || 1,
        document,
      );
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
    const watchDpr = () => {
      dprQuery?.removeEventListener('change', watchDpr);
      dprQuery = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dprQuery.addEventListener('change', watchDpr);
      schedule();
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(scroller);
    const onScroll = () => {
      schedule();
      const sheet = document?.sheet;
      if (
        sheet &&
        GRID.columnHeaderHeight +
          sheet.rows.length * GRID.rowHeight -
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
    if (!document?.sheet) return;
    const next = {
      row: Math.max(0, Math.min(document.sheet.rows.length - 1, row)),
      column: Math.max(0, Math.min(document.sheet.columns.length - 1, column)),
    };
    document.select(next, extend);
    if (next.row >= document.sheet.rows.length - 20) document.extendRows();
    const scroller = scrollRef.current;
    if (!scroller) return;
    const x = next.column * GRID.columnWidth,
      y = next.row * GRID.rowHeight;
    if (x < scroller.scrollLeft) scroller.scrollLeft = x;
    if (x + GRID.columnWidth > scroller.scrollLeft + scroller.clientWidth - GRID.rowHeaderWidth)
      scroller.scrollLeft = x + GRID.columnWidth - scroller.clientWidth + GRID.rowHeaderWidth;
    if (y < scroller.scrollTop) scroller.scrollTop = y;
    if (y + GRID.rowHeight > scroller.scrollTop + scroller.clientHeight - GRID.columnHeaderHeight)
      scroller.scrollTop = y + GRID.rowHeight - scroller.clientHeight + GRID.columnHeaderHeight;
  };

  return (
    <section className="grid-surface" aria-label={document?.sheet?.name ?? 'Пустой лист'}>
      <div
        ref={scrollRef}
        className="grid-scroll"
        tabIndex={0}
        role="region"
        aria-label="Область таблицы"
        aria-describedby="grid-description"
        onPaste={(event) => {
          if (!document?.sheet) return;
          event.preventDefault();
          const formula = event.clipboardData.getData('application/x-pxel-formula+json');
          const range = event.clipboardData.getData('application/x-pxel-range+json');
          const tsv = event.clipboardData.getData('application/x-pxel-tsv+json');
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
          } else document.pasteTsv(event.clipboardData.getData('text/plain'), document.pasteOrigin);
        }}
        onCopy={(event) => {
          if (!document?.sheet) return;
          event.preventDefault();
          const text = document.copySelectionTsv();
          if (text === undefined) return;
          event.clipboardData.setData('text/plain', text);
          event.clipboardData.setData(
            'application/x-pxel-tsv+json',
            JSON.stringify({ schema_version: 1, text }),
          );
          const formula = document.copyFormulaPayload();
          if (formula) event.clipboardData.setData('application/x-pxel-formula+json', formula);
          const range = document.copyRangePayload();
          if (range) event.clipboardData.setData('application/x-pxel-range+json', range);
        }}
        onPointerDown={(event) => {
          if (!document?.sheet) return;
          document.commit();
          const box = event.currentTarget.getBoundingClientRect();
          const x = event.clientX - box.left - GRID.rowHeaderWidth,
            y = event.clientY - box.top - GRID.columnHeaderHeight;
          if (x < 0 || y < 0) return;
          const row = Math.floor((y + event.currentTarget.scrollTop) / GRID.rowHeight);
          const column = Math.floor((x + event.currentTarget.scrollLeft) / GRID.columnWidth);
          move(row, column);
          dragRef.current = { id: event.pointerId, frame: undefined, row, column };
          event.currentTarget.setPointerCapture(event.pointerId);
          event.currentTarget.focus();
        }}
        onPointerMove={(event) => {
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) return;
          const box = event.currentTarget.getBoundingClientRect();
          drag.row = Math.floor(
            (event.clientY - box.top - GRID.columnHeaderHeight + event.currentTarget.scrollTop) /
              GRID.rowHeight,
          );
          drag.column = Math.floor(
            (event.clientX - box.left - GRID.rowHeaderWidth + event.currentTarget.scrollLeft) /
              GRID.columnWidth,
          );
          drag.frame ??= requestAnimationFrame(() => {
            drag.frame = undefined;
            move(drag.row, drag.column, true);
          });
        }}
        onPointerUp={(event) => {
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) return;
          if (drag.frame !== undefined) cancelAnimationFrame(drag.frame);
          move(drag.row, drag.column, true);
          dragRef.current = undefined;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={(event) => {
          const drag = dragRef.current;
          if (!drag || drag.id !== event.pointerId) return;
          if (drag.frame !== undefined) cancelAnimationFrame(drag.frame);
          dragRef.current = undefined;
        }}
        onDoubleClick={() => document?.begin()}
        onKeyDown={(event) => {
          if (!document?.sheet) return;
          if (
            (event.ctrlKey || event.metaKey) &&
            !event.altKey &&
            !event.shiftKey &&
            event.key.toLowerCase() === 'z'
          ) {
            event.preventDefault();
            document.undo();
            return;
          }
          if (event.ctrlKey || event.metaKey || event.altKey) return;
          const { row, column } = document.selected;
          const moves: Record<string, [number, number]> = {
            ArrowDown: [1, 0],
            ArrowUp: [-1, 0],
            ArrowLeft: [0, -1],
            ArrowRight: [0, 1],
            Enter: [event.shiftKey ? -1 : 1, 0],
            Tab: [0, event.shiftKey ? -1 : 1],
          };
          const delta = moves[event.key];
          if (delta) {
            event.preventDefault();
            move(
              row + delta[0],
              column + delta[1],
              event.shiftKey && event.key.startsWith('Arrow'),
            );
          } else if (event.key === 'F2') {
            event.preventDefault();
            document.begin();
          } else if (event.key === 'Delete' || event.key === 'Backspace') {
            event.preventDefault();
            document.set(document.selected, { type: 'blank' });
          } else if (event.key.length === 1) {
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
              GRID.rowHeaderWidth +
              (document?.sheet?.columns.length ?? GRID.columnCount) * GRID.columnWidth,
            height:
              GRID.columnHeaderHeight +
              (document?.sheet?.rows.length ?? GRID.rowCount) * GRID.rowHeight,
          }}
        />
      </div>
      <canvas ref={canvasRef} className="grid-canvas" aria-hidden="true" />
      {document?.editing?.source === 'cell' && (
        <textarea
          ref={editorRef}
          className="cell-editor"
          aria-label="Редактор ячейки"
          value={document.editing.text}
          style={{
            left:
              GRID.rowHeaderWidth +
              document.editing.position.column * GRID.columnWidth -
              scroll.left,
            top:
              GRID.columnHeaderHeight + document.editing.position.row * GRID.rowHeight - scroll.top,
            width: GRID.columnWidth,
            height: GRID.rowHeight,
          }}
          onChange={(event) => document.draft(event.target.value)}
          onBlur={() => document.commit()}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
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
          {columnLabel(document.selected.column)}
          {document.selected.row + 1}: {document.display(document.selected)}
        </output>
      )}
      {document?.sheet &&
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
          ? 'Стрелки и Tab перемещают выделение. Shift со стрелками выделяет диапазон, Ctrl+C и Ctrl+V копируют и вставляют значения. Ctrl+Z отменяет последнюю сохраненную правку ячейки. F2 или двойной щелчок открывают редактор. Enter сохраняет, Escape отменяет ввод.'
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
