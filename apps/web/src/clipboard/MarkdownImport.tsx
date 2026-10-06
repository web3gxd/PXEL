import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowDownRightIcon,
  CheckCircleIcon,
  FileTextIcon,
  TableIcon,
  WarningCircleIcon,
  XIcon,
} from '@phosphor-icons/react';
import { parseCellAddress } from './address';
import { columnLabel } from '../grid/viewport';
import type { MarkdownDocument, MarkdownTable } from './markdown';
import type { DocumentStore, Position } from '../document/store';
import { isDefaultStyle } from '../document/styles';

export type MarkdownImportRequest = {
  text: string;
  markdown: MarkdownDocument;
  origin: Position;
  force: boolean;
};

export function MarkdownImport({
  request,
  document,
  onClose,
  onPreview,
}: {
  request: MarkdownImportRequest;
  document: DocumentStore;
  onClose: () => void;
  onPreview: (rect: { origin: Position; rows: number; columns: number } | undefined) => void;
}) {
  const dialog = useRef<HTMLElement>(null);
  const [index, setIndex] = useState(0);
  const [address, setAddress] = useState(
    () => `${columnLabel(request.origin.column)}${request.origin.row + 1}`,
  );
  const [replace, setReplace] = useState(false);
  const [error, setError] = useState('');
  const tables = request.markdown.tables;
  const table = tables[index];
  const matrix = table?.cells;
  const target = parseCellAddress(address);
  const rows = table?.rows ?? 1;
  const columns = table?.columns ?? 1;

  useEffect(() => {
    dialog.current
      ?.querySelector<HTMLInputElement>('input[aria-label="Начальная ячейка вставки"]')
      ?.focus();
    return () => onPreview(undefined);
  }, [onPreview]);

  const overlap = useMemo(() => {
    if (!table || !target || table.tooLarge) return false;
    for (let row = 0; row < rows; row++) {
      for (let column = 0; column < columns; column++) {
        const position = { row: target.row + row, column: target.column + column };
        const cell = document.value(position);
        if ((cell && cell.type !== 'blank') || !isDefaultStyle(document.style(position)))
          return true;
      }
    }
    return false;
  }, [document, table, target?.row, target?.column, rows, columns]);

  const bounds = useMemo(() => {
    if (!target) return '';
    const end = `${columnLabel(target.column + columns - 1)}${target.row + rows}`;
    return `${columnLabel(target.column)}${target.row + 1}:${end}`;
  }, [target?.row, target?.column, rows, columns]);

  const close = () => {
    onPreview(undefined);
    onClose();
  };

  const wholeText = () => {
    if (request.text.length > 32 * 1024) {
      setError('Текст целиком превышает лимит одной ячейки (32 КиБ).');
      return;
    }
    if (
      !target ||
      target.row >= (document.sheet?.rows.length ?? 0) ||
      target.column >= (document.sheet?.columns.length ?? 0)
    ) {
      setError('Выберите целевую ячейку внутри листа.');
      return;
    }
    const prior = document.value(target);
    if (prior && prior.type !== 'blank' && !replace) {
      setError(
        'Целевая ячейка содержит данные. Подтвердите перезапись или выберите другую ячейку.',
      );
      return;
    }
    if (document.pasteMatrix([[request.text]], target, true)) close();
    else setError(document.error || 'Не удалось вставить Markdown.');
  };

  const importTable = () => {
    if (!table || !matrix || !target) {
      setError(
        table?.tooLarge
          ? 'Таблица превышает лимит 1000 ячеек.'
          : 'Укажите корректную ячейку, например A1.',
      );
      return;
    }
    if (
      target.row + rows > (document.sheet?.rows.length ?? 0) ||
      target.column + columns > (document.sheet?.columns.length ?? 0)
    ) {
      setError('Вся таблица должна помещаться внутри текущего диапазона листа.');
      return;
    }
    if (overlap && !replace) {
      setError(
        'Часть диапазона содержит данные. Подтвердите перезапись или выберите другую ячейку.',
      );
      return;
    }
    if (document.pasteMatrix(matrix, target, true, table.alignment)) close();
    else setError(document.error || 'Не удалось вставить таблицу.');
  };

  const selectTable = (item: MarkdownTable) => {
    setIndex(item.number - 1);
    setError('');
    setReplace(false);
  };

  const canPreview = Boolean(
    table &&
    !table.tooLarge &&
    matrix &&
    target &&
    target.row + rows <= (document.sheet?.rows.length ?? 0) &&
    target.column + columns <= (document.sheet?.columns.length ?? 0),
  );

  useEffect(() => {
    onPreview(canPreview && table && target ? { origin: target, rows, columns } : undefined);
  }, [canPreview, table, rows, columns, target?.row, target?.column, onPreview]);

  return (
    <div
      className="markdown-import-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <section
        ref={dialog}
        className="markdown-import"
        role="dialog"
        aria-modal="true"
        aria-labelledby="markdown-import-title"
        aria-describedby="markdown-import-description"
        data-testid="markdown-import"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            close();
            return;
          }
          if (event.key !== 'Tab') return;
          const items = [
            ...(dialog.current?.querySelectorAll<HTMLElement>(
              'button:not(:disabled), input:not(:disabled)',
            ) ?? []),
          ];
          const first = items[0],
            last = items.at(-1);
          if (event.shiftKey && globalThis.document.activeElement === first) {
            event.preventDefault();
            last?.focus();
          } else if (!event.shiftKey && globalThis.document.activeElement === last) {
            event.preventDefault();
            first?.focus();
          }
        }}
      >
        <header className="markdown-import-heading">
          <div className="markdown-import-mark" aria-hidden="true">
            {tables.length ? (
              <TableIcon size={21} weight="duotone" />
            ) : (
              <FileTextIcon size={21} weight="duotone" />
            )}
          </div>
          <div className="markdown-import-title-wrap">
            <span className="markdown-eyebrow">ИМПОРТ ИЗ ДОКУМЕНТА</span>
            <h2 id="markdown-import-title">Импорт Markdown</h2>
            <p id="markdown-import-description">
              {tables.length
                ? `Найдено таблиц: ${tables.length}. Выберите данные и место вставки.`
                : 'Сохраните исходную разметку в одной ячейке.'}
            </p>
          </div>
          <button className="markdown-close" aria-label="Закрыть предпросмотр" onClick={close}>
            <XIcon size={19} />
          </button>
        </header>

        <div className="markdown-import-body">
          {tables.length > 1 && (
            <section className="markdown-table-picker" aria-label="Найденные таблицы">
              <div className="markdown-section-heading">
                <span>Найденные таблицы</span>
                <small>{tables.length} блока</small>
              </div>
              <div className="markdown-table-list">
                {tables.map((item) => (
                  <button
                    className="markdown-table-option"
                    key={item.number}
                    aria-pressed={index === item.number - 1}
                    onClick={() => selectTable(item)}
                  >
                    <span className="markdown-table-option-top">
                      <strong>Таблица {item.number}</strong>
                      <small>
                        {item.rows} × {item.columns}
                      </small>
                    </span>
                    <span className="markdown-table-option-heading">
                      {item.heading || 'Без заголовка'}
                    </span>
                    <span className="markdown-table-option-context">
                      {item.context.slice(0, 160)}
                    </span>
                  </button>
                ))}
              </div>
            </section>
          )}

          {table && (
            <section
              className="markdown-preview"
              aria-label={`Предпросмотр таблицы ${table.number}`}
            >
              <div className="markdown-preview-heading">
                <div>
                  <span className="markdown-section-heading">Предпросмотр</span>
                  <strong>{table.heading || `Таблица ${table.number}`}</strong>
                </div>
                <span className="markdown-dimensions">
                  {table.rows} строк <i>×</i> {table.columns} столбцов
                </span>
              </div>
              {table.tooLarge && (
                <p className="markdown-alert" role="alert">
                  <WarningCircleIcon size={17} />
                  Таблица превышает лимит вставки (1000 ячеек). Показаны первые строки.
                </p>
              )}
              <div className="markdown-table-scroll">
                <table>
                  <tbody>
                    {(matrix ?? table.sample).slice(0, 4).map((row, rowIndex) => (
                      <tr key={rowIndex}>
                        {row.slice(0, 6).map((value, columnIndex) =>
                          rowIndex === 0 ? (
                            <th key={columnIndex} scope="col">
                              {value}
                            </th>
                          ) : (
                            <td key={columnIndex}>
                              {value || <span className="markdown-empty-cell">пусто</span>}
                            </td>
                          ),
                        )}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          )}

          <section className="markdown-placement" aria-label="Место вставки">
            <div className="markdown-placement-copy">
              <span className="markdown-section-heading">МЕСТО ВСТАВКИ</span>
              <p>Верхняя левая ячейка выбранной таблицы</p>
            </div>
            <label className="markdown-target">
              <span className="sr-only">Начальная ячейка</span>
              <input
                aria-label="Начальная ячейка вставки"
                value={address}
                onChange={(event) => {
                  setAddress(event.target.value.toUpperCase());
                  setError('');
                }}
              />
            </label>
            <div className="markdown-range-card">
              <ArrowDownRightIcon size={16} aria-hidden="true" />
              <span>
                <small>ДИАПАЗОН</small>
                <strong data-testid="markdown-target-range">
                  Диапазон: {bounds || 'укажите ячейку'}
                </strong>
              </span>
              {canPreview && (
                <CheckCircleIcon
                  className="markdown-range-check"
                  size={17}
                  aria-label="Диапазон помещается на листе"
                />
              )}
            </div>
          </section>

          {table?.alignment.some(Boolean) && (
            <p className="markdown-warning">
              <WarningCircleIcon size={16} /> GFM задаёт выравнивание, но его атомарный импорт пока
              недоступен.
            </p>
          )}
          {overlap && (
            <label className="markdown-overwrite">
              <input
                aria-label="Разрешить перезапись непустых ячеек"
                type="checkbox"
                checked={replace}
                onChange={(event) => setReplace(event.target.checked)}
              />
              <span>
                <strong>Разрешить перезапись непустых ячеек</strong>
                <small>В целевом диапазоне есть заполненные или оформленные ячейки.</small>
              </span>
            </label>
          )}
          {error && (
            <p className="markdown-alert" role="alert">
              <WarningCircleIcon size={17} />
              {error}
            </p>
          )}
        </div>

        <footer className="markdown-actions">
          <div className="markdown-actions-hint">
            {request.force
              ? 'Файл будет вставлен в активную ячейку.'
              : 'Предпросмотр диапазона показан на листе.'}
          </div>
          <div className="markdown-actions-buttons">
            {tables.length > 0 && (
              <button
                className="markdown-button-primary"
                disabled={!canPreview || (overlap && !replace)}
                onClick={importTable}
              >
                Вставить таблицу
              </button>
            )}
            <button
              className="markdown-button-secondary"
              disabled={
                document.pending.length >= 128 || (overlap && !replace && tables.length === 0)
              }
              onClick={wholeText}
            >
              Вставить весь текст
            </button>
            <button className="markdown-button-cancel" onClick={close}>
              Отмена
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}
