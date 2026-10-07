import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { Select, SelectOption } from '../ui/Select';
import { columnLabel } from '../grid/viewport';
import {
  previewXlsx,
  readXlsxFile,
  xlsxCellText,
  xlsxIssues,
  type XlsxPayload,
  type XlsxPreview,
} from './xlsx-import';

export function XlsxImportPanel({ onClose }: { onClose: () => void }) {
  const [payload, setPayload] = useState<XlsxPayload>();
  const [preview, setPreview] = useState<XlsxPreview>();
  const [selection, setSelection] = useState<number[]>([]);
  const [sheetIndex, setSheetIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const generation = useRef(0),
    abort = useRef<AbortController>(undefined);
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
      }
    } catch (reason) {
      if (current === generation.current && !controller.signal.aborted)
        setError(reason instanceof Error ? reason.message : 'Не удалось проверить файл.');
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const sheet = preview?.sheets[sheetIndex];
  const checked =
    !!preview &&
    preview.selected_sheets.length === selection.length &&
    preview.selected_sheets.every((index, position) => index === selection[position]);
  return (
    <Modal title="Предпросмотр XLSX" onClose={onClose}>
      <p>
        До 32 МиБ. Проверьте листы и замечания совместимости. Создание книги из XLSX пока
        недоступно.
      </p>
      <label>
        XLSX-файл
        <input
          type="file"
          accept=".xlsx,application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
          disabled={busy}
          onChange={(event) => void choose(event.target.files?.[0])}
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
                  disabled={busy}
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
            {preview.issues.length ? (
              <ul>
                {preview.issues.map((issue, index) => (
                  <li key={index}>
                    {issue.sheet_index === null ? 'Книга' : preview.sheets[issue.sheet_index]?.name}
                    : {xlsxIssues[issue.code]}
                  </li>
                ))}
              </ul>
            ) : (
              <p>
                Парсер не обнаружил потерь в поддерживаемом профиле. Совместимость формул при
                создании книги ещё не проверена.
              </p>
            )}
          </section>
          <p role="status">
            {checked
              ? `Проверено листов: ${selection.length}.`
              : 'Выбор листов изменён. Выполните проверку заново.'}
          </p>
        </>
      )}
      {error && <p role="alert">{error}</p>}
      <footer className="csv-actions">
        <button
          className="sharing-primary"
          disabled={!payload || busy || (!!preview && !selection.length)}
          onClick={() => void run()}
        >
          {busy ? 'Проверяем…' : preview ? 'Проверить выбранные листы' : 'Предпросмотр'}
        </button>
      </footer>
    </Modal>
  );
}
