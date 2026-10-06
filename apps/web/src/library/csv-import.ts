import { boundedJson, isUuid7 } from '../sharing/api';
import { uuidV7 } from '../document/store';

export type ColumnType = 'text' | 'number' | 'boolean';
export type CsvPayload = {
  schema_version: 1;
  file_base64: string;
  title: string;
  delimiter: 'comma' | 'semicolon' | 'tab';
  column_types: ColumnType[];
  header: boolean;
  document_id: string;
  request_id: string;
  preview_digest?: string | undefined;
};
export type CsvPreview = {
  schema_version: 1;
  title: string;
  rows: number;
  columns: number;
  column_types: ColumnType[];
  sample: string[][];
  preview_digest: string;
};
export function decodeCsvPreview(value: unknown): CsvPreview {
  const v = value as CsvPreview;
  if (
    !v ||
    v.schema_version !== 1 ||
    typeof v.title !== 'string' ||
    [...v.title].length > 256 ||
    !Number.isSafeInteger(v.rows) ||
    v.rows < 1 ||
    v.rows > 100000 ||
    !Number.isSafeInteger(v.columns) ||
    v.columns < 1 ||
    v.columns > 1000 ||
    v.rows * v.columns > 100000 ||
    !Array.isArray(v.column_types) ||
    v.column_types.length !== v.columns ||
    !v.column_types.every((t) => ['text', 'number', 'boolean'].includes(t)) ||
    typeof v.preview_digest !== 'string' ||
    !/^[0-9a-f]{64}$/.test(v.preview_digest) ||
    !Array.isArray(v.sample) ||
    v.sample.length !== Math.min(v.rows, 20) ||
    !v.sample.every(
      (row) =>
        Array.isArray(row) &&
        row.length === Math.min(v.columns, 20) &&
        row.every((cell) => typeof cell === 'string' && [...cell].length <= 256),
    )
  )
    throw new Error('Некорректный предпросмотр CSV.');
  return v;
}
export async function readCsvFile(file: File): Promise<CsvPayload> {
  if (!file.size || file.size > 16 * 1024 * 1024)
    throw new Error('Выберите CSV-файл размером до 16 МиБ.');
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i += 8192)
    binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return {
    schema_version: 1,
    file_base64: btoa(binary),
    title: [...file.name.replace(/\.csv$/i, '')].slice(0, 256).join('').trim() || 'Импорт CSV',
    delimiter: 'comma',
    column_types: [],
    header: false,
    document_id: uuidV7(),
    request_id: uuidV7(),
  };
}
const messages: Record<string, string> = {
  csv_syntax: 'CSV содержит неправильные кавычки, строки разной ширины или недопустимые символы.',
  csv_encoding: 'CSV должен быть сохранён в кодировке UTF-8.',
  csv_type: 'Значения не соответствуют выбранному типу столбца.',
  csv_limit: 'CSV превышает лимиты: 16 МиБ, 100 000 полей, 1000 столбцов, 32 КиБ на значение.',
  csv_options: 'Проверьте название и параметры CSV.',
  csv_request_timeout: 'CSV не успел загрузиться. Повторите попытку.',
  csv_preview_changed: 'Файл или параметры изменились. Выполните предпросмотр заново.',
  csv_preview_required: 'Сначала выполните предпросмотр CSV.',
  import_busy: 'Другой импорт или экспорт ещё выполняется. Повторите попытку.',
};
async function request(payload: CsvPayload, publish: boolean, signal: AbortSignal) {
  const response = await fetch('/api/imports/csv' + (publish ? '' : '/preview'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
    signal: AbortSignal.any([signal, AbortSignal.timeout(60000)]),
  });
  if (!response.ok) {
    const error = (await boundedJson(response, 8192).catch(() => ({}))) as { code?: string };
    throw new Error(
      messages[error.code ?? ''] ?? 'Не удалось импортировать CSV. Повторите попытку.',
    );
  }
  return boundedJson(response, publish ? 8192 : 512 * 1024);
}
export async function previewCsv(payload: CsvPayload, signal: AbortSignal) {
  return decodeCsvPreview(await request(payload, false, signal));
}
export async function publishCsv(payload: CsvPayload, signal: AbortSignal) {
  const result = (await request(payload, true, signal)) as {
    document_id?: string;
    version?: string;
  };
  if (
    !result ||
    result.document_id !== payload.document_id ||
    !isUuid7(result.document_id) ||
    typeof result.version !== 'string' ||
    !/^[1-9]\d*$/.test(result.version)
  )
    throw new Error('Некорректное подтверждение импорта CSV.');
  return result.document_id;
}
