import { useRef, useState, type FormEvent } from 'react';
import { useAccount } from './AuthGate';
import { AccountsPanel } from './AccountsPanel';
import { authenticate, logout, validPassword } from './api';
import { Modal } from '../sharing/Modal';

export function AccountControls() {
  const { account } = useAccount();
  const [panel, setPanel] = useState<'accounts' | 'password'>();
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('');
  const form = useRef<HTMLFormElement>(null);
  if (!account) return null;
  async function password(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    const data = new FormData(event.currentTarget),
      next = String(data.get('new_password') ?? '');
    if (!validPassword(next) || next !== data.get('confirmation')) {
      setError('Нужен пароль от 12 до 128 символов и совпадающее подтверждение.');
      return;
    }
    setBusy(true);
    setError('');
    try {
      await authenticate('password', {
        current_password: String(data.get('current_password') ?? ''),
        new_password: next,
      });
      form.current?.reset();
      setPanel(undefined);
    } catch (e) {
      form.current?.reset();
      setError(e instanceof Error ? e.message : 'Не удалось сменить пароль.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="auth-controls">
        <span>
          {account.username}
          {account.is_master ? ' · мастер' : ''}
        </span>
        {account.is_master && (
          <button type="button" onClick={() => setPanel('accounts')}>
            Пользователи
          </button>
        )}
        <button
          type="button"
          onClick={() => {
            setError('');
            setPanel('password');
          }}
        >
          Сменить пароль
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            void logout()
              .then(() => location.assign('/'))
              .catch(() => {
                setError('Не удалось завершить сессию.');
                setBusy(false);
              });
          }}
        >
          Выйти
        </button>
        {!panel && error && <span role="alert">{error}</span>}
      </div>
      {panel === 'accounts' && <AccountsPanel onClose={() => setPanel(undefined)} />}
      {panel === 'password' && (
        <Modal
          title="Сменить пароль"
          onClose={() => setPanel(undefined)}
          closeDisabled={busy}
          clipboard="credentials"
        >
          <form
            className="auth-admin"
            ref={form}
            onSubmit={(event) => {
              void password(event);
            }}
          >
            <p>После смены пароля остальные сессии будут завершены.</p>
            <label>
              Текущий пароль
              <input
                name="current_password"
                type="password"
                required
                maxLength={512}
                autoComplete="current-password"
                disabled={busy}
              />
            </label>
            <label>
              Новый пароль
              <input
                name="new_password"
                type="password"
                required
                minLength={12}
                maxLength={512}
                autoComplete="new-password"
                disabled={busy}
              />
            </label>
            <label>
              Повторите пароль
              <input
                name="confirmation"
                type="password"
                required
                maxLength={512}
                autoComplete="new-password"
                disabled={busy}
              />
            </label>
            {error && (
              <p className="auth-error" role="alert">
                {error}
              </p>
            )}
            <button className="auth-primary" type="submit" disabled={busy}>
              {busy ? 'Сохранение…' : 'Сохранить пароль'}
            </button>
          </form>
        </Modal>
      )}
    </>
  );
}
