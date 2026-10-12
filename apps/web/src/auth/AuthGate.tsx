import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { readStatus, type AuthStatus } from './api';
import { LoginScreen } from './LoginScreen';
import { onAccountChanged } from './sessionEvents';
import './auth.css';

const AuthContext = createContext<AuthStatus>({ schema_version: 1, mode: 'unavailable' });
export function useAccount() {
  return useContext(AuthContext);
}

export function AuthGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>();
  const [error, setError] = useState('');
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    const refresh = () => setGeneration((value) => value + 1);
    const stop = onAccountChanged(refresh);
    window.addEventListener('focus', refresh);
    return () => {
      stop();
      window.removeEventListener('focus', refresh);
    };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    void readStatus(controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) {
          setStatus(value);
          setError('');
        }
      })
      .catch(() => {
        if (!controller.signal.aborted) setError('Не удалось связаться с сервером.');
      });
    return () => controller.abort();
  }, [generation]);
  if (error)
    return (
      <main className="auth-page">
        <section className="auth-card">
          <h1>PXEL недоступен</h1>
          <p role="alert">{error}</p>
          <button
            type="button"
            onClick={() => {
              setError('');
              setGeneration((value) => value + 1);
            }}
          >
            Повторить
          </button>
        </section>
      </main>
    );
  if (!status)
    return (
      <main className="auth-page">
        <p role="status">Подключение к PXEL…</p>
      </main>
    );
  if (status.mode === 'development' || status.mode === 'authenticated')
    return (
      <AuthContext.Provider key={status.account?.user_id ?? status.mode} value={status}>
        {children}
      </AuthContext.Provider>
    );
  return <LoginScreen key={status.mode} status={status} onAuthenticated={setStatus} />;
}
