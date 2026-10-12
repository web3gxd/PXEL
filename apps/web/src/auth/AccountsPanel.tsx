import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { Modal } from '../sharing/Modal';
import {
  changeAccount,
  createAccount,
  readAccounts,
  validPassword,
  validUsername,
  type Account,
} from './api';

export function AccountsPanel({ onClose }: { onClose: () => void }) {
  const [accounts, setAccounts] = useState<Account[]>([]),
    [loading, setLoading] = useState(true);
  const [error, setError] = useState(''),
    [notice, setNotice] = useState(''),
    [busy, setBusy] = useState(false);
  const [target, setTarget] = useState<Account>();
  const form = useRef<HTMLFormElement>(null);
  const reload = useCallback(async (signal?: AbortSignal) => {
    const rows = await readAccounts(signal);
    if (!signal?.aborted) setAccounts(rows);
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    void reload(controller.signal)
      .catch(() => {
        if (!controller.signal.aborted) setError('Не удалось загрузить пользователей.');
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [reload]);
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    const data = new FormData(event.currentTarget),
      username = String(data.get('username') ?? '');
    const password = String(data.get('password') ?? '');
    if ((!target && !validUsername(username)) || !validPassword(password)) {
      setError('Укажите корректный логин и временный пароль от 12 до 128 символов.');
      return;
    }
    setBusy(true);
    setError('');
    setNotice('');
    try {
      if (target) await changeAccount(target.user_id, 'reset_password', password);
      else await createAccount(username, password);
      form.current?.reset();
      setTarget(undefined);
      setNotice(
        'Готово. Передайте логин и временный пароль пользователю по защищённому каналу. При входе он задаст свой пароль.',
      );
      await reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось сохранить пользователя.');
    } finally {
      if (form.current) {
        const field = form.current.querySelector<HTMLInputElement>('input[type=password]');
        if (field) field.value = '';
      }
      setBusy(false);
    }
  }
  async function toggle(account: Account) {
    if (busy || account.is_master) return;
    if (
      account.enabled &&
      !window.confirm(`Отключить ${account.username}? Его сессии будут отозваны.`)
    )
      return;
    setBusy(true);
    setError('');
    setNotice('');
    try {
      await changeAccount(account.user_id, account.enabled ? 'disable' : 'enable');
      await reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось изменить доступ.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal title="Пользователи" onClose={onClose} closeDisabled={busy} clipboard="credentials">
      <div className="auth-admin">
        <p>
          Мастер-аккаунт управляет пользователями и имеет полный доступ ко всем книгам этой
          установки. Созданные пользователи получают доступ к другим книгам по отдельным правам.
        </p>
        {loading ? (
          <p role="status">Загрузка пользователей…</p>
        ) : (
          <ul className="auth-account-list">
            {accounts.map((account) => (
              <li key={account.user_id}>
                <div>
                  <strong>{account.username}</strong>
                  <small>
                    {account.is_master
                      ? 'Мастер-аккаунт'
                      : !account.enabled
                        ? 'Отключён'
                        : account.must_change_password
                          ? 'Ожидает смены пароля'
                          : 'Пользователь'}
                  </small>
                </div>
                {!account.is_master && (
                  <div className="auth-account-actions">
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        form.current?.reset();
                        setTarget(account);
                        setError('');
                        setNotice('');
                      }}
                    >
                      Сбросить пароль
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        void toggle(account);
                      }}
                    >
                      {account.enabled ? 'Отключить' : 'Включить'}
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        <form
          ref={form}
          onSubmit={(event) => {
            void save(event);
          }}
          aria-busy={busy}
        >
          <h3>{target ? `Новый временный пароль: ${target.username}` : 'Новый пользователь'}</h3>
          {!target && (
            <label>
              Логин
              <input
                name="username"
                required
                minLength={3}
                maxLength={32}
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                disabled={busy}
              />
              <small>3–32 символа: a–z, 0–9, . _ -</small>
            </label>
          )}
          <label>
            Временный пароль
            <input
              name="password"
              type="password"
              required
              minLength={12}
              maxLength={512}
              autoComplete="new-password"
              disabled={busy}
            />
            <small>12–128 символов. Пользователь сменит его при входе.</small>
          </label>
          <div className="auth-account-actions">
            <button className="auth-primary" type="submit" disabled={busy || loading}>
              {busy ? 'Сохранение…' : target ? 'Сбросить пароль' : 'Создать пользователя'}
            </button>
            {target && (
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  form.current?.reset();
                  setTarget(undefined);
                }}
              >
                Отмена
              </button>
            )}
          </div>
        </form>
        {error && (
          <p className="auth-error" role="alert">
            {error}
          </p>
        )}
        {notice && <p role="status">{notice}</p>}
      </div>
    </Modal>
  );
}
