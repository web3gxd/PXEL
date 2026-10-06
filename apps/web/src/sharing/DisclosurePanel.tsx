import { Select, SelectOption } from '../ui/Select';
import { useEffect, useState } from 'react';
import type { DocumentStore } from '../document/store';
import { boundedJson, listShares, type Share } from './api';
import {
  readPolicy,
  savePolicy,
  decodeAuditPage,
  type DisclosurePolicy,
  type DisclosureRules,
  type DisclosureAuditEvent,
} from './disclosure';
import { Modal } from './Modal';

const reasons: Record<string, string> = {
  allowed: 'Разрешено',
  role_denied: 'Роль не разрешает действие',
  copy_denied: 'Копирование запрещено',
  format_denied: 'Формат запрещён',
  anonymization_required: 'Требуется обезличивание',
  policy_changed: 'Политика изменилась',
  version_changed: 'Версия изменилась',
  privacy_analysis_failed: 'Анализ обезличивания не завершён — данные не выданы',
};
export function DisclosurePanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const id = document.projection!.document_id;
  const [scope, setScope] = useState('');
  const [shares, setShares] = useState<Share[]>([]);
  const [policy, setPolicy] = useState<DisclosurePolicy>();
  const [rules, setRules] = useState<DisclosureRules>();
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [status, setStatus] = useState('');
  const [events, setEvents] = useState<DisclosureAuditEvent[]>([]),
    [cursor, setCursor] = useState<string | null>(null);
  const [auditBusy, setAuditBusy] = useState(false);
  const path = `/api/documents/${encodeURIComponent(id)}/disclosure-policy${scope ? `?link_id=${encodeURIComponent(scope)}` : ''}`;
  useEffect(() => {
    const abort = new AbortController();
    void listShares(id, abort.signal)
      .then(setShares)
      .catch(() => {
        if (!abort.signal.aborted) setError('Не удалось загрузить ссылки.');
      });
    return () => abort.abort();
  }, [id]);
  useEffect(() => {
    const abort = new AbortController();
    setPolicy(undefined);
    setRules(undefined);
    setError('');
    setStatus('');
    void readPolicy(path, abort.signal)
      .then((value) => {
        if (!abort.signal.aborted) {
          setPolicy(value);
          setRules(value.rules);
        }
      })
      .catch((error) => {
        if (!abort.signal.aborted)
          setError(error instanceof Error ? error.message : 'Не удалось загрузить политику.');
      });
    return () => abort.abort();
  }, [path]);
  const update = (patch: Partial<DisclosureRules>) =>
    setRules((previous) => (previous ? { ...previous, ...patch } : previous));
  const save = async () => {
    if (!policy || !rules || busy) return;
    setBusy(true);
    setError('');
    setStatus('');
    try {
      const value = await savePolicy(path, policy, rules);
      setPolicy(value);
      setRules(value.rules);
      let refreshed = false;
      // A policy change can reconnect the socket and supersede an HTTP refresh.
      for (let attempt = 0; attempt < 3 && !refreshed; attempt++) {
        refreshed = await document.refresh();
        if (
          !scope &&
          document.projection?.capabilities?.disclosure_stamp.book_revision !== value.revision
        )
          refreshed = false;
      }
      if (!refreshed)
        throw new Error('Политика сохранена. Не удалось обновить права; перезагрузите таблицу.');
      setStatus('Политика сохранена. Действующие гостевые сессии закрыты.');
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Не удалось сохранить политику.');
    } finally {
      setBusy(false);
    }
  };
  const loadAudit = async (next: string | null = null) => {
    setAuditBusy(true);
    setError('');
    try {
      const response = await fetch(
        `/api/documents/${encodeURIComponent(id)}/audit${next ? `?cursor=${encodeURIComponent(next)}` : ''}`,
        { cache: 'no-store', signal: AbortSignal.timeout(10_000) },
      );
      if (!response.ok) throw new Error('Не удалось прочитать журнал выдачи.');
      const value = decodeAuditPage(await boundedJson(response));
      setEvents(value.events);
      setCursor(value.next_cursor ?? null);
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Не удалось прочитать журнал.');
    } finally {
      setAuditBusy(false);
    }
  };
  return (
    <Modal title="Выдача и аудит" onClose={onClose} clipboardError={document.error}>
      <p className="sharing-intro">
        Политика управляет копированием и выгрузкой средствами PXEL. Она не предотвращает снимки
        экрана и использование уже полученных данных.
      </p>
      <label className="sharing-label">
        Политика для
        <Select
          aria-label="Политика для"
          value={scope}
          disabled={busy}
          onValueChange={(event) => setScope(event)}
        >
          <SelectOption value="">Книги</SelectOption>
          {shares
            .filter((share) => share.status === 'active')
            .map((share, index) => (
              <SelectOption key={share.link_id} value={share.link_id}>
                Ссылка {index + 1} ·{' '}
                {share.permission === 'view'
                  ? 'Просмотр'
                  : share.permission === 'comment'
                    ? 'Комментарии'
                    : 'Редактирование'}
              </SelectOption>
            ))}
        </Select>
      </label>
      {!rules ? (
        <p role="status">Загружаем политику…</p>
      ) : (
        <fieldset disabled={busy} className="sharing-fieldset disclosure-fields">
          <legend>Разрешённая выдача</legend>
          <label>
            <input
              type="checkbox"
              checked={rules.allow_copy}
              onChange={(event) => update({ allow_copy: event.target.checked })}
            />
            Копирование и вырезание
          </label>
          <label>
            <input
              type="checkbox"
              checked={rules.export_formats.includes('xlsx')}
              onChange={(event) =>
                update({
                  export_formats: event.target.checked
                    ? [...rules.export_formats.filter((f) => f !== 'xlsx'), 'xlsx']
                    : rules.export_formats.filter((f) => f !== 'xlsx'),
                })
              }
            />
            Выгрузка XLSX
          </label>
          <label>
            <input
              type="checkbox"
              checked={rules.export_formats.includes('pxel')}
              onChange={(event) =>
                update({
                  export_formats: event.target.checked
                    ? [...rules.export_formats.filter((f) => f !== 'pxel'), 'pxel']
                    : rules.export_formats.filter((f) => f !== 'pxel'),
                })
              }
            />
            Выгрузка PXEL
          </label>
          <label>
            <input
              type="checkbox"
              checked={rules.allow_participant_export}
              onChange={(event) => update({ allow_participant_export: event.target.checked })}
            />
            {scope
              ? 'Разрешить выгрузку получателям этой ссылки'
              : 'Разрешить выгрузку участникам и гостям'}
          </label>
          <label>
            <input
              type="checkbox"
              checked={rules.require_anonymization}
              onChange={(event) => update({ require_anonymization: event.target.checked })}
            />
            Требовать обезличивание перед выдачей
          </label>
          <p className="sharing-help">
            Права книги и ссылки применяются совместно. Требование обезличивания блокирует выдачу,
            пока безопасная копия не подготовлена. Политика не предоставляет права редактирования.
          </p>
          <button
            type="button"
            className="sharing-primary"
            disabled={busy}
            onClick={() => void save()}
          >
            {busy ? 'Сохраняем…' : 'Сохранить политику'}
          </button>
        </fieldset>
      )}
      {status && <p role="status">{status}</p>}
      {error && (
        <p role="alert" className="sharing-error">
          {error}
        </p>
      )}
      <section aria-label="Журнал выдачи">
        <h3>Журнал попыток</h3>
        <button
          type="button"
          className="sharing-secondary"
          disabled={auditBusy}
          onClick={() => void loadAudit()}
        >
          {auditBusy ? 'Загружаем…' : 'Показать журнал'}
        </button>
        {events.length ? (
          <ul className="disclosure-audit">
            {events.map((event) => (
              <li key={event.event_id}>
                <strong>
                  {event.action === 'copy'
                    ? 'Копирование'
                    : event.action === 'cut'
                      ? 'Вырезание'
                      : 'Экспорт XLSX'}{' '}
                  · {reasons[event.reason] ?? 'Неизвестное решение'}
                </strong>
                <span>
                  {new Date(event.created_at * 1000).toLocaleString('ru')} · Версия{' '}
                  {event.document_version} · Автор {event.actor_id}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="sharing-help">
            Загрузите журнал, чтобы увидеть разрешённые и отклонённые попытки без содержимого
            таблицы.
          </p>
        )}
        {cursor && (
          <button type="button" disabled={auditBusy} onClick={() => void loadAudit(cursor)}>
            Следующая страница
          </button>
        )}
      </section>
    </Modal>
  );
}
