import { Select, SelectOption } from '../ui/Select';
import { useEffect, useMemo, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { columnLabel } from '../grid/viewport';
import type { DocumentStore } from './store';
import { suggestSortBounds, validSortBounds, type SortBounds, type SortKey } from './sort-range';

const label = (bounds: SortBounds) =>
  `${columnLabel(bounds.startColumn)}${bounds.startRow + 1}:${columnLabel(bounds.endColumn)}${bounds.endRow + 1}`;

export function SortPanel({
  document,
  onClose,
  onPreview,
  onSort,
}: {
  document: DocumentStore;
  onClose: () => void;
  onPreview: (bounds?: SortBounds) => void;
  onSort: (
    bounds: SortBounds,
    headerRows: number,
    keys: SortKey[],
    expectedVersion: string,
  ) => boolean;
}) {
  const sheet = document.sheet!;
  const rowPositions = useMemo(() => new Map(sheet.rows.map((id, index) => [id, index])), [sheet]);
  const columnPositions = useMemo(
    () => new Map(sheet.columns.map((id, index) => [id, index])),
    [sheet],
  );
  const [version] = useState(document.projection!.version);
  const [bounds, setBounds] = useState<SortBounds>({ ...document.selectionBounds });
  const suggestion = useMemo(
    () =>
      suggestSortBounds(
        { ...sheet, cells: document.confirmedCells(sheet) },
        document.selectionBounds,
      ),
    [sheet, document, document.projection?.version],
  );
  const existingHeader = document.personalView.settings.filters.find(
    (f) => f.sheet_id === sheet.id,
  )?.header_row_id;
  const [header, setHeader] = useState(existingHeader !== undefined && existingHeader !== null);
  const [headerRow, setHeaderRow] = useState(
    existingHeader ? sheet.rows.indexOf(existingHeader) + 1 : bounds.startRow + 1,
  );
  const [keys, setKeys] = useState<SortKey[]>([{ column: bounds.startColumn, descending: false }]);
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => () => onPreview(undefined), [onPreview]);
  const valid = validSortBounds(sheet, bounds);
  const stale = document.projection?.version !== version;
  const headerRows = header ? headerRow - bounds.startRow : 0;
  const merged = (sheet.merges ?? []).some(
    (merge) =>
      (rowPositions.get(merge.first_row_id) ?? -1) <= bounds.endRow &&
      (rowPositions.get(merge.last_row_id) ?? -1) >= bounds.startRow + headerRows &&
      (columnPositions.get(merge.first_column_id) ?? -1) <= bounds.endColumn &&
      (columnPositions.get(merge.last_column_id) ?? -1) >= bounds.startColumn,
  );
  const ready =
    valid &&
    !merged &&
    confirmed &&
    !stale &&
    !document.pending.length &&
    !document.editing &&
    keys.length > 0 &&
    keys.length <= 50 &&
    new Set(keys.map((k) => k.column)).size === keys.length &&
    keys.every((key) => key.column >= bounds.startColumn && key.column <= bounds.endColumn) &&
    Number.isInteger(headerRows) &&
    headerRows >= 0 &&
    (!header || headerRows > 0) &&
    headerRows <= bounds.endRow - bounds.startRow;
  const change = (next: SortBounds) => {
    setBounds(next);
    setConfirmed(false);
    onPreview(validSortBounds(sheet, next) ? next : undefined);
    setKeys((previous) =>
      previous.filter((key) => key.column >= next.startColumn && key.column <= next.endColumn),
    );
  };
  const columns = sheet.columns
    .map((id, index) => ({ id, index }))
    .filter(({ index }) => index >= bounds.startColumn && index <= bounds.endColumn);
  return (
    <Modal title="Сортировка диапазона" onClose={onClose} clipboardError={document.error}>
      <p className="sharing-help">
        Строки перемещаются вместе с формулами и оформлением. Относительные ссылки пересчитываются;
        абсолютные сохраняются. Скрытые фильтром строки участвуют. Пустые значения и ошибки остаются
        в конце.
      </p>
      {suggestion && (
        <div className="sharing-notice">
          <p>Выделен один столбец связанной таблицы. Предлагаемый диапазон: {label(suggestion)}.</p>
          <button type="button" onClick={() => change(suggestion)}>
            Использовать предложенный диапазон
          </button>
        </div>
      )}
      <fieldset className="sharing-fieldset view-fields">
        <legend>Границы диапазона</legend>
        <div className="view-range">
          <label>
            Первая строка{' '}
            <input
              type="number"
              min={1}
              max={sheet.rows.length}
              value={bounds.startRow + 1}
              onChange={(e) => change({ ...bounds, startRow: Number(e.target.value) - 1 })}
            />
          </label>
          <label>
            Последняя строка{' '}
            <input
              type="number"
              min={1}
              max={sheet.rows.length}
              value={bounds.endRow + 1}
              onChange={(e) => change({ ...bounds, endRow: Number(e.target.value) - 1 })}
            />
          </label>
          <label>
            Первый столбец{' '}
            <Select
              aria-label="Первый столбец"
              value={bounds.startColumn}
              onValueChange={(e) => change({ ...bounds, startColumn: Number(e) })}
            >
              {sheet.columns.map((id, index) => (
                <SelectOption key={id} value={index}>
                  {columnLabel(index)}
                </SelectOption>
              ))}
            </Select>
          </label>
          <label>
            Последний столбец{' '}
            <Select
              aria-label="Последний столбец"
              value={bounds.endColumn}
              onValueChange={(e) => change({ ...bounds, endColumn: Number(e) })}
            >
              {sheet.columns.map((id, index) => (
                <SelectOption key={id} value={index}>
                  {columnLabel(index)}
                </SelectOption>
              ))}
            </Select>
          </label>
        </div>
        <label>
          <input type="checkbox" checked={header} onChange={(e) => setHeader(e.target.checked)} />
          Есть заголовок
        </label>
        {header && (
          <label>
            Строка заголовка{' '}
            <input
              type="number"
              min={bounds.startRow + 1}
              max={bounds.endRow}
              value={headerRow}
              onChange={(e) => setHeaderRow(Number(e.target.value))}
            />
          </label>
        )}
        {header && <p className="sharing-help">Заголовок и строки перед ним остаются на месте.</p>}
        <label>
          <input
            type="checkbox"
            checked={confirmed}
            onChange={(e) => setConfirmed(e.target.checked)}
          />
          Подтверждаю границы {label(bounds)}
        </label>
      </fieldset>
      <fieldset className="sharing-fieldset view-fields">
        <legend>Ключи по порядку приоритета</legend>
        {keys.map((key, index) => (
          <div className="view-range" key={index}>
            <label>
              Столбец {index + 1}
              <Select
                aria-label={`Столбец сортировки ${index + 1}`}
                value={key.column}
                onValueChange={(e) =>
                  setKeys((previous) =>
                    previous.map((k, i) => (i === index ? { ...k, column: Number(e) } : k)),
                  )
                }
              >
                {columns.map(({ id, index }) => (
                  <SelectOption key={id} value={index}>
                    {columnLabel(index)}
                  </SelectOption>
                ))}
              </Select>
            </label>
            <label>
              Направление {index + 1}
              <Select
                aria-label={`Направление сортировки ${index + 1}`}
                value={key.descending ? 'desc' : 'asc'}
                onValueChange={(e) =>
                  setKeys((previous) =>
                    previous.map((k, i) => (i === index ? { ...k, descending: e === 'desc' } : k)),
                  )
                }
              >
                <SelectOption value="asc">По возрастанию</SelectOption>
                <SelectOption value="desc">По убыванию</SelectOption>
              </Select>
            </label>
            <button
              type="button"
              aria-label={`Удалить ключ ${index + 1}`}
              onClick={() => setKeys((previous) => previous.filter((_, i) => i !== index))}
            >
              Удалить
            </button>
          </div>
        ))}
        <button
          type="button"
          disabled={keys.length >= Math.min(50, columns.length)}
          onClick={() => {
            const column = columns.find((c) => !keys.some((k) => k.column === c.index));
            if (column)
              setKeys((previous) => [...previous, { column: column.index, descending: false }]);
          }}
        >
          Добавить ключ
        </button>
      </fieldset>
      <p className="sharing-help">
        Предел — 100 000 выбранных ячеек и 4 МиБ события. Диапазон тела с объединениями отклоняется
        целиком. Равные ключи сохраняют исходный порядок.
      </p>
      {stale && <p role="alert">Книга изменилась. Закройте панель и заново проверьте границы.</p>}
      {merged && (
        <p role="alert">
          Тело диапазона пересекает объединение. Измените границы или снимите объединение.
        </p>
      )}
      {error && <p role="alert">{error}</p>}
      <button
        type="button"
        className="sharing-primary"
        disabled={!ready}
        onClick={() => {
          if (onSort(bounds, headerRows, keys, version)) onClose();
          else
            setError(
              'Дождитесь сохранения и подтверждённого расчёта, затем проверьте диапазон и права.',
            );
        }}
      >
        Сортировать
      </button>
    </Modal>
  );
}
