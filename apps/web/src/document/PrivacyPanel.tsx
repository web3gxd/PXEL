import { useEffect, useState } from 'react';
import { Modal } from '../sharing/Modal';
import { columnLabel } from '../grid/viewport';
import { writeSelectionClipboard } from '../clipboard/outbound';
import type { DocumentStore } from './store';
import {
  PRIVACY_TYPES,
  readPrivacy,
  savePrivacy,
  analyzePrivacy,
  type PrivacyPolicy,
  type PrivacyReport,
  type PrivacyKind,
  privacyExceptionBatch,
  privacyRangeFindings,
} from './privacy';
export function PrivacyPanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const id = document.projection!.document_id;
  const [policy, setPolicy] = useState<PrivacyPolicy>();
  const [enabled, setEnabled] = useState(false);
  const [types, setTypes] = useState<PrivacyKind[]>([]);
  const [report, setReport] = useState<PrivacyReport | undefined>(document.privacyReport);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [range, setRange] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    setReport(document.privacyReport);
    const current = new Set(document.privacyReport?.findings.map((finding) => finding.fingerprint));
    setChosen(
      (previous) => new Set([...previous].filter((fingerprint) => current.has(fingerprint))),
    );
  }, [document.privacyReport]);
  useEffect(() => {
    const abort = new AbortController();
    void readPrivacy(id, abort.signal)
      .then((p) => {
        setPolicy(p);
        setEnabled(p.rules.enabled);
        setTypes(p.rules.types);
      })
      .catch((e) => {
        if (!abort.signal.aborted)
          setError(e instanceof Error ? e.message : 'Не удалось загрузить настройки.');
      });
    return () => abort.abort();
  }, [id]);
  const save = async (exceptions = policy?.rules.exceptions ?? []) => {
    if (!policy) return;
    setBusy(true);
    setError('');
    try {
      const p = await savePrivacy(id, document.projection!.version, policy.revision, {
        enabled,
        types,
        exceptions,
      });
      setPolicy(p);
      document.privacyPolicy = p;
      setReport(undefined);
      document.privacyFindings = [];
      if (!enabled && document.privacyMask) document.refreshPrivacyMask(undefined);
      await document.refresh(BigInt(document.projection!.version));
      if (p.rules.enabled && p.rules.types.length) {
        const refreshed = await analyzePrivacy(id, document.projection!.version);
        if (
          refreshed.document_version !== document.projection?.version ||
          refreshed.policy_revision !== p.revision
        )
          throw new Error('Книга изменилась. Анализ обновится автоматически.');
        document.privacyReport = refreshed;
        setReport(refreshed);
        setChosen(new Set());
        document.privacyFindings = refreshed.findings;
        document.refreshPrivacyMask(refreshed.projection);
      } else document.privacyReport = undefined;
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось сохранить.');
    } finally {
      setBusy(false);
      document.notify();
    }
  };
  const scan = async () => {
    setBusy(true);
    setError('');
    try {
      const p = await analyzePrivacy(id, document.projection!.version);
      if (
        p.projection.document_id !== id ||
        p.document_version !== document.projection?.version ||
        p.policy_revision !== policy?.revision
      )
        throw new Error('Книга или настройки изменились. Повторите анализ.');
      setReport(p);
      document.privacyReport = p;
      document.privacyFindings = p.findings;
      document.refreshPrivacyMask(p.projection);
      document.notify();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось выполнить анализ.');
    } finally {
      setBusy(false);
    }
  };
  const stale = report && report.document_version !== document.projection?.version;
  const exclude = (findings: PrivacyReport['findings']) => {
    try {
      if (!policy || !report || stale || report.policy_revision !== policy.revision)
        throw new Error('Дождитесь актуального анализа.');
      if (!findings.length) throw new Error('В выбранных ячейках нет совпадений.');
      void save(privacyExceptionBatch(policy.rules.exceptions, findings));
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось создать исключения.');
    }
  };
  return (
    <Modal title="Персональные данные" onClose={onClose} clipboardError={document.error}>
      <p>
        Распознавание может ошибаться и пропускать данные. Проверяйте копию перед передачей.
        Оригиналы остаются в книге.
      </p>
      {!policy && !error && <p role="status">Загрузка настроек…</p>}
      {error && <p role="alert">{error}</p>}
      {policy && (
        <>
          <fieldset disabled={busy}>
            <legend>Распознавание книги</legend>
            <label>
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
              />{' '}
              Включить распознавание
            </label>
            <div className="privacy-types">
              {(Object.keys(PRIVACY_TYPES) as PrivacyKind[]).map((k) => (
                <label key={k}>
                  <input
                    type="checkbox"
                    checked={types.includes(k)}
                    onChange={(e) =>
                      setTypes((old) =>
                        e.target.checked ? [...old, k] : old.filter((t) => t !== k),
                      )
                    }
                  />
                  {PRIVACY_TYPES[k]}
                </label>
              ))}
            </div>
          </fieldset>
          <p className="sharing-help">
            ИНН, СНИЛС и карты проверяются по контрольным суммам. Для паспортов учитываются формат и
            слова «паспорт» или «серия и номер» в ячейке, её строке или столбце. После включения
            анализ и локальный фильтр обновляются автоматически.
          </p>
          <div className="sharing-actions">
            <button onClick={() => void save()} disabled={busy}>
              Сохранить настройки
            </button>
            <button
              onClick={() => void scan()}
              disabled={busy || !policy.rules.enabled || policy.rules.types.length === 0}
            >
              Обновить анализ
            </button>
          </div>
          <p>
            Исключений: {policy.rules.exceptions.length}. Они привязаны к конкретному содержимому
            ячейки.
          </p>
          {policy.rules.exceptions.length > 0 && (
            <button disabled={busy} onClick={() => void save([])}>
              Сбросить исключения
            </button>
          )}
        </>
      )}
      {report && (
        <>
          <p role="status">
            Совпадений: {report.findings.length}. Замаскировано ячеек и зависимых результатов:{' '}
            {report.affected}. Версия {report.document_version}.
          </p>
          {stale && <p role="alert">Отчёт устарел. Повторите анализ.</p>}
          <div className="sharing-actions">
            <button
              aria-pressed={!!document.privacyMask}
              disabled={busy || !!stale}
              onClick={() => {
                document.setPrivacyMask(document.privacyMask ? undefined : report.projection);
              }}
            >
              {' '}
              {document.privacyMask ? 'Показать оригиналы' : 'Локальная маска'}
            </button>
            <button
              disabled={busy || !!stale || !document.canCopy}
              onClick={() => void writeSelectionClipboard(document, 'copy', true)}
            >
              Безопасно скопировать выделение
            </button>
            <button
              disabled={busy || !!stale || !document.canExport}
              onClick={() => void document.exportXlsx(true)}
            >
              Обезличенный XLSX
            </button>
          </div>
          <ul className="privacy-findings">
            {report.findings.slice(0, 1000).map((f, i) => (
              <li key={`${f.fingerprint}/${i}`}>
                <label>
                  <input
                    type="checkbox"
                    aria-label={`Выбрать ${columnLabel(f.column)}${f.row + 1} · ${PRIVACY_TYPES[f.kind]}`}
                    checked={chosen.has(f.fingerprint)}
                    disabled={busy || !!stale}
                    onChange={(event) =>
                      setChosen((previous) => {
                        const next = new Set(previous);
                        if (event.target.checked) next.add(f.fingerprint);
                        else next.delete(f.fingerprint);
                        return next;
                      })
                    }
                  />
                </label>
                <button
                  onClick={() => {
                    document.selectSheet(f.sheet_id);
                    document.select({ row: f.row, column: f.column });
                    onClose();
                  }}
                >
                  {columnLabel(f.column)}
                  {f.row + 1} · {PRIVACY_TYPES[f.kind]}
                </button>
                <button
                  disabled={busy || !!stale || (policy?.rules.exceptions.length ?? 0) >= 1000}
                  onClick={() => exclude([f])}
                >
                  Ложное срабатывание
                </button>
              </li>
            ))}
          </ul>
          <div className="privacy-bulk">
            <h3>Исключить ложные срабатывания одной операцией</h3>
            <div className="sharing-actions">
              <button
                disabled={busy || !!stale}
                onClick={() => setChosen(new Set(report.findings.map((f) => f.fingerprint)))}
              >
                Выбрать все совпадения
              </button>
              <button
                disabled={busy || !!stale || !chosen.size}
                onClick={() => exclude(report.findings.filter((f) => chosen.has(f.fingerprint)))}
              >
                Исключить выбранные ({chosen.size})
              </button>
              <button
                disabled={busy || !!stale}
                onClick={() => {
                  const bounds = document.selectionBounds;
                  if (!bounds) return;
                  exclude(
                    report.findings.filter(
                      (f) =>
                        f.sheet_id === document.sheet?.id &&
                        f.row >= bounds.startRow &&
                        f.row <= bounds.endRow &&
                        f.column >= bounds.startColumn &&
                        f.column <= bounds.endColumn,
                    ),
                  );
                }}
              >
                Исключить в выделении таблицы
              </button>
            </div>
            <div className="privacy-range">
              <label>
                Диапазон исключений{' '}
                <input
                  value={range}
                  onChange={(event) => setRange(event.target.value)}
                  placeholder="B2:D20"
                />
              </label>
              <button
                disabled={busy || !!stale || !range.trim()}
                onClick={() => {
                  try {
                    exclude(privacyRangeFindings(report, document.sheet?.id ?? '', range));
                  } catch (e) {
                    setError(e instanceof Error ? e.message : 'Некорректный диапазон.');
                  }
                }}
              >
                Исключить диапазон
              </button>
            </div>
          </div>
          {report.findings.length > 1000 && (
            <p>В списке первые 1000 совпадений; маскирование охватывает весь отчёт.</p>
          )}
        </>
      )}
    </Modal>
  );
}
