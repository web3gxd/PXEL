import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import type { DocumentStore } from '../document/store';
import { GRID, columnLabel } from './viewport';

export function AxisSizePanel({
  document,
  onClose,
  axis = 'row',
}: {
  document: DocumentStore;
  onClose: () => void;
  axis?: 'row' | 'column';
}) {
  const heightInput = useRef<HTMLInputElement>(null);
  const widthInput = useRef<HTMLInputElement>(null);
  const sheet = document.sheet;
  const { row, column } = document.selected;
  const rowId = sheet?.rows[row],
    columnId = sheet?.columns[column];
  const [height, setHeight] = useState('');
  const [width, setWidth] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    setHeight(String(document.axisSize('row', row) ?? GRID.rowHeight));
    setWidth(String(document.axisSize('column', column) ?? GRID.columnWidth));
    setError('');
  }, [sheet?.id, rowId, columnId, sheet]);

  const apply = (axis: 'row' | 'column') => {
    const raw = axis === 'row' ? height : width;
    const value = Number(raw);
    const minimum = axis === 'row' ? 12 : 24,
      maximum = axis === 'row' ? 1000 : 2000;
    if (!Number.isInteger(value) || value < minimum || value > maximum) {
      setError(`Введите целое число от ${minimum} до ${maximum} px.`);
      return;
    }
    const ok = document.setAxisSize(
      axis,
      axis === 'row' ? row : column,
      value === (axis === 'row' ? GRID.rowHeight : GRID.columnWidth) ? undefined : value,
    );
    if (ok) onClose();
    else setError(document.error || 'Не удалось изменить размер.');
  };

  return (
    <Modal
      title="Размеры ячеек"
      onClose={onClose}
      initialFocus={axis === 'row' ? heightInput : widthInput}
    >
      <p className="sharing-intro">Точная настройка активной строки и столбца</p>
      <div className="axis-size-fields">
        <label>
          <span>
            Высота строки <strong>{row + 1}</strong>
          </span>
          <div>
            <input
              ref={heightInput}
              aria-label="Высота строки в пикселях"
              type="number"
              min={12}
              max={1000}
              step={1}
              value={height}
              onChange={(event) => setHeight(event.target.value)}
            />
            <small>px</small>
            <button onClick={() => apply('row')}>Применить</button>
          </div>
        </label>
        <label>
          <span>
            Ширина столбца <strong>{columnLabel(column)}</strong>
          </span>
          <div>
            <input
              aria-label="Ширина столбца в пикселях"
              ref={widthInput}
              type="number"
              min={24}
              max={2000}
              step={1}
              value={width}
              onChange={(event) => setWidth(event.target.value)}
            />
            <small>px</small>
            <button onClick={() => apply('column')}>Применить</button>
          </div>
        </label>
      </div>
      {error && (
        <p role="alert" className="axis-size-error">
          {error}
        </p>
      )}
      <footer>
        <span>Строка: 12–1000 px · Столбец: 24–2000 px</span>
        <button className="markdown-button-cancel" onClick={onClose}>
          Готово
        </button>
      </footer>
    </Modal>
  );
}
