import { useRef, useState, useEffect } from 'react';
import type { DocumentStore, Position } from '../document/store';
import type { FillBounds } from './fill';

export function FillHandle({
  document,
  left,
  top,
  visible,
  locate,
}: {
  document: DocumentStore;
  left: number;
  top: number;
  visible: boolean;
  locate: (x: number, y: number, scroll: boolean) => Position;
}) {
  const gesture = useRef<
    | {
        source: FillBounds;
        version: string;
        sheet: string;
        target: Position;
        pointer?: number;
        x?: number;
        y?: number;
      }
    | undefined
  >(undefined);
  const frame = useRef<number | undefined>(undefined);
  const [preview, setPreview] = useState<Position>();
  const cancel = () => {
    if (frame.current !== undefined) cancelAnimationFrame(frame.current);
    frame.current = undefined;
    gesture.current = undefined;
    document.autofillPreview = undefined;
    setPreview(undefined);
    document.notify();
  };
  useEffect(
    () => () => {
      if (frame.current !== undefined) cancelAnimationFrame(frame.current);
      document.autofillPreview = undefined;
    },
    [document],
  );
  const begin = () => {
    if (!document.canAutofill || !document.projection || !document.sheet) return false;
    gesture.current = {
      source: { ...document.selectionBounds },
      version: document.projection.version,
      sheet: document.sheet.id,
      target: { row: document.selectionBounds.endRow, column: document.selectionBounds.endColumn },
    };
    return true;
  };
  const update = (target: Position) => {
    const current = gesture.current;
    if (!current) return;
    if (
      document.projection?.version !== current.version ||
      document.sheet?.id !== current.sheet ||
      !document.canAutofill
    ) {
      cancel();
      return;
    }
    current.target = target;
    setPreview(target);
    document.autofillPreview = document.planAutofill(current.source, target)?.destination;
    document.notify();
  };
  const finish = () => {
    const current = gesture.current;
    cancel();
    if (
      current &&
      current.sheet === document.sheet?.id &&
      document.planAutofill(current.source, current.target)
    )
      document.autofill(current.source, current.target, current.version);
  };
  const tick = () => {
    frame.current = undefined;
    const current = gesture.current;
    if (!current || current.x === undefined || current.y === undefined) return;
    update(locate(current.x, current.y, true));
    if (gesture.current) frame.current = requestAnimationFrame(tick);
  };
  return (
    <button
      type="button"
      className="fill-handle"
      style={{
        left: left - 8,
        top: top - 8,
        visibility: visible || preview ? 'visible' : 'hidden',
      }}
      aria-label="Протянуть заполнение диапазона"
      title="Протяните для заполнения. С клавиатуры: стрелки, Enter; Escape — отмена. Числовой ряд продолжается, остальные блоки повторяются целиком."
      onPointerDown={(event) => {
        if (event.button !== 0 || !begin()) return;
        event.preventDefault();
        event.currentTarget.focus();
        gesture.current!.pointer = event.pointerId;
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        const current = gesture.current;
        if (!current || current.pointer !== event.pointerId) return;
        current.x = event.clientX;
        current.y = event.clientY;
        update(locate(event.clientX, event.clientY, false));
        if (gesture.current && frame.current === undefined)
          frame.current = requestAnimationFrame(tick);
      }}
      onPointerUp={(event) => {
        if (gesture.current?.pointer !== event.pointerId) return;
        update(locate(event.clientX, event.clientY, false));
        finish();
      }}
      onPointerCancel={cancel}
      onLostPointerCapture={() => {
        if (gesture.current?.pointer !== undefined) cancel();
      }}
      onBlur={() => {
        if (gesture.current?.pointer === undefined) cancel();
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          cancel();
        } else if (event.key === 'Enter' && gesture.current) {
          event.preventDefault();
          finish();
        } else if (event.key.startsWith('Arrow')) {
          event.preventDefault();
          if (!gesture.current && !begin()) return;
          const target = gesture.current!.target;
          update({
            row: target.row + (event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0),
            column:
              target.column + (event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0),
          });
        }
      }}
    >
      <span aria-hidden="true" />
    </button>
  );
}
