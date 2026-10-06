import { Select, SelectOption } from '../ui/Select';
import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { boundedJson } from '../sharing/api';
import { writeTextClipboard } from '../clipboard/outbound';
import { columnLabel } from '../grid/viewport';
import {
  decodeActivity,
  decodeDiff,
  decodeHistorical,
  describeHistorical,
  diffCategories,
  type ActivityPage,
  type Difference,
  type DiffReport,
  type Historical,
  type Footprint,
} from './version-history';
import type { DocumentStore, Projection, Sheet } from './store';
const categories: Record<string, string> = {
  value: 'Значения',
  formula: 'Формулы',
  calculated: 'Результаты',
  style: 'Оформление',
  size: 'Размеры',
  merge: 'Объединения',
  sheet: 'Листы',
  structure: 'Структура',
  layout: 'Закрепления',
};
const operations: Record<string, string> = {
  create_workbook: 'Создание книги',
  add_sheet: 'Добавление листа',
  frozen_rows: 'Закрепление строк',
  axis_size: 'Размер осей',
  cell_write: 'Изменение ячейки',
  undo_redo: 'Undo / Redo',
  range_write: 'Изменение диапазона',
  copy_range: 'Копирование',
  move_range: 'Перемещение',
  style_write: 'Оформление',
  sort_range: 'Сортировка',
  cell_batch: 'Изменение ячеек',
  merge_write: 'Объединение',
  rows: 'Изменение строк',
  columns: 'Изменение столбцов',
  axis_edit: 'Структурная операция',
  calculation_epoch: 'Контекст расчёта',
};
const geometry = new WeakMap<Sheet, { rows: Map<string, number>; columns: Map<string, number> }>();
function positions(sheet: Sheet) {
  let value = geometry.get(sheet);
  if (!value) {
    value = {
      rows: new Map(sheet.rows.map((id, i) => [id, i])),
      columns: new Map(sheet.columns.map((id, i) => [id, i])),
    };
    geometry.set(sheet, value);
  }
  return value;
}
function cellLabel(sheet: Sheet, row: string, column: string) {
  const { rows, columns } = positions(sheet),
    r = rows.get(row),
    c = columns.get(column);
  return r === undefined || c === undefined ? 'Удалённая ячейка' : `${columnLabel(c)}${r + 1}`;
}
function footprint(f: Footprint, projection: Projection) {
  const sheet = projection.sheets.find((s) => s.id === f.sheet_id);
  if (!sheet) return 'Удалённый лист';
  const { rows: rowPositions, columns: columnPositions } = positions(sheet);
  let minRow = Infinity,
    maxRow = -1,
    minColumn = Infinity,
    maxColumn = -1,
    deleted = 0;
  for (const [r, c] of f.addresses) {
    const row = rowPositions.get(r),
      column = columnPositions.get(c);
    if (row === undefined || column === undefined) {
      deleted++;
      continue;
    }
    minRow = Math.min(minRow, row);
    maxRow = Math.max(maxRow, row);
    minColumn = Math.min(minColumn, column);
    maxColumn = Math.max(maxColumn, column);
  }
  const range =
    maxRow < 0
      ? ''
      : `${columnLabel(minColumn)}${minRow + 1}:${columnLabel(maxColumn)}${maxRow + 1} (границы, ячеек ${f.addresses.length - deleted})`;
  const rows = f.row_ids.map((r) => rowPositions.get(r) ?? -1),
    cols = f.column_ids.map((c) => columnPositions.get(c) ?? -1);
  const labels = [
    range,
    deleted ? `Удалённых ячеек: ${deleted}` : '',
    rows.length
      ? `Строки: ${rows
          .map((r) => (r < 0 ? 'удалена' : r + 1))
          .slice(0, 8)
          .join(', ')}${rows.length > 8 ? '…' : ''}`
      : '',
    cols.length
      ? `Столбцы: ${cols
          .map((c) => (c < 0 ? 'удалён' : columnLabel(c)))
          .slice(0, 8)
          .join(', ')}${cols.length > 8 ? '…' : ''}`
      : '',
  ].filter(Boolean);
  return `${sheet.name}${labels.length ? '!' + labels.join('; ') : ''}`;
}
function location(change: Difference, projection: Projection) {
  const sheet = projection.sheets.find((s) => s.id === change.sheet_id);
  if (!sheet) return 'Удалённый лист';
  const ids = change.entity_id.split('/');
  if (ids.length === 2) return `${sheet.name}!${cellLabel(sheet, ids[0]!, ids[1]!)}`;
  if (ids.length === 4)
    return `${sheet.name}!${cellLabel(sheet, ids[0]!, ids[2]!)}:${cellLabel(sheet, ids[1]!, ids[3]!)}`;
  if (ids[0]) {
    const row = sheet.rows.indexOf(ids[0]),
      col = sheet.columns.indexOf(ids[0]);
    return `${sheet.name}: ${row >= 0 ? 'строка ' + (row + 1) : col >= 0 ? 'столбец ' + columnLabel(col) : 'удалённая ось'}`;
  }
  return sheet.name;
}
export function HistoryPanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const [captured] = useState(() => ({
    id: document.projection!.document_id,
    version: document.projection!.version,
    projection: structuredClone(document.projection!),
  }));
  const [feed, setFeed] = useState<ActivityPage>();
  const [feedBefore, setFeedBefore] = useState<string>();
  const [feedBack, setFeedBack] = useState<(string | undefined)[]>([]);
  const [historical, setHistorical] = useState<Historical>();
  const [report, setReport] = useState<DiffReport>();
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('');
  const [from, setFrom] = useState('1'),
    [to, setTo] = useState(captured.version);
  const [filter, setFilter] = useState('all'),
    [diffPage, setDiffPage] = useState(0),
    [sheetIndex, setSheetIndex] = useState(0),
    [rowPage, setRowPage] = useState(0),
    [columnPage, setColumnPage] = useState(0);
  const request = useRef<AbortController>(null);
  async function read<T>(path: string, decode: (raw: unknown) => T): Promise<T> {
    const current = request.current!;
    const response = await fetch(`/api/documents/${captured.id}/${path}`, {
      signal: current.signal,
    });
    if (!response.ok)
      throw new Error(
        response.status === 403
          ? 'История доступна владельцу.'
          : response.status === 404
            ? 'Версия не найдена.'
            : 'История недоступна или превышен бюджет анализа.',
      );
    const body = await boundedJson(response, 16 * 1024 * 1024);
    if (current.signal.aborted || request.current !== current) throw new Error('Запрос заменён.');
    return decode(body);
  }
  async function run(action: () => Promise<void>) {
    request.current?.abort();
    request.current = new AbortController();
    const current = request.current;
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (e) {
      if (!current.signal.aborted) setError(e instanceof Error ? e.message : 'История недоступна.');
    } finally {
      if (!current.signal.aborted) setBusy(false);
    }
  }
  useEffect(() => {
    void run(async () => {
      setFeed(
        await read(`history?schema_version=1&through_version=${captured.version}`, decodeActivity),
      );
    });
    return () => request.current?.abort();
  }, [captured]);
  const preview = async (target: string) => {
    const value = await read(`history/${encodeURIComponent(target)}`, decodeHistorical);
    if (value.projection.document_id !== captured.id || value.projection.version !== target)
      throw new Error('Ответ относится к другой версии.');
    setHistorical(value);
    setSheetIndex(0);
    setRowPage(0);
    setColumnPage(0);
  };
  const sheet = historical?.projection.sheets[sheetIndex];
  const values = new Map(
    historical?.calculated.map((c) => [`${c.sheet_id}/${c.row_id}/${c.column_id}`, c.value]),
  );
  const content = new Map(sheet?.cells.map((c) => [`${c.row_id}/${c.column_id}`, c.value]));
  const styles = new Map(sheet?.styles?.map((c) => [`${c.row_id}/${c.column_id}`, c.style]));
  const visibleMerges = new Set<string>();
  if (sheet) {
    const { rows, columns } = positions(sheet);
    for (const merge of sheet.merges ?? []) {
      const first = Math.max(rows.get(merge.first_row_id) ?? 0, rowPage * 30);
      const last = Math.min(rows.get(merge.last_row_id) ?? -1, rowPage * 30 + 29);
      const left = Math.max(columns.get(merge.first_column_id) ?? 0, columnPage * 12);
      const right = Math.min(columns.get(merge.last_column_id) ?? -1, columnPage * 12 + 11);
      for (let r = first; r <= last; r++)
        for (let c = left; c <= right; c++)
          visibleMerges.add(`${sheet.rows[r]}/${sheet.columns[c]}`);
    }
  }
  const filtered = report?.changes.filter((c) => filter === 'all' || c.category === filter) ?? [];
  const changed = new Set(
    report?.changes
      .filter((c) => ['value', 'formula', 'calculated', 'style'].includes(c.category))
      .map((c) => `${c.sheet_id}/${c.entity_id}`),
  );
  return (
    <Modal title="Журнал и версии" onClose={onClose} clipboardError={document.error}>
      <div
        className="version-history"
        data-disclosure-clipboard="owned"
        onCopyCapture={(e) => {
          e.preventDefault();
          const text = window.getSelection()?.toString() ?? '';
          if (text) void writeTextClipboard(document, text, 'copy', undefined, true);
        }}
        onCutCapture={(e) => e.preventDefault()}
      >
        <p>
          История зафиксирована до версии {captured.version}. Просмотр и сравнение используют
          отдельные проекции; текущая книга и Undo / Redo сохраняются.
        </p>
        {document.projection?.version !== captured.version && (
          <p role="status">Открытая книга изменилась. Здесь сохранён выбранный срез истории.</p>
        )}
        {busy && <p role="status">Загрузка истории…</p>}
        {error && <p role="alert">{error}</p>}
        <fieldset disabled={busy}>
          <legend>Выбор версий</legend>
          <label>
            До
            <input
              aria-label="Версия до"
              type="number"
              min="1"
              max={captured.version}
              value={from}
              onChange={(e) => setFrom(e.target.value)}
            />
          </label>
          <label>
            После
            <input
              aria-label="Версия после"
              type="number"
              min="1"
              max={captured.version}
              value={to}
              onChange={(e) => setTo(e.target.value)}
            />
          </label>
          <button type="button" onClick={() => void run(() => preview(to))}>
            Открыть версию
          </button>
          <button
            type="button"
            onClick={() =>
              void run(async () => {
                const value = await read(
                  `diff?${new URLSearchParams({ schema_version: '1', from_version: from, to_version: to })}`,
                  decodeDiff,
                );
                if (
                  value.document_id !== captured.id ||
                  value.from_version !== from ||
                  value.to_version !== to
                )
                  throw new Error('Ответ относится к другому сравнению.');
                setReport(value);
                setFilter('all');
                setDiffPage(0);
                await preview(to);
              })
            }
          >
            Сравнить версии
          </button>
        </fieldset>
        {historical && sheet && (
          <section aria-label="Историческая версия" className="historical-view">
            <h3>Версия {historical.projection.version} · Только чтение</h3>
            <label>
              Исторический лист
              <Select
                aria-label="Исторический лист"
                value={sheetIndex}
                onValueChange={(e) => {
                  setSheetIndex(Number(e));
                  setRowPage(0);
                  setColumnPage(0);
                }}
              >
                {historical.projection.sheets.map((s, i) => (
                  <SelectOption key={s.id} value={i}>
                    {s.name}
                  </SelectOption>
                ))}
              </Select>
            </label>
            <p>
              Закреплено строк: {sheet.frozen_rows ?? 0}. Объединений: {sheet.merges?.length ?? 0}.
              Формулы и результаты показаны отдельно.
            </p>
            <div
              className="historical-table-scroll"
              tabIndex={0}
              aria-label="Таблица выбранной версии"
            >
              <table className="historical-table">
                <caption>
                  {sheet.name} · строки {rowPage * 30 + 1}–
                  {Math.min((rowPage + 1) * 30, sheet.rows.length)}
                </caption>
                <thead>
                  <tr>
                    <th scope="col">Строка</th>
                    {sheet.columns.slice(columnPage * 12, columnPage * 12 + 12).map((c, i) => (
                      <th scope="col" key={c}>
                        {columnLabel(columnPage * 12 + i)}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {sheet.rows.slice(rowPage * 30, rowPage * 30 + 30).map((r, i) => (
                    <tr
                      key={r}
                      style={{
                        height: sheet.row_heights?.find((h) => h.axis_id === r)?.size_css_px,
                      }}
                    >
                      <th scope="row">{rowPage * 30 + i + 1}</th>
                      {sheet.columns.slice(columnPage * 12, columnPage * 12 + 12).map((c) => {
                        const key = `${r}/${c}`,
                          v = content.get(key),
                          style = styles.get(key),
                          result = values.get(`${sheet.id}/${key}`);
                        return (
                          <td
                            key={c}
                            className={changed.has(`${sheet.id}/${key}`) ? 'diff-cell' : ''}
                            style={{
                              minWidth: sheet.column_widths?.find((w) => w.axis_id === c)
                                ?.size_css_px,
                              fontWeight: style?.font_bold ? 'bold' : undefined,
                              fontStyle: style?.font_italic ? 'italic' : undefined,
                              background: style ? `rgb(${style.fill.join(',')})` : undefined,
                              color: style ? `rgb(${style.font_color.join(',')})` : undefined,
                            }}
                          >
                            {v?.type === 'formula' ? (
                              <>
                                <code>{v.value.current_text ?? v.value.source}</code>
                                <span className="historical-result">
                                  Результат: {describeHistorical(result)}
                                </span>
                              </>
                            ) : (
                              describeHistorical(v ?? { type: 'blank' })
                            )}
                            {visibleMerges.has(key) && (
                              <span className="historical-result">▣ Объединение</span>
                            )}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="sharing-header-actions">
              <button
                type="button"
                disabled={rowPage === 0}
                onClick={() => setRowPage((p) => p - 1)}
              >
                Предыдущие строки
              </button>
              <button
                type="button"
                disabled={(rowPage + 1) * 30 >= sheet.rows.length}
                onClick={() => setRowPage((p) => p + 1)}
              >
                Следующие строки
              </button>
              <button
                type="button"
                disabled={columnPage === 0}
                onClick={() => setColumnPage((p) => p - 1)}
              >
                Предыдущие столбцы
              </button>
              <button
                type="button"
                disabled={(columnPage + 1) * 12 >= sheet.columns.length}
                onClick={() => setColumnPage((p) => p + 1)}
              >
                Следующие столбцы
              </button>
            </div>
          </section>
        )}
        {report && (
          <section aria-label="Сравнение версий">
            <h3>
              Изменения {report.from_version} → {report.to_version}: {report.changes.length}
            </h3>
            {report.structural_context && (
              <p>
                Между версиями менялась структура. Ссылки формул сравниваются в координатах каждой
                версии.
              </p>
            )}
            <label>
              Тип изменений
              <Select
                aria-label="Тип изменений"
                value={filter}
                onValueChange={(e) => {
                  setFilter(e);
                  setDiffPage(0);
                }}
              >
                <SelectOption value="all">Все изменения</SelectOption>
                {diffCategories.map((c) => (
                  <SelectOption key={c} value={c}>
                    {categories[c]}
                  </SelectOption>
                ))}
              </Select>
            </label>
            <p role="status">
              Показано {Math.min(diffPage * 100 + 100, filtered.length) - diffPage * 100} из{' '}
              {filtered.length} изменений выбранного типа.
            </p>
            <ol className="version-diff-list">
              {filtered.slice(diffPage * 100, diffPage * 100 + 100).map((c) => (
                <li key={`${c.sheet_id}/${c.category}/${c.entity_id}/${c.property}`}>
                  <strong>
                    {c.before === null
                      ? '+ Добавлено'
                      : c.after === null
                        ? '− Удалено'
                        : '↔ Изменено'}{' '}
                    · {categories[c.category]} ·{' '}
                    {location(c, historical?.projection ?? captured.projection)}
                  </strong>
                  <span>{c.property}</span>
                  {c.structural_context && (
                    <p>Ссылка изменилась в контексте структурной операции.</p>
                  )}
                  <div className="diff-pair">
                    <div className="diff-before">
                      <b>До</b>
                      <pre>{describeHistorical(c.before)}</pre>
                    </div>
                    <div className="diff-after">
                      <b>После</b>
                      <pre>{describeHistorical(c.after)}</pre>
                    </div>
                  </div>
                </li>
              ))}
            </ol>
            <button
              type="button"
              disabled={diffPage === 0}
              onClick={() => setDiffPage((p) => p - 1)}
            >
              Предыдущие изменения
            </button>
            <button
              type="button"
              disabled={(diffPage + 1) * 100 >= filtered.length}
              onClick={() => setDiffPage((p) => p + 1)}
            >
              Следующие изменения
            </button>
          </section>
        )}
        <section aria-label="Лента действий">
          <h3>Действия</h3>
          <p>Диапазоны указаны в координатах открытой книги; удалённые оси обозначены отдельно.</p>
          <ol className="activity-list">
            {feed?.events.map((e) => (
              <li key={e.operation_id}>
                <div>
                  <strong>
                    Версия {e.version} · {operations[e.operation_type]}
                  </strong>
                  <time dateTime={new Date(e.created_at).toISOString()}>
                    {new Date(e.created_at).toLocaleString('ru-RU')}
                  </time>
                  <span>Автор: {e.actor_id ?? 'Система'}</span>
                  {e.footprints.map((f) => (
                    <span key={f.sheet_id}>{footprint(f, captured.projection)}</span>
                  ))}
                </div>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void run(() => preview(e.version))}
                >
                  Просмотр версии {e.version}
                </button>
              </li>
            ))}
          </ol>
          {!!feedBack.length && (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const target = feedBack.at(-1);
                  const previous = await read(
                    `history?schema_version=1&through_version=${captured.version}${target ? '&before_version=' + target : ''}`,
                    decodeActivity,
                  );
                  setFeed(previous);
                  setFeedBefore(target);
                  setFeedBack(feedBack.slice(0, -1));
                })
              }
            >
              Более поздние действия
            </button>
          )}
          {feed?.next_before && (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const next = await read(
                    `history?schema_version=1&through_version=${captured.version}&before_version=${feed.next_before}`,
                    decodeActivity,
                  );
                  if (next.through_version !== captured.version)
                    throw new Error('Ответ относится к другой истории.');
                  setFeedBack([...feedBack, feedBefore]);
                  setFeedBefore(feed.next_before ?? undefined);
                  setFeed(next);
                })
              }
            >
              Более ранние действия
            </button>
          )}
        </section>
      </div>
    </Modal>
  );
}
