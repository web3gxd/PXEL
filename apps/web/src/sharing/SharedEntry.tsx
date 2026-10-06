import { useEffect, useRef, useState, type ReactNode } from 'react';
import { guestSession, isUuid7, SharingError, unlockShare, type GuestSession } from './api';
export type ShareRoute = { linkId?: string; token?: string };
export function shareRoute(pathname: string, hash: string): ShareRoute | undefined {
  if (!pathname.startsWith('/share/') && pathname !== '/share') return undefined;
  const match = /^\/share\/([^/]+)\/?$/.exec(pathname);
  const linkId = match?.[1];
  return {
    ...(isUuid7(linkId) ? { linkId } : {}),
    ...(/^#[0-9a-f]{64}$/i.test(hash) ? { token: hash.slice(1).toLowerCase() } : {}),
  };
}
export function SharedEntry({
  route,
  children,
}: {
  route: ShareRoute;
  children: (guest: GuestSession) => ReactNode;
}) {
  const token = useRef(route.token);
  const alive = useRef(true);
  const [session, setSession] = useState<GuestSession>();
  const [secret, setSecret] = useState('');
  const [loading, setLoading] = useState(Boolean(route.linkId));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(
    route.linkId ? '' : 'Ссылка или код недоступны. Обратитесь к владельцу.',
  );
  const [retryAt, setRetryAt] = useState(0);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    alive.current = true;
    const abort = new AbortController();
    // The fragment is never sent with page/API requests or placed in storage.
    if (location.hash) history.replaceState(null, '', `${location.pathname}${location.search}`);
    if (route.linkId)
      void guestSession(route.linkId, abort.signal)
        .then((value) => {
          if (!abort.signal.aborted) {
            setSession(value);
            token.current = undefined;
          }
        })
        .catch(() => {})
        .finally(() => {
          if (!abort.signal.aborted) setLoading(false);
        });
    return () => {
      alive.current = false;
      abort.abort();
      queueMicrotask(() => {
        if (!alive.current) token.current = undefined;
      });
    };
  }, [route.linkId]);
  useEffect(() => {
    if (!retryAt) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [retryAt]);
  const remaining = Math.max(0, Math.ceil((retryAt - now) / 1000));
  const unlock = async () => {
    if (
      !route.linkId ||
      !token.current ||
      busy ||
      remaining ||
      !secret ||
      [...secret].length > 128
    ) {
      if (!token.current) setError('Для открытия нужна полная ссылка от владельца.');
      return;
    }
    setBusy(true);
    setError('');
    try {
      const admitted = await unlockShare(route.linkId, token.current, secret);
      if (alive.current) {
        setSecret('');
        token.current = undefined;
        setSession(admitted);
      }
    } catch (reason) {
      if (alive.current) {
        setSecret('');
        if (reason instanceof SharingError && reason.code === 'locked') {
          const time = Date.now();
          setNow(time);
          setRetryAt(time + reason.retryAfter * 1000);
        }
        setError(
          reason instanceof SharingError
            ? reason.message
            : 'Ссылка или код недоступны. Обратитесь к владельцу.',
        );
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  if (session) return children(session);
  return (
    <main className="shared-unlock">
      <div className="shared-unlock-card">
        <span className="shared-wordmark">PXEL</span>
        <h1>Открыть таблицу</h1>
        <p>Введите PIN или пароль, который владелец передал отдельно от ссылки.</p>
        {loading ? (
          <p role="status">Проверяем доступ…</p>
        ) : (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void unlock();
            }}
          >
            <label className="sharing-label">
              PIN или пароль
              <input
                autoFocus
                type="password"
                autoComplete="off"
                value={secret}
                onChange={(event) => setSecret(event.target.value)}
                disabled={busy || remaining > 0 || !route.linkId}
                aria-describedby="shared-unlock-error"
              />
            </label>
            <button
              className="sharing-primary"
              type="submit"
              disabled={
                !route.linkId || !secret || [...secret].length > 128 || busy || remaining > 0
              }
            >
              {busy ? 'Открываем…' : 'Открыть таблицу'}
            </button>
          </form>
        )}
        {error && (
          <p id="shared-unlock-error" className="sharing-error" role="alert">
            {error}
          </p>
        )}
        {remaining > 0 && <p role="status">Повторить можно через {remaining} с.</p>}
        <p className="sharing-help">Доступ ограничен этой таблицей. Аккаунт PXEL не требуется.</p>
      </div>
    </main>
  );
}
