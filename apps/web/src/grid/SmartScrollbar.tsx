import { useEffect, useRef, useState, type PointerEvent, type KeyboardEvent } from 'react';
import { type AxisLayout, AxisScrollMapping, columnLabel } from './viewport';

type Props = {
  axis: 'row' | 'column';
  metrics: AxisLayout;
  mapping: AxisScrollMapping;
  offset: number;
  viewportSize: number;
  onOffset: (offset: number) => void;
};

const MIN_THUMB = 32;

export function SmartScrollbar({ axis, metrics, mapping, offset, viewportSize, onOffset }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; grabOffset: number } | undefined>(undefined);
  const [trackLength, setTrackLength] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [tooltip, setTooltip] = useState<{ left: number; top: number; text: string }>();
  const vertical = axis === 'row';
  const thumbLength =
    trackLength > 0 && metrics.totalSize > viewportSize
      ? Math.min(trackLength, Math.max(MIN_THUMB, (trackLength * viewportSize) / metrics.totalSize))
      : trackLength;
  const thumbTravel = Math.max(0, trackLength - thumbLength);
  const thumbPosition = thumbTravel * mapping.positionAt(offset);
  const index = metrics.indexAt(Math.max(0, offset));
  const label = vertical ? `Строка ${index + 1}` : `Колонка ${columnLabel(index)}`;

  useEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    const observer = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) setTrackLength(vertical ? box.height : box.width);
    });
    observer.observe(track);
    return () => observer.disconnect();
  }, [vertical]);

  const positionAtPointer = (event: PointerEvent<HTMLDivElement>, grabOffset: number) => {
    const track = trackRef.current;
    if (!track || thumbTravel === 0) return;
    const box = track.getBoundingClientRect();
    const coordinate = vertical ? event.clientY - box.top : event.clientX - box.left;
    const normalized = Math.max(0, Math.min(1, (coordinate - grabOffset) / thumbTravel));
    const nextOffset = mapping.offsetAt(normalized);
    onOffset(nextOffset);
    const targetIndex = metrics.indexAt(nextOffset + viewportSize);
    setTooltip({
      left: vertical
        ? Math.max(8, box.left - 112)
        : Math.max(8, Math.min(window.innerWidth - 150, event.clientX - 42)),
      top: vertical
        ? Math.max(8, Math.min(window.innerHeight - 40, event.clientY - 15))
        : Math.max(8, box.top - 34),
      text: vertical ? `Строка ${targetIndex + 1}` : `Колонка ${columnLabel(targetIndex)}`,
    });
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (thumbTravel <= 0 || metrics.count === 0) return;
    event.preventDefault();
    event.stopPropagation();
    const box = event.currentTarget.getBoundingClientRect();
    const coordinate = vertical ? event.clientY - box.top : event.clientX - box.left;
    const grabOffset =
      event.target === event.currentTarget
        ? thumbLength / 2
        : Math.max(0, Math.min(thumbLength, coordinate - thumbPosition));
    dragRef.current = { pointerId: event.pointerId, grabOffset };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
    positionAtPointer(event, grabOffset);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    positionAtPointer(event, drag.grabOffset);
  };

  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = undefined;
    setDragging(false);
    setTooltip(undefined);
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const current = Math.max(0, Math.min(mapping.totalOffset, offset));
    const step = metrics.sizeAt(metrics.indexAt(current)) || (vertical ? 28 : 112);
    let next: number | undefined;
    switch (event.key) {
      case 'ArrowUp':
      case 'ArrowLeft':
        next = current - step;
        break;
      case 'ArrowDown':
      case 'ArrowRight':
        next = current + step;
        break;
      case 'PageUp':
        next = current - viewportSize;
        break;
      case 'PageDown':
        next = current + viewportSize;
        break;
      case 'Home':
        next = 0;
        break;
      case 'End':
        next = mapping.totalOffset;
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    onOffset(Math.max(0, Math.min(mapping.totalOffset, next)));
  };

  if (metrics.totalSize <= viewportSize) return null;
  const ariaIndex = metrics.indexAt(offset) + 1;
  return (
    <>
      <div
        ref={trackRef}
        className={`smart-scrollbar smart-scrollbar-${axis}${dragging ? ' is-dragging' : ''}`}
        role="scrollbar"
        aria-label={vertical ? 'Прокрутка строк' : 'Прокрутка столбцов'}
        aria-orientation={vertical ? 'vertical' : 'horizontal'}
        aria-valuemin={1}
        aria-valuemax={metrics.count}
        aria-valuenow={Math.min(metrics.count, ariaIndex)}
        aria-valuetext={label}
        data-scroll-mode={mapping.isAdaptive ? 'adaptive' : 'linear'}
        data-used-scroll-offset={mapping.usedOffset}
        data-scroll-limit={mapping.totalOffset}
        data-testid={`smart-scrollbar-${axis}`}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={onKeyDown}
      >
        <span
          className="smart-scrollbar-thumb"
          aria-hidden="true"
          data-testid={`smart-scrollbar-thumb-${axis}`}
          style={
            vertical
              ? { top: thumbPosition, height: thumbLength }
              : { left: thumbPosition, width: thumbLength }
          }
        />
      </div>
      {dragging && tooltip && (
        <output
          className="smart-scrollbar-tooltip"
          style={{ left: tooltip.left, top: tooltip.top }}
        >
          {tooltip.text}
        </output>
      )}
    </>
  );
}
