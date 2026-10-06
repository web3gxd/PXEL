import { useEffect, useRef, useState } from 'react';
import { uuidV7 } from '../document/store';
import {
  createShare,
  listShares,
  revokeShare,
  type Share,
  type SharePermission,
  type ShareTtl,
} from './api';
import { Modal } from './Modal';
import { PinInput } from './PinInput';
import {
  formatExpiry,
  generatePin,
  INDEFINITE_CONFIRMATION,
  TTL_OPTIONS,
  validPassword,
} from './policy';
export const PERMISSION_LABELS = {
  view: 'Просмотр',
  comment: 'Комментирование',
  edit: 'Редактирование',
} as const;
type CreateBody = Parameters<typeof createShare>[1];
export function SharingDialog({
  documentId,
  onClose,
}: {
  documentId: string;
  onClose: () => void;
}) {
  const [permission, setPermission] = useState<SharePermission>('view');
  const [anonymized, setAnonymized] = useState(false);
  const [ttlIndex, setTtlIndex] = useState(1);
  const [confirmed, setConfirmed] = useState(false);
  const [secretKind, setSecretKind] = useState<'pin' | 'password'>('pin');
  const [pin, setPin] = useState('');
  const [password, setPassword] = useState('');
  const [shares, setShares] = useState<Share[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [copyStatus, setCopyStatus] = useState('');
  const [uncertain, setUncertain] = useState(false);
  const [created, setCreated] = useState<{ share: Share; url?: string; secret?: string }>();
  const pending = useRef<CreateBody | undefined>(undefined);
  const alive = useRef(true);
  const option = TTL_OPTIONS[ttlIndex] ?? TTL_OPTIONS[1]!;
  const secretValid = secretKind === 'pin' ? /^[0-9]{6}$/.test(pin) : validPassword(password);
  useEffect(() => {
    alive.current = true;
    const abort = new AbortController();
    void listShares(documentId, abort.signal)
      .then(setShares)
      .catch(() => {
        if (!abort.signal.aborted)
          setError('Не удалось загрузить ссылки. Закройте окно и повторите.');
      })
      .finally(() => {
        if (!abort.signal.aborted) setLoading(false);
      });
    return () => {
      alive.current = false;
      pending.current = undefined;
      abort.abort();
    };
  }, [documentId]);
  const copy = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      if (alive.current) setCopyStatus(`${label} скопирован${label === 'Ссылка' ? 'а' : ''}.`);
    } catch {
      if (alive.current)
        setCopyStatus('Не удалось скопировать. Выделите текст и скопируйте вручную.');
    }
  };
  const submit = async () => {
    if (busy || (!uncertain && (!secretValid || (option.value === 'indefinite' && !confirmed))))
      return;
    pending.current ??= {
      request_id: uuidV7(),
      permission,
      anonymized,
      ttl: option.value as ShareTtl,
      confirm_indefinite: option.value === 'indefinite' && confirmed,
      ...(secretKind === 'pin' ? { pin } : { password }),
    };
    setBusy(true);
    setError('');
    try {
      const body = pending.current;
      const result = await createShare(documentId, body);
      if (!alive.current) return;
      setCreated({ ...result, ...(result.url ? { secret: body.pin ?? body.password } : {}) });
      setShares((previous) =>
        [result.share, ...previous.filter((share) => share.link_id !== result.share.link_id)].slice(
          0,
          100,
        ),
      );
      pending.current = undefined;
      setUncertain(false);
      setPin('');
      setPassword('');
    } catch (reason) {
      if (alive.current) {
        setUncertain(true);
        setError(
          reason instanceof Error ? reason.message : 'Ответ не получен. Повторите тот же запрос.',
        );
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  const revoke = async (share: Share) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await revokeShare(documentId, share.link_id, uuidV7());
      if (!alive.current) return;
      setShares((previous) =>
        previous.map((item) =>
          item.link_id === share.link_id ? { ...item, status: 'revoked' } : item,
        ),
      );
      if (created?.share.link_id === share.link_id) {
        setCreated(undefined);
        setPin('');
        setPassword('');
      }
    } catch {
      if (alive.current) setError('Не удалось отозвать ссылку. Проверьте список и повторите.');
    } finally {
      if (alive.current) setBusy(false);
    }
  };
  return (
    <Modal title="Поделиться таблицей" onClose={onClose} clipboard="credentials">
      <p className="sharing-intro">
        Получатель ссылки и PIN или пароля получает выбранные права. Передавайте секрет отдельно от
        ссылки. Уже полученные копии нельзя отозвать.
      </p>
      {!created && (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <fieldset disabled={busy || uncertain} className="sharing-fieldset">
            <legend>Права получателя</legend>
            <div className="sharing-permissions">
              {(['view', 'comment', 'edit'] as const).map((value) => (
                <label key={value}>
                  <input
                    type="radio"
                    name="share-permission"
                    value={value}
                    checked={permission === value}
                    disabled={anonymized && value === 'edit'}
                    onChange={() => setPermission(value)}
                  />
                  {PERMISSION_LABELS[value]}
                </label>
              ))}
            </div>
          </fieldset>
          <fieldset disabled={busy || uncertain}>
            <legend>Персональные данные</legend>
            <label>
              <input
                type="checkbox"
                checked={anonymized}
                onChange={(e) => {
                  setAnonymized(e.target.checked);
                  if (e.target.checked && permission === 'edit') setPermission('view');
                }}
              />{' '}
              Обезличенное представление
            </label>
            <p className="sharing-help">
              Только рассчитанные значения и маски, без формул и истории. Перед созданием включите
              распознавание в настройках книги.
            </p>
          </fieldset>
          <fieldset disabled={busy || uncertain} className="sharing-fieldset">
            <legend>Защита ссылки</legend>
            <div className="sharing-secret-tabs">
              <button
                type="button"
                aria-pressed={secretKind === 'pin'}
                onClick={() => {
                  setSecretKind('pin');
                  setPassword('');
                }}
              >
                PIN
              </button>
              <button
                type="button"
                aria-pressed={secretKind === 'password'}
                onClick={() => {
                  setSecretKind('password');
                  setPin('');
                }}
              >
                Пароль
              </button>
            </div>
            {secretKind === 'pin' ? (
              <>
                <PinInput value={pin} onChange={setPin} disabled={busy || uncertain} />
                <button
                  type="button"
                  className="sharing-secondary"
                  onClick={() => setPin(generatePin())}
                >
                  Сгенерировать PIN
                </button>
              </>
            ) : (
              <label className="sharing-label">
                Пароль
                <input
                  type="password"
                  autoComplete="new-password"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                  aria-describedby="share-password-help"
                />
                <span id="share-password-help" className="sharing-help">
                  От 10 до 128 символов. Пробелы и Unicode сохраняются.
                </span>
              </label>
            )}
          </fieldset>
          <fieldset
            disabled={busy || uncertain}
            className={`sharing-fieldset${option.value === 'indefinite' ? ' sharing-indefinite' : ''}`}
          >
            <legend>Срок доступа</legend>
            <input
              aria-label="Срок доступа"
              aria-valuetext={option.label}
              type="range"
              min="0"
              max="3"
              step="1"
              value={ttlIndex}
              onChange={(event) => {
                setTtlIndex(Number(event.target.value));
                setConfirmed(false);
              }}
            />
            <div className="sharing-ttl-labels" aria-hidden="true">
              {TTL_OPTIONS.map((item) => (
                <span key={item.value}>{item.label}</span>
              ))}
            </div>
            {option.seconds !== null ? (
              <p className="sharing-help">
                Примерно до {formatExpiry(Math.floor(Date.now() / 1000) + option.seconds)}. Точное
                время задаёт сервер при создании.
              </p>
            ) : (
              <div className="sharing-warning">
                <strong>
                  <span className="sharing-warning-icon" aria-hidden="true">
                    ⚠
                  </span>{' '}
                  Бессрочный доступ требует внимания
                </strong>
                <p>
                  Ссылка будет работать, пока вы её не отзовёте. Регулярно проверяйте, кому доступна
                  таблица.
                </p>
                <label>
                  <input
                    type="checkbox"
                    checked={confirmed}
                    onChange={(event) => setConfirmed(event.target.checked)}
                  />
                  {INDEFINITE_CONFIRMATION}
                </label>
              </div>
            )}
          </fieldset>
          {uncertain && (
            <p className="sharing-help">
              Ответ мог потеряться после создания. Повторный запрос проверит ту же ссылку; если
              токен уже выдан, её потребуется отозвать и создать новую.
            </p>
          )}
          <p className="sharing-help" aria-live="polite">
            Выдача: {PERMISSION_LABELS[permission]} · {option.label}.{' '}
            {anonymized
              ? 'Обезличенные значения и комментарии; распознавание может пропускать данные.'
              : 'Персональные данные в ячейках, формулах и комментариях передаются в исходном виде.'}
          </p>
          <button
            className="sharing-primary"
            type="submit"
            disabled={
              busy ||
              loading ||
              (!uncertain && (!secretValid || (option.value === 'indefinite' && !confirmed)))
            }
          >
            {busy ? 'Создаём…' : uncertain ? 'Проверить тот же запрос' : 'Создать ссылку'}
          </button>
        </form>
      )}
      {created && (
        <section className="sharing-created" aria-label="Созданная ссылка">
          <h3>Ссылка создана</h3>
          <p>
            {PERMISSION_LABELS[created.share.permission]} · {formatExpiry(created.share.expires_at)}
          </p>
          {created.url ? (
            <>
              <label className="sharing-label">
                Ссылка
                <input
                  readOnly
                  value={created.url}
                  onFocus={(event) => event.currentTarget.select()}
                />
              </label>
              <button
                className="sharing-primary"
                type="button"
                onClick={() => void copy(created.url!, 'Ссылка')}
              >
                Копировать ссылку
              </button>
              <label className="sharing-label">
                {created.share.secret_kind === 'pin' ? 'PIN' : 'Пароль'}
                <input
                  readOnly
                  value={created.secret ?? ''}
                  onFocus={(event) => event.currentTarget.select()}
                />
              </label>
              <button
                className="sharing-secondary"
                type="button"
                onClick={() =>
                  void copy(
                    created.secret ?? '',
                    created.share.secret_kind === 'pin' ? 'PIN' : 'Пароль',
                  )
                }
              >
                Копировать {created.share.secret_kind === 'pin' ? 'PIN' : 'пароль'}
              </button>
              <p className="sharing-help">
                Передайте PIN или пароль другим каналом, отдельно от ссылки. После закрытия окна
                восстановить их здесь нельзя.
              </p>
            </>
          ) : (
            <>
              <p role="status">
                Токен этой ссылки уже был выдан и больше не возвращается. Отзовите ссылку и создайте
                новую.
              </p>
              <button type="button" disabled={busy} onClick={() => void revoke(created.share)}>
                Отозвать и создать заново
              </button>
            </>
          )}
        </section>
      )}
      {copyStatus && <p role="status">{copyStatus}</p>}
      {error && (
        <p className="sharing-error" role="alert">
          {error}
        </p>
      )}
      <section className="sharing-existing">
        <h3>Ссылки доступа</h3>
        {loading ? (
          <p role="status">Загружаем ссылки…</p>
        ) : shares.length ? (
          <ul>
            {shares.map((share) => (
              <li key={share.link_id}>
                <div>
                  <strong>{PERMISSION_LABELS[share.permission]}</strong>
                  {share.anonymized_at_creation && <span>Обезличенная при создании</span>}
                  <span>
                    {share.secret_kind === 'pin' ? 'PIN' : 'Пароль'} ·{' '}
                    {share.expires_at === null ? (
                      <strong className="sharing-expiry-indefinite">Бессрочно</strong>
                    ) : (
                      formatExpiry(share.expires_at)
                    )}
                  </span>
                  <span>
                    {
                      (
                        {
                          active: 'Активна',
                          revoked: 'Отозвана',
                          expired: 'Истекла',
                          disabled: 'Отключена после неудачных попыток',
                        } as const
                      )[share.status]
                    }
                  </span>
                </div>
                {share.status === 'active' && (
                  <button type="button" disabled={busy} onClick={() => void revoke(share)}>
                    Отозвать
                  </button>
                )}
              </li>
            ))}
          </ul>
        ) : (
          <p>Ссылок пока нет.</p>
        )}
      </section>
    </Modal>
  );
}
