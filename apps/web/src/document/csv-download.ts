export type CsvExportOptions = {
  sheetId: string;
  version: string;
  delimiter: 'comma' | 'semicolon' | 'tab';
  safeText: boolean;
  anonymized: boolean;
};
export async function downloadCsv(
  path: string,
  options: CsvExportOptions,
  signal: AbortSignal,
): Promise<Blob> {
  const query = new URLSearchParams({
    schema_version: '1',
    sheet_id: options.sheetId,
    document_version: options.version,
    delimiter: options.delimiter,
    safe_text: String(options.safeText),
    anonymized: String(options.anonymized),
  });
  const response = await fetch(path + '?' + query.toString(), {
    credentials: 'same-origin',
    cache: 'no-store',
    signal: AbortSignal.any([signal, AbortSignal.timeout(30000)]),
  });
  if (!response.ok)
    throw new Error(
      response.status === 409
        ? 'Версия таблицы изменилась. Повторите скачивание.'
        : response.status === 403
          ? 'CSV-экспорт запрещён политикой или права изменились.'
          : response.status === 422
            ? 'Не удалось выгрузить CSV: проверьте лимиты и результаты формул.'
            : 'Не удалось скачать CSV.',
    );
  if (response.headers.get('Content-Type')?.split(';')[0]?.trim().toLowerCase() !== 'text/csv') {
    await response.body?.cancel();
    throw new Error('Некорректный CSV-ответ.');
  }
  const reader = response.body?.getReader();
  if (!reader) throw new Error('Пустой CSV-ответ.');
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > 16 * 1024 * 1024) throw new Error('CSV превышает лимит 16 МиБ.');
      parts.push(new Uint8Array(value));
    }
    if (!size) throw new Error('Пустой CSV-ответ.');
    return new Blob(parts, { type: 'text/csv;charset=utf-8' });
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
