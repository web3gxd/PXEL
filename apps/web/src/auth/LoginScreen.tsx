import { useRef, useState, type FormEvent } from 'react';
import { BookOpen, ShieldCheck, UsersThree, ArrowRight } from '@phosphor-icons/react';
import {
  authenticate,
  logout,
  readStatus,
  validPassword,
  validUsername,
  type AuthStatus,
} from './api';
import { Legal } from './Legal';

export function LoginScreen({
  status,
  onAuthenticated,
}: {
  status: AuthStatus;
  onAuthenticated: (status: AuthStatus) => void;
}) {
  const setup = status.mode === 'setup',
    change = status.mode === 'password_change';
  const [step, setStep] = useState(setup ? 0 : 1);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('');
  const form = useRef<HTMLFormElement>(null);
  const title = setup
    ? 'Ваше пространство для таблиц'
    : change
      ? 'Задайте свой пароль'
      : 'С возвращением';
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    const fields = new FormData(event.currentTarget);
    const username = String(fields.get('username') ?? '');
    const password = String(fields.get('password') ?? '');
    if (!change && !validUsername(username)) {
      setError('Логин: от 3 до 32 символов a–z, 0–9, точки, дефисы и подчёркивания.');
      return;
    }
    if ((setup || change) && !validPassword(password)) {
      setError('Пароль должен содержать от 12 до 128 символов.');
      return;
    }
    if ((setup || change) && password !== fields.get('confirmation')) {
      setError('Пароли не совпадают.');
      return;
    }
    setBusy(true);
    setError('');
    try {
      const input = change
        ? { current_password: String(fields.get('current_password') ?? ''), new_password: password }
        : setup
          ? { username, password, bootstrap_token: String(fields.get('bootstrap_token') ?? '') }
          : { username, password };
      const next = await authenticate(change ? 'password' : setup ? 'setup' : 'login', input);
      form.current?.reset();
      onAuthenticated(next);
    } catch (e) {
      form.current?.querySelectorAll<HTMLInputElement>('input[type=password]').forEach((field) => {
        field.value = '';
      });
      setError(e instanceof Error ? e.message : 'Не удалось выполнить вход.');
    } finally {
      setBusy(false);
    }
  }
  if (status.mode === 'unavailable')
    return (
      <main className="auth-page">
        <section className="auth-card">
          <BookOpen size={32} />
          <h1>Установка ещё не подготовлена</h1>
          <p>
            Администратору сервера нужно завершить настройку PXEL. После этого здесь появится
            создание мастер-аккаунта или вход.
          </p>
          <button type="button" onClick={() => location.reload()}>
            Проверить снова
          </button>
          <Legal />
        </section>
      </main>
    );
  return (
    <main className="auth-page">
      <div className="auth-layout">
        <aside className="auth-story">
          <a className="auth-brand" href="/">
            <BookOpen size={28} weight="bold" /> PXEL
          </a>
          <span className="auth-eyebrow">НА ВАШЕМ СЕРВЕРЕ</span>
          <h1>{title}</h1>
          <p>
            Совместные таблицы для вашей команды. Ваши данные и учётные записи находятся в этой
            установке PXEL.
          </p>
          <ul className="auth-points">
            <li>
              <ShieldCheck size={24} />
              <span>
                <strong>Управление внутри команды</strong>Мастер-аккаунт создаёт пользователей и
                имеет доступ ко всем книгам инстанса.
              </span>
            </li>
            <li>
              <UsersThree size={24} />
              <span>
                <strong>Вместе в одной сети</strong>Работайте с таблицами и выдавайте доступ
                коллегам. Открытой регистрации нет.
              </span>
            </li>
          </ul>
          <Legal />
        </aside>
        <section className="auth-card" aria-labelledby="auth-form-title">
          {setup && <p className="auth-progress">Первый запуск · шаг {step + 1} из 2</p>}
          <h2 id="auth-form-title">
            {setup
              ? step === 0
                ? 'Начнём с главного'
                : 'Создать мастер-аккаунт'
              : change
                ? 'Новый личный пароль'
                : 'Войти в PXEL'}
          </h2>
          {setup && step === 0 ? (
            <>
              <p>
                Первая учётная запись станет мастер-аккаунтом этой установки. Сохраните её данные в
                надёжном месте.
              </p>
              <ol className="auth-steps">
                <li>Подтвердите доступ одноразовым кодом установки от администратора сервера.</li>
                <li>Выберите логин и пароль мастер-аккаунта.</li>
                <li>Откройте «Пользователи» и создайте учётные записи команды.</li>
              </ol>
              <button className="auth-primary" type="button" onClick={() => setStep(1)}>
                Настроить пространство <ArrowRight size={18} />
              </button>
            </>
          ) : (
            <form
              ref={form}
              onSubmit={(event) => {
                void submit(event);
              }}
              aria-busy={busy}
            >
              <p>
                {setup
                  ? 'Код установки подтверждает доступ к серверу. Мастер-аккаунт можно создать только один раз.'
                  : change
                    ? 'Временный пароль выдал администратор. Доступ к книгам появится после смены пароля.'
                    : 'Используйте учётную запись, выданную администратором.'}
              </p>
              {setup && (
                <label>
                  Код установки
                  <input
                    name="bootstrap_token"
                    type="password"
                    required
                    minLength={64}
                    maxLength={64}
                    autoComplete="off"
                    spellCheck={false}
                    disabled={busy}
                  />
                </label>
              )}
              {!change && (
                <label>
                  Логин
                  <input
                    name="username"
                    required
                    minLength={3}
                    maxLength={32}
                    autoCapitalize="none"
                    autoComplete="username"
                    spellCheck={false}
                    disabled={busy}
                    aria-describedby="auth-username-help"
                  />
                  <small id="auth-username-help">3–32 символа: a–z, 0–9, . _ -</small>
                </label>
              )}
              {change && (
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
              )}
              <label>
                {change ? 'Новый пароль' : 'Пароль'}
                <input
                  name="password"
                  type="password"
                  required
                  minLength={setup || change ? 12 : 1}
                  maxLength={512}
                  autoComplete={setup || change ? 'new-password' : 'current-password'}
                  disabled={busy}
                  aria-describedby={setup || change ? 'auth-password-help' : undefined}
                />
                {(setup || change) && (
                  <small id="auth-password-help">
                    От 12 до 128 символов. Можно использовать пробелы и Unicode.
                  </small>
                )}
              </label>
              {(setup || change) && (
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
              )}
              {error && (
                <p role="alert" className="auth-error">
                  {error}
                </p>
              )}
              <button className="auth-primary" type="submit" disabled={busy}>
                {busy
                  ? 'Подождите…'
                  : setup
                    ? 'Создать мастер-аккаунт'
                    : change
                      ? 'Сохранить пароль'
                      : 'Войти'}
                <ArrowRight size={18} />
              </button>
              {change && (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void logout()
                      .then(() => readStatus())
                      .then(onAuthenticated)
                      .catch(() => setError('Не удалось завершить сессию.'))
                      .finally(() => setBusy(false));
                  }}
                >
                  Выйти
                </button>
              )}
            </form>
          )}
        </section>
      </div>
    </main>
  );
}
