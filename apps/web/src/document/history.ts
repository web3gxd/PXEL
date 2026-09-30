export type HistoryReference = { operation_id: string; committed_version: string };
export type DiscoveredHistory = {
  schema_version: 1;
  document_id: string;
  actor_id: string;
  through_version: string;
  earliest_scanned_version: string;
  undo: HistoryReference[];
  redo: HistoryReference[];
  truncated: boolean;
  truncation_reason: null | 'version_window' | 'byte_budget';
};
const maximumBytes = 64 * 1024;
const invalid = () => new Error('Ответ сервера не соответствует формату истории правок');
const uuid = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
const version = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^(0|[1-9]\d{0,18})$/.test(value) &&
  BigInt(value) <= 9223372036854775807n;
function record(input: unknown, keys: string[]): Record<string, unknown> {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw invalid();
  if (Object.keys(input).length !== keys.length || keys.some((key) => !Object.hasOwn(input, key)))
    throw invalid();
  return input as Record<string, unknown>;
}
export function decodeHistory(input: unknown): DiscoveredHistory {
  const value = record(input, [
    'schema_version',
    'document_id',
    'actor_id',
    'through_version',
    'earliest_scanned_version',
    'undo',
    'redo',
    'truncated',
    'truncation_reason',
  ]);
  if (
    value.schema_version !== 1 ||
    !uuid(value.document_id) ||
    !uuid(value.actor_id) ||
    !version(value.through_version) ||
    !version(value.earliest_scanned_version) ||
    typeof value.truncated !== 'boolean' ||
    (value.truncation_reason !== null &&
      value.truncation_reason !== 'version_window' &&
      value.truncation_reason !== 'byte_budget') ||
    value.truncated !== (value.truncation_reason !== null)
  )
    throw invalid();
  const head = BigInt(value.through_version),
    earliest = BigInt(value.earliest_scanned_version);
  if (earliest > head || (head > 0n && earliest === 0n) || head - earliest > 10000n)
    throw invalid();
  if (!value.truncated && earliest > 1n) throw invalid();
  if (value.truncated && earliest <= 1n) throw invalid();
  if (
    value.truncation_reason === 'version_window' &&
    (earliest <= 1n || head - earliest !== 10000n)
  )
    throw invalid();
  const ids = new Set<string>(),
    versions = new Set<string>();
  for (const stack of [value.undo, value.redo]) {
    if (!Array.isArray(stack) || stack.length > 100) throw invalid();
    let previous = 0n;
    for (const item of stack) {
      const entry = record(item, ['operation_id', 'committed_version']);
      if (!uuid(entry.operation_id) || !version(entry.committed_version)) throw invalid();
      const committed = BigInt(entry.committed_version);
      if (
        committed <= previous ||
        committed < earliest ||
        committed > head ||
        ids.has(entry.operation_id) ||
        versions.has(entry.committed_version)
      )
        throw invalid();
      ids.add(entry.operation_id);
      versions.add(entry.committed_version);
      previous = committed;
    }
  }
  return value as DiscoveredHistory;
}

export async function readHistory(response: Response): Promise<DiscoveredHistory> {
  const length = response.headers.get('content-length');
  if (length !== null && (!/^\d+$/.test(length) || Number(length) > maximumBytes)) throw invalid();
  const reader = response.body?.getReader();
  if (!reader) throw invalid();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let bytes = 0,
    text = '';
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      bytes += chunk.value.byteLength;
      if (bytes > maximumBytes) {
        await reader.cancel();
        throw invalid();
      }
      text += decoder.decode(chunk.value, { stream: true });
    }
    text += decoder.decode();
  } finally {
    reader.releaseLock();
  }
  return decodeHistory(JSON.parse(text) as unknown);
}
