import { useEffect, useState } from 'react';
import { Bell } from '@phosphor-icons/react';
import { notifications, type OwnerNotification } from './api';
import { formatExpiry } from './policy';
export function OwnerNotifications() {
  const [items, setItems] = useState<OwnerNotification[]>([]);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    const abort = new AbortController();
    const load = () =>
      void notifications(abort.signal)
        .then((value) => {
          if (!abort.signal.aborted) {
            setItems(value);
            setError('');
          }
        })
        .catch(() => {
          if (!abort.signal.aborted) setError('Не удалось загрузить уведомления.');
        })
        .finally(() => {
          if (!abort.signal.aborted) setLoading(false);
        });
    load();
    const timer = setInterval(load, 30_000);
    return () => {
      abort.abort();
      clearInterval(timer);
    };
  }, []);
  return (
    <details className="sharing-notifications">
      <summary aria-label={`Уведомления о доступе${items.length ? `: ${items.length}` : ''}`}>
        <Bell size={19} aria-hidden="true" />
        {items.length > 0 && <span className="sharing-notification-count">{items.length}</span>}
      </summary>
      <div className="sharing-notification-list">
        {loading ? (
          <p role="status">Загружаем…</p>
        ) : error ? (
          <p role="alert">{error}</p>
        ) : !items.length ? (
          <p>Уведомлений о доступе пока нет.</p>
        ) : (
          <ul>
            {items.map((item) => (
              <li key={item.notification_id}>
                <strong>Ссылка отключена</strong>
                <p>
                  После повторных неудачных попыток ввода кода доступ закрыт. Чтобы вернуть доступ,
                  создайте новую ссылку.
                </p>
                <time dateTime={new Date(item.created_at * 1000).toISOString()}>
                  {formatExpiry(item.created_at)}
                </time>
              </li>
            ))}
          </ul>
        )}
      </div>
    </details>
  );
}
