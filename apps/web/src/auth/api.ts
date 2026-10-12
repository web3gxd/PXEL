import { boundedJson } from '../sharing/api';
import { refreshAccount } from './sessionEvents';

export type Account = {
  user_id: string;
  username: string;
  is_master: boolean;
  enabled: boolean;
  must_change_password: boolean;
};
export type AuthStatus = {
  schema_version: 1;
  mode: 'development' | 'setup' | 'login' | 'authenticated' | 'password_change' | 'unavailable';
  account?: Account;
};
const modes = ['development', 'setup', 'login', 'authenticated', 'password_change', 'unavailable'];
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Некорректный ответ сервера.');
  return value as Record<string, unknown>;
}
export function validUsername(value: string): boolean {
  return /^[a-z0-9_.-]{3,32}$/.test(value);
}
export function validPassword(value: string): boolean {
  const length = [...value].length;
  return length >= 12 && length <= 128 && new TextEncoder().encode(value).length <= 512;
}
function decodeAccount(value: unknown): Account {
  const row = object(value);
  if (
    typeof row.user_id !== 'string' ||
    !uuid.test(row.user_id) ||
    typeof row.username !== 'string' ||
    !validUsername(row.username) ||
    typeof row.is_master !== 'boolean' ||
    typeof row.enabled !== 'boolean' ||
    typeof row.must_change_password !== 'boolean'
  )
    throw new Error('Некорректная учётная запись.');
  return {
    user_id: row.user_id,
    username: row.username,
    is_master: row.is_master,
    enabled: row.enabled,
    must_change_password: row.must_change_password,
  };
}
export function decodeStatus(value: unknown): AuthStatus {
  const row = object(value);
  if (row.schema_version !== 1 || typeof row.mode !== 'string' || !modes.includes(row.mode))
    throw new Error('Версия входа не поддерживается. Обновите страницу.');
  const mode = row.mode as AuthStatus['mode'];
  const account =
    row.account === undefined || row.account === null ? undefined : decodeAccount(row.account);
  if (mode === 'authenticated' || mode === 'password_change') {
    if (!account?.enabled || account.must_change_password !== (mode === 'password_change'))
      throw new Error('Некорректное состояние входа.');
  } else if (account) throw new Error('Некорректное состояние входа.');
  return account ? { schema_version: 1, mode, account } : { schema_version: 1, mode };
}
export function decodeAccounts(value: unknown): Account[] {
  const row = object(value);
  if (row.schema_version !== 1 || !Array.isArray(row.accounts) || row.accounts.length > 1000)
    throw new Error('Некорректный список пользователей.');
  const accounts = row.accounts.map(decodeAccount);
  if (
    new Set(accounts.map((a) => a.user_id)).size !== accounts.length ||
    new Set(accounts.map((a) => a.username)).size !== accounts.length
  )
    throw new Error('Повторяющиеся учётные записи.');
  return accounts;
}
async function request(path: string, body?: unknown, signal?: AbortSignal): Promise<Response> {
  const response = await fetch(path, {
    method: body === undefined ? 'GET' : 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    signal: signal
      ? AbortSignal.any([signal, AbortSignal.timeout(20_000)])
      : AbortSignal.timeout(20_000),
    ...(body === undefined
      ? {}
      : {
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        }),
  });
  if (!response.ok) {
    if (response.status === 401 && path.startsWith('/api/admin/')) refreshAccount();
    // Never reflect server text, credential inputs or request bodies into UI errors.
    const messages: Record<number, string> = {
      400: 'Проверьте логин, пароль и заполнение полей.',
      401: 'Не удалось войти. Проверьте данные или повторите позже.',
      403: 'Действие недоступно для этой учётной записи или адреса сервера.',
      409: 'Состояние изменилось или логин уже занят. Обновите страницу.',
      429: 'Слишком много попыток. Подождите и попробуйте снова.',
      503: 'Сервис временно недоступен. Обратитесь к администратору.',
    };
    throw new Error(messages[response.status] ?? 'Не удалось выполнить действие.');
  }
  return response;
}
export async function readStatus(signal?: AbortSignal): Promise<AuthStatus> {
  return decodeStatus(
    await boundedJson(await request('/api/auth/status', undefined, signal), 4096),
  );
}
export async function authenticate(
  action: 'setup' | 'login' | 'password',
  input: Record<string, string>,
): Promise<AuthStatus> {
  return decodeStatus(
    await boundedJson(await request(`/api/auth/${action}`, { schema_version: 1, ...input }), 4096),
  );
}
export async function logout(): Promise<void> {
  await request('/api/auth/logout', { schema_version: 1 });
}
export async function readAccounts(signal?: AbortSignal): Promise<Account[]> {
  return decodeAccounts(
    await boundedJson(await request('/api/admin/accounts', undefined, signal), 256 * 1024),
  );
}
export async function createAccount(username: string, password: string): Promise<void> {
  await request('/api/admin/accounts', { schema_version: 1, username, password });
}
export async function changeAccount(
  userId: string,
  action: 'disable' | 'enable' | 'reset_password',
  password?: string,
): Promise<void> {
  if (!uuid.test(userId)) throw new Error('Некорректная учётная запись.');
  await request(`/api/admin/accounts/${userId}`, {
    schema_version: 1,
    action,
    ...(password === undefined ? {} : { password }),
  });
}
