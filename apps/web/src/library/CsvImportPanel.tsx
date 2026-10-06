import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { Select, SelectOption } from '../ui/Select';
import {
  previewCsv,
  publishCsv,
  readCsvFile,
  type ColumnType,
  type CsvPayload,
  type CsvPreview,
} from './csv-import';

export function CsvImportPanel({
  onClose,
  onImported,
}: {
  onClose: () => void;
  onImported: (id: string) => void;
}) {
  const [payload, setPayload] = useState<CsvPayload>();
  const [preview, setPreview] = useState<CsvPreview>();
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState('');
  const [page, setPage] = useState(0);
  const [attempted, setAttempted] = useState(false);
  const generation = useRef(0),
    abort = useRef<AbortController>(undefined),
    publication = useRef<CsvPayload>(undefined);
  useEffect(
    () => () => {
      generation.current++;
      abort.current?.abort();
    },
    [],
  );
  const change = (changes: Partial<CsvPayload>) => {
    generation.current++;
    abort.current?.abort();
    setPreview(undefined);
    setError('');
    setPayload((current) =>
      current ? { ...current, ...changes, preview_digest: undefined } : current,
    );
  };
  const choose = async (file?: File) => {
    const run = ++generation.current;
    abort.current?.abort();
    setPayload(undefined);
    setPreview(undefined);
    setError('');
    setPage(0);
    setReading(true);
    try {
      const next = file ? await readCsvFile(file) : undefined;
      if (run === generation.current) setPayload(next);
    } catch (reason) {
      if (run === generation.current)
        setError(reason instanceof Error ? reason.message : 'Не удалось прочитать CSV.');
    } finally {
      if (run === generation.current) setReading(false);
    }
  };
  const run = async (publish: boolean) => {
    if (!payload || (publish && !preview)) return;
    const current = ++generation.current;
    abort.current?.abort();
    const controller = new AbortController();
    abort.current = controller;
    setBusy(true);
    setError('');
    if (publish) {
      if (!preview) return;
      publication.current ??= { ...payload, preview_digest: preview.preview_digest };
      setAttempted(true);
    }
    try {
      if (publish) {
        const target = publication.current;
        if (!target) return;
        const id = await publishCsv(target, controller.signal);
        if (current === generation.current) onImported(id);
      } else {
        const next = await previewCsv(payload, controller.signal);
        if (current === generation.current) {
          setPreview(next);
          setPayload((p) =>
            p ? { ...p, column_types: next.column_types, preview_digest: next.preview_digest } : p,
          );
        }
      }
    } catch (reason) {
      if (current === generation.current && !controller.signal.aborted)
        setError(reason instanceof Error ? reason.message : 'Не удалось импортировать CSV.');
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const locked = busy || reading || attempted;
  return (
    <Modal title="Импорт CSV" onClose={onClose} closeDisabled={busy && attempted}>
      <p>Создаём новую книгу. UTF-8, до 16 МиБ и 100 000 полей. Формулы из CSV остаются текстом.</p>
      <label>
        CSV-файл
        <input
          type="file"
          accept=".csv,text/csv"
          disabled={locked}
          onChange={(e) => void choose(e.target.files?.[0])}
        />
      </label>
      <label>
        Название книги
        <input
          value={payload?.title ?? ''}
          disabled={!payload || locked}
          onChange={(e) => change({ title: e.target.value })}
          maxLength={256}
        />
      </label>
      <label>
        Разделитель
        <Select
          aria-label="Разделитель"
          value={payload?.delimiter ?? 'comma'}
          disabled={!payload || locked}
          onValueChange={(value) => {
            setPage(0);
            change({ delimiter: value as CsvPayload['delimiter'], column_types: [] });
          }}
        >
          <SelectOption value="comma">Запятая</SelectOption>
          <SelectOption value="semicolon">Точка с запятой</SelectOption>
          <SelectOption value="tab">Табуляция</SelectOption>
        </Select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={payload?.header ?? false}
          disabled={!payload || locked}
          onChange={(e) => change({ header: e.target.checked })}
        />
        Первая строка содержит заголовки
      </label>
      {!!payload?.column_types.length && (
        <fieldset>
          <legend>Типы столбцов</legend>
          <p>
            Для чисел используйте точку; логические значения — TRUE или FALSE. Пустые поля остаются
            пустыми.
          </p>
          <div className="csv-column-types">
            {payload.column_types.slice(page * 20, page * 20 + 20).map((kind, index) => (
              <label key={page * 20 + index}>
                Столбец {page * 20 + index + 1}
                <Select
                  aria-label={`Тип столбца ${page * 20 + index + 1}`}
                  value={kind}
                  disabled={locked}
                  onValueChange={(value) => {
                    const types = [...payload.column_types];
                    types[page * 20 + index] = value as ColumnType;
                    change({ column_types: types });
                  }}
                >
                  <SelectOption value="text">Текст</SelectOption>
                  <SelectOption value="number">Число</SelectOption>
                  <SelectOption value="boolean">Логическое</SelectOption>
                </Select>
              </label>
            ))}
          </div>
          {payload.column_types.length > 20 && (
            <div className="csv-actions">
              <button disabled={page === 0 || locked} onClick={() => setPage((p) => p - 1)}>
                Предыдущие столбцы
              </button>
              <button
                disabled={(page + 1) * 20 >= payload.column_types.length || locked}
                onClick={() => setPage((p) => p + 1)}
              >
                Следующие столбцы
              </button>
            </div>
          )}
        </fieldset>
      )}
      {preview && (
        <section>
          <p role="status">
            {preview.rows} строк, {preview.columns} столбцов. Показаны первые 20 строк и столбцов;
            длинные значения сокращены.
          </p>
          <div className="csv-preview">
            <table aria-label="Предпросмотр CSV">
              <tbody>
                {preview.sample.map((row, index) => (
                  <tr key={index}>
                    {row.map((cell, col) =>
                      payload?.header && index === 0 ? (
                        <th key={col} scope="col">
                          {cell || '—'}
                        </th>
                      ) : (
                        <td key={col}>{cell || '—'}</td>
                      ),
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
      {error && <p role="alert">{error}</p>}
      {attempted && error && (
        <p>
          Книга могла сохраниться. Повторное создание использует тот же запрос и не создаёт копию
          книги.
        </p>
      )}
      <footer className="csv-actions">
        <button disabled={!payload || locked} onClick={() => void run(false)}>
          Предпросмотр
        </button>
        <button
          className="sharing-primary"
          disabled={!preview || busy || reading}
          onClick={() => void run(true)}
        >
          {busy && attempted ? 'Создаём…' : 'Создать книгу'}
        </button>
      </footer>
    </Modal>
  );
}
