import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { Select, SelectOption } from '../ui/Select';
import { columnLabel } from '../grid/viewport';
import {
  previewXlsx,
  publishXlsx,
  readXlsxFile,
  xlsxCellText,
  xlsxIssues,
  type XlsxPayload,
  type XlsxPreview,
  type XlsxPublication,
} from './xlsx-import';

export function XlsxImportPanel({
  onClose,
  onImported,
}: {
  onClose: () => void;
  onImported: (id: string) => void;
}) {
  const [payload, setPayload] = useState<XlsxPayload>();
  const [preview, setPreview] = useState<XlsxPreview>();
  const [selection, setSelection] = useState<number[]>([]);
  const [sheetIndex, setSheetIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [accepted, setAccepted] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [previewTitle, setPreviewTitle] = useState('');
  const generation = useRef(0),
    abort = useRef<AbortController>(undefined),
    publication = useRef<XlsxPublication>(undefined);
  useEffect(
    () => () => {
      generation.current++;
      abort.current?.abort();
    },
    [],
  );
  const choose = async (file?: File) => {
    const current = ++generation.current;
    abort.current?.abort();
    setPayload(undefined);
    setPreview(undefined);
    setSelection([]);
    setSheetIndex(0);
    setError('');
    setAccepted(false);
    setBusy(true);
    try {
      const next = file ? await readXlsxFile(file) : undefined;
      if (current === generation.current) setPayload(next);
    } catch (reason) {
      if (current === generation.current)
        setError(reason instanceof Error ? reason.message : 'Не удалось прочитать файл.');
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const run = async () => {
    if (!payload) return;
    const current = ++generation.current;
    abort.current?.abort();
    const controller = new AbortController();
    abort.current = controller;
    setBusy(true);
    setError('');
    try {
      const result = await previewXlsx(
        preview ? { ...payload, selected_sheets: selection } : payload,
        controller.signal,
      );
      if (current === generation.current) {
        setPreview(result);
        setSelection(result.selected_sheets);
        setPreviewTitle(payload.title);
        setAccepted(false);
      }
    } catch (reason) {
      if (current === generation.current && !controller.signal.aborted)
        setError(reason instanceof Error ? reason.message : 'Не удалось проверить файл.');
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const sheet = preview?.sheets[sheetIndex];
  const hasLoss = !!(preview && (preview.issues.length || preview.formula_issues?.length));
  const checked =
    !!preview &&
    preview.selected_sheets.length === selection.length &&
    preview.selected_sheets.every((index, position) => index === selection[position]) &&
    previewTitle === payload?.title;
  const create = async () => {
    if (!payload || !preview || !checked) return;
    publication.current ??= {
      ...payload,
      selected_sheets: [...selection],
      preview_digest: preview.preview_digest,
      accept_reported: accepted,
    };
    const current = ++generation.current;
    const controller = new AbortController();
    abort.current?.abort();
    abort.current = controller;
    setBusy(true);
    setAttempted(true);
    setError('');
    try {
      const id = await publishXlsx(publication.current, controller.signal);
      if (current === generation.current) onImported(id);
    } catch (reason) {
      if (current === generation.current && !controller.signal.aborted)
        setError(reason instanceof Error ? reason.message : 'Не удалось создать книгу.');
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  return (
    <Modal title="Предпросмотр XLSX" onClose={onClose} closeDisabled={busy && attempted}>
      <p>До 32 МиБ. Проверьте листы и замечания совместимости перед созданием новой книги.</p>
      <label>
        XLSX-файл
        <input
          type="file"
          accept=".xlsx,application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
          disabled={busy || attempted}
          onChange={(event) => void choose(event.target.files?.[0])}
        />
      </label>
      <label>
        Название книги
        <input
          value={payload?.title ?? ''}
          disabled={!payload || busy || attempted}
          maxLength={256}
          onChange={(event) =>
            setPayload((previous) =>
              previous ? { ...previous, title: event.target.value } : previous,
            )
          }
        />
      </label>
      {preview && (
        <>
          <fieldset className="xlsx-sheet-list">
            <legend>Листы для импорта</legend>
            {preview.sheets.map((item) => (
              <label key={item.index}>
                <input
                  type="checkbox"
                  checked={selection.includes(item.index)}
                  disabled={busy || attempted}
                  onChange={(event) =>
                    setSelection((previous) =>
                      event.target.checked
                        ? [...previous, item.index].sort((a, b) => a - b)
                        : previous.filter((index) => index !== item.index),
                    )
                  }
                />
                {item.name}
              </label>
            ))}
          </fieldset>
          <label>
            Показать лист
            <Select
              aria-label="Показать лист"
              value={String(sheetIndex)}
              onValueChange={(value) => setSheetIndex(Number(value))}
            >
              {preview.sheets.map((item) => (
                <SelectOption key={item.index} value={String(item.index)}>
                  {item.name}
                </SelectOption>
              ))}
            </Select>
          </label>
          {sheet && (
            <section>
              <p>
                {sheet.cells} заполненных ячеек, {sheet.styles} оформленных, {sheet.merges}{' '}
                объединений. Показаны первые 40 заполненных ячеек; длинные значения сокращены.
              </p>
              <div className="csv-preview">
                <table aria-label="Предпросмотр ячеек XLSX">
                  <thead>
                    <tr>
                      <th scope="col">Ячейка</th>
                      <th scope="col">Значение или формула</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sheet.sample.map((cell) => (
                      <tr key={`${cell.row}:${cell.column}`}>
                        <th scope="row">
                          {columnLabel(cell.column - 1)}
                          {cell.row}
                        </th>
                        <td>{xlsxCellText(cell.value)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          )}
          <section aria-label="Совместимость XLSX">
            <h3>Замечания совместимости</h3>
            {hasLoss ? (
              <ul>
                {preview.issues.map((issue, index) => (
                  <li key={index}>
                    {issue.sheet_index === null ? 'Книга' : preview.sheets[issue.sheet_index]?.name}
                    : {xlsxIssues[issue.code]}
                  </li>
                ))}
                {preview.formula_issues?.map((issue) => (
                  <li key={`${issue.sheet_index}:${issue.function}`}>
                    {preview.sheets[issue.sheet_index]?.name}: функция {issue.function} не
                    поддерживается ({issue.cells} ячеек). Формула сохранится, результат может
                    содержать ошибку.
                  </li>
                ))}
              </ul>
            ) : (
              <p>
                Парсер не обнаружил потерь в поддерживаемом профиле. Структура формул и ссылки
                проверены; это не гарантирует поддержку всех функций Excel.
              </p>
            )}
          </section>
          {hasLoss && (
            <label>
              <input
                type="checkbox"
                checked={accepted}
                disabled={busy || attempted}
                onChange={(event) => setAccepted(event.target.checked)}
              />
              Принимаю перечисленные ограничения совместимости
            </label>
          )}
          <p role="status">
            {checked
              ? `Проверено листов: ${selection.length}.`
              : 'Выбор листов или название изменены. Выполните проверку заново.'}
          </p>
        </>
      )}
      {error && <p role="alert">{error}</p>}
      {attempted && error && (
        <p>
          Книга могла сохраниться. Повторный запрос использует те же идентификаторы и не создаёт
          копию.
        </p>
      )}
      <footer className="csv-actions">
        <button
          className="sharing-primary"
          disabled={!payload || busy || attempted || (!!preview && !selection.length)}
          onClick={() => void run()}
        >
          {busy ? 'Проверяем…' : preview ? 'Проверить выбранные листы' : 'Предпросмотр'}
        </button>
        <button
          className="sharing-primary"
          disabled={!checked || busy || (hasLoss && !accepted)}
          onClick={() => void create()}
        >
          {busy && attempted ? 'Создаём…' : 'Создать книгу'}
        </button>
      </footer>
    </Modal>
  );
}
