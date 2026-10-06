import { useEffect, useRef, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { Select, SelectOption } from '../ui/Select';
import { downloadCsv, type CsvExportOptions } from './csv-download';
import type { DocumentStore } from './store';

export function CsvExportPanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const [sheet, setSheet] = useState(document.sheet?.id ?? ''),
    [delimiter, setDelimiter] = useState<CsvExportOptions['delimiter']>('comma'),
    [safeText, setSafeText] = useState(true),
    [anonymous, setAnonymous] = useState(false),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [status, setStatus] = useState('');
  const abort = useRef<AbortController>(undefined);
  useEffect(() => () => abort.current?.abort(), []);
  const allowed =
    document.canExportCsv &&
    !document.pending.length &&
    !document.editing &&
    document.projection?.sheets.some((s) => s.id === sheet);
  const run = async () => {
    const head = document.projection;
    if (!head || !allowed || busy) return;
    const controller = new AbortController();
    abort.current = controller;
    setBusy(true);
    setError('');
    setStatus('');
    try {
      const blob = await downloadCsv(
        document.documentUrl(head.document_id, '/export.csv'),
        {
          sheetId: sheet,
          version: head.version,
          delimiter,
          safeText,
          anonymized: anonymous || !!document.privacyMask,
        },
        controller.signal,
      );
      if (controller.signal.aborted) return;
      const url = URL.createObjectURL(blob);
      const link = window.document.createElement('a');
      link.href = url;
      link.download = 'PXEL.csv';
      window.document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 30000);
      setStatus('CSV скачан.');
    } catch (e) {
      if (!controller.signal.aborted)
        setError(e instanceof Error ? e.message : 'Не удалось скачать CSV.');
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  return (
    <Modal title="Экспорт CSV" onClose={onClose}>
      <p>
        Один лист, UTF-8, до 16 МиБ и 100 000 полей. Формулы заменяются результатами. Оформление и
        комментарии не входят в CSV. Скрытые строки и столбцы входят.
      </p>
      <label>
        Лист
        <Select aria-label="Лист" value={sheet} onValueChange={setSheet} disabled={busy}>
          {document.projection?.sheets.map((s) => (
            <SelectOption key={s.id} value={s.id}>
              {document.privacyMask?.sheets.find((safe) => safe.id === s.id)?.name ?? s.name}
            </SelectOption>
          ))}
        </Select>
      </label>
      <label>
        Разделитель
        <Select
          aria-label="Разделитель"
          value={delimiter}
          disabled={busy}
          onValueChange={(v) => setDelimiter(v as CsvExportOptions['delimiter'])}
        >
          <SelectOption value="comma">Запятая</SelectOption>
          <SelectOption value="semicolon">Точка с запятой</SelectOption>
          <SelectOption value="tab">Табуляция</SelectOption>
        </Select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={safeText}
          disabled={busy}
          onChange={(e) => setSafeText(e.target.checked)}
        />
        Экранировать текст, похожий на формулу
      </label>
      <p>
        В этом режиме к тексту, начинающемуся с =, +, −, @ или управляющих символов, добавляется
        апостроф. Отключите для сохранения исходного текста.
      </p>
      <label>
        <input
          type="checkbox"
          checked={anonymous || !!document.privacyMask}
          disabled={busy || !!document.privacyMask}
          onChange={(e) => setAnonymous(e.target.checked)}
        />
        Обезличить данные
      </label>
      {error && <p role="alert">{error}</p>}
      {status && <p role="status">{status}</p>}
      <button className="sharing-primary" disabled={!allowed || busy} onClick={() => void run()}>
        {busy ? 'Готовим CSV…' : 'Скачать CSV'}
      </button>
    </Modal>
  );
}
