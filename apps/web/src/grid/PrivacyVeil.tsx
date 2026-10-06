import { useState } from 'react';
import type { DocumentStore, Position } from '../document/store';
import { columnLabel } from './viewport';

export function PrivacyVeil({
  document,
  position,
  left,
  top,
  width,
  height,
  contentLeft,
  contentTop,
  contentWidth,
  contentHeight,
}: {
  document: DocumentStore;
  position: Position;
  left: number;
  top: number;
  width: number;
  height: number;
  contentLeft: number;
  contentTop: number;
  contentWidth: number;
  contentHeight: number;
}) {
  const [revealed, setRevealed] = useState(false);
  const address = `${columnLabel(position.column)}${position.row + 1}`;
  const original = revealed ? document.privacyOriginalDisplay(position) : undefined;
  return (
    <button
      type="button"
      className={`privacy-veil${revealed ? ' privacy-veil-revealed' : ''}`}
      style={{ left, top, width, height }}
      aria-label={`${revealed ? 'Скрыть' : 'Раскрыть'} данные ${address}`}
      aria-pressed={revealed}
      onPointerEnter={() => setRevealed(true)}
      onPointerLeave={() => setRevealed(false)}
      onFocus={() => setRevealed(true)}
      onBlur={() => setRevealed(false)}
      onClick={() => {
        document.select(position);
        setRevealed(true);
      }}
      onDoubleClick={() => {
        document.select(position);
        document.begin();
      }}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          setRevealed(false);
        } else if (
          !event.ctrlKey &&
          !event.metaKey &&
          !event.altKey &&
          (event.key === 'F2' || event.key.length === 1)
        ) {
          event.preventDefault();
          event.stopPropagation();
          document.select(position);
          document.begin(event.key === 'F2' ? undefined : event.key);
        }
      }}
    >
      <span
        className="privacy-veil-content"
        style={{ left: contentLeft, top: contentTop, width: contentWidth, height: contentHeight }}
      >
        <span className="privacy-veil-orbit" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        <span className="privacy-veil-label" aria-hidden="true">
          Личные данные
        </span>
        {revealed && <span className="privacy-veil-value">{original ?? 'Ожидаем расчёт'}</span>}
      </span>
    </button>
  );
}
