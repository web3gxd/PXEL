import { useEffect, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { boundedJson, isUuid } from '../sharing/api';
import { columnLabel } from '../grid/viewport';
import type { DocumentStore } from './store';
import type { HardcodeReport } from './diagnostics';
export function decodeHardcodeReport(raw: unknown): HardcodeReport {
  const value = raw as HardcodeReport;
  if (
    !value ||
    value.schema_version !== 1 ||
    !/^\d+$/.test(value.document_version) ||
    !Number.isSafeInteger(value.formulas) ||
    value.formulas < 0 ||
    value.formulas > 100000 ||
    !Array.isArray(value.warnings) ||
    value.warnings.length > 1000 ||
    value.warnings.some(
      (warning) =>
        !warning ||
        ![warning.sheet_id, warning.row_id, warning.column_id].every(isUuid) ||
        !/^[0-9a-f]{64}$/.test(warning.fingerprint) ||
        !['row', 'column'].includes(warning.series_axis) ||
        !Number.isInteger(warning.row) ||
        warning.row < 0 ||
        warning.row >= 100000 ||
        !Number.isInteger(warning.column) ||
        warning.column < 0 ||
        warning.column >= 1000 ||
        !Number.isSafeInteger(warning.formulas) ||
        warning.formulas < 100 ||
        !Number.isSafeInteger(warning.dominant) ||
        warning.dominant * 100 < warning.formulas * 90 ||
        warning.dominant > warning.formulas,
    )
  )
    throw new Error('Некорректный отчёт диагностики.');
  return value;
}
export function HardcodePanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const id = document.projection!.document_id,
    version = document.projection!.version;
  const [result, setResult] = useState<HardcodeReport>();
  const [error, setError] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    void (async () => {
      try {
        const response = await fetch(`/api/documents/${id}/diagnostics`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ schema_version: 1, document_version: version }),
          signal: controller.signal,
        });
        if (!response.ok)
          throw new Error(
            response.status === 409
              ? 'Книга изменилась. Откройте диагностику заново.'
              : 'Анализ недоступен или превысил лимит.',
          );
        const value = decodeHardcodeReport(await boundedJson(response, 512 * 1024));
        if (!controller.signal.aborted) setResult(value);
      } catch (failure) {
        if (!controller.signal.aborted)
          setError(failure instanceof Error ? failure.message : 'Анализ недоступен.');
      }
    })();
    return () => controller.abort();
  }, [id, version]);
  const dismissed = new Set(document.personalView.settings.hardcode_dismissals ?? []);
  return (
    <Modal title="Проверка констант" onClose={onClose} clipboardError={document.error}>
      <p>
        Это эвристика проверки, а не ошибка вычисления. Анализируются строки и столбцы с минимум 100
        формулами; общий шаблон должен занимать не менее 90%. Типовые константы всей серии не
        вызывают предупреждения.
      </p>
      <p className="syntax-legend">
        <span className="syntax-reference">Ссылки</span> ·{' '}
        <span className="syntax-number">Числа</span> · <span className="syntax-text">Строки</span> ·{' '}
        <span className="syntax-boolean">Логические константы</span>
      </p>
      {error && <p role="alert">{error}</p>}
      {!result && !error && <p role="status">Анализ формул…</p>}
      {result && (
        <>
          <p role="status">
            Версия {result.document_version}: проверено формул {result.formulas}; предупреждений{' '}
            {result.warnings.filter((warning) => !dismissed.has(warning.fingerprint)).length}.
          </p>
          {result.document_version !== version && (
            <p role="alert">Отчёт устарел после изменения книги.</p>
          )}
          <ul className="diagnostics-list">
            {result.warnings
              .filter((warning) => !dismissed.has(warning.fingerprint))
              .map((warning) => (
                <li key={warning.fingerprint}>
                  <button
                    type="button"
                    onClick={() => {
                      document.selectSheet(warning.sheet_id);
                      document.select({ row: warning.row, column: warning.column });
                      onClose();
                    }}
                  >
                    ⚠ {columnLabel(warning.column)}
                    {warning.row + 1}
                  </button>
                  <p>
                    Константы отличаются от общего шаблона{' '}
                    {warning.series_axis === 'row' ? 'строки' : 'столбца'}: {warning.dominant} из{' '}
                    {warning.formulas} формул совпадают. Проверьте намерение автора.
                  </p>
                  <button
                    type="button"
                    disabled={document.viewLoading || result.document_version !== version}
                    onClick={() => {
                      void document
                        .saveFilters({
                          ...document.personalView.settings,
                          hardcode_dismissals: [...dismissed, warning.fingerprint],
                        })
                        .catch((failure: unknown) =>
                          setError(
                            failure instanceof Error
                              ? failure.message
                              : 'Не удалось скрыть предупреждение.',
                          ),
                        );
                    }}
                  >
                    Скрыть предупреждение {columnLabel(warning.column)}
                    {warning.row + 1}
                  </button>
                </li>
              ))}
          </ul>
        </>
      )}
    </Modal>
  );
}
