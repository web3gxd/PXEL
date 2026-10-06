import { boundedJson } from '../sharing/api';
import { uuidV7 } from './store';
export const MAX_PXEL_FILE = 32 * 1024 * 1024 + 72;
export type Composition = {
  schema_version: 1;
  title: string;
  sheets: { name: string; rows: number; columns: number; cells: number }[];
  formulas: number;
  anonymized: boolean;
  features: string[];
  omitted_filters: number;
};
export function validPassword(password: string) {
  return [...password].length >= 12 && [...password].length <= 128;
}
export function decodeComposition(value: unknown): Composition {
  if (!value || typeof value !== 'object') throw new Error('Некорректный состав файла.');
  const v = value as Composition;
  const count = (n: unknown): n is number =>
    typeof n === 'number' && Number.isSafeInteger(n) && n >= 0;
  if (
    v.schema_version !== 1 ||
    typeof v.title !== 'string' ||
    typeof v.anonymized !== 'boolean' ||
    !count(v.formulas) ||
    !count(v.omitted_filters) ||
    !Array.isArray(v.features) ||
    !v.features.every((f) => typeof f === 'string') ||
    !Array.isArray(v.sheets) ||
    v.sheets.length < 1 ||
    v.sheets.length > 64 ||
    !v.sheets.every(
      (s) =>
        s &&
        typeof s.name === 'string' &&
        count(s.rows) &&
        s.rows > 0 &&
        count(s.columns) &&
        s.columns > 0 &&
        count(s.cells),
    )
  )
    throw new Error('Неподдерживаемый состав файла.');
  return v;
}
const messages: Record<string, string> = {
  pxel_authentication: 'Неверный пароль или файл повреждён.',
  pxel_password_length: 'Пароль должен содержать 12–128 символов.',
  pxel_version: 'Версия или возможности файла не поддерживаются.',
  pxel_invalid: 'Файл не прошёл полную проверку.',
  pxel_limit: 'Файл превышает поддерживаемые лимиты.',
  disclosure_denied: 'Этот формат запрещён политикой выдачи.',
  disclosure_changed: 'Права или версия изменились. Повторите экспорт.',
};
export async function fileError(response: Response): Promise<Error> {
  try {
    const value = (await boundedJson(response, 8192)) as { code?: string };
    return new Error(
      messages[value.code ?? ''] ?? 'Не удалось обработать файл. Повторите попытку.',
    );
  } catch {
    return new Error('Не удалось обработать файл.');
  }
}
export type ImportPayload = {
  schema_version: 1;
  password: string;
  file_base64: string;
  document_id: string;
  request_id: string;
};
export async function readFile(file: File): Promise<Omit<ImportPayload, 'password'>> {
  if (file.size < 72 || file.size > MAX_PXEL_FILE)
    throw new Error('Выберите файл .pxel размером до 32 МиБ.');
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i += 8192)
    binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return {
    schema_version: 1,
    file_base64: btoa(binary),
    document_id: uuidV7(),
    request_id: uuidV7(),
  };
}
export async function previewFile(payload: ImportPayload): Promise<Composition> {
  const response = await fetch('/api/imports/pxel/preview', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) throw await fileError(response);
  return decodeComposition(await boundedJson(response, 65536));
}
export async function importFile(payload: ImportPayload): Promise<string> {
  const response = await fetch('/api/imports/pxel', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) throw await fileError(response);
  const value = (await boundedJson(response, 8192)) as { document_id?: string };
  if (value.document_id !== payload.document_id) throw new Error('Некорректный ответ импорта.');
  return value.document_id;
}
