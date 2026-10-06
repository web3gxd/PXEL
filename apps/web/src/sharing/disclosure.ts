import { boundedJson, decodeDisclosureStamp, isUuid, type DisclosureStamp } from './api';
export type DisclosureRules = {
  allow_copy: boolean;
  export_formats: ('xlsx' | 'pxel')[];
  allow_participant_export: boolean;
  require_anonymization: boolean;
};
export type DisclosurePolicy = {
  schema_version: 1;
  revision: number;
  access_epoch: number;
  rules: DisclosureRules;
};
const object = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Некорректный ответ политики.');
  return value as Record<string, unknown>;
};
const integer = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
export function decodePolicy(value: unknown): DisclosurePolicy {
  const item = object(value),
    rules = object(item.rules);
  if (
    item.schema_version !== 1 ||
    !integer(item.revision) ||
    !integer(item.access_epoch) ||
    !['allow_copy', 'allow_participant_export', 'require_anonymization'].every(
      (key) => typeof rules[key] === 'boolean',
    ) ||
    !Array.isArray(rules.export_formats) ||
    rules.export_formats.length > 2 ||
    new Set(rules.export_formats).size !== rules.export_formats.length ||
    !rules.export_formats.every((format) => format === 'xlsx' || format === 'pxel')
  )
    throw new Error('Неподдерживаемая политика выдачи.');
  return item as DisclosurePolicy;
}
export async function authorizeClipboard(
  path: string,
  action: 'copy' | 'cut' | 'copy_anonymized',
  stamp: DisclosureStamp,
  text?: string,
): Promise<{ anonymized: boolean; projection?: unknown; text?: string }> {
  const response = await fetch(path, {
    method: 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      schema_version: 1,
      action,
      stamp,
      ...(text === undefined ? {} : { text }),
    }),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok)
    throw new Error(
      response.status === 403
        ? 'Копирование запрещено политикой или права изменились.'
        : 'Не удалось записать аудит копирования. Повторите попытку.',
    );
  const item = object(await boundedJson(response, 16 * 1024 * 1024));
  const current = decodeDisclosureStamp(item.stamp);
  if (
    item.schema_version !== 1 ||
    item.allowed !== true ||
    item.reason !== 'allowed' ||
    current.document_version !== stamp.document_version ||
    current.book_revision !== stamp.book_revision ||
    current.link_revision !== stamp.link_revision
  )
    throw new Error('Права или версия изменились. Повторите копирование.');
  if (item.anonymized !== undefined && typeof item.anonymized !== 'boolean')
    throw new Error('Некорректный режим выдачи.');
  if (item.anonymized === true && item.projection === undefined)
    throw new Error('Сервер не предоставил безопасную копию.');
  if (item.text !== undefined && typeof item.text !== 'string')
    throw new Error('Некорректный текст копии.');
  return {
    anonymized: item.anonymized === true,
    ...(item.projection === undefined ? {} : { projection: item.projection }),
    ...(typeof item.text === 'string' ? { text: item.text } : {}),
  };
}
export async function readPolicy(path: string, signal?: AbortSignal): Promise<DisclosurePolicy> {
  const response = await fetch(path, {
    cache: 'no-store',
    signal: signal ?? AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error('Не удалось загрузить политику выдачи.');
  return decodePolicy(await boundedJson(response, 8192));
}
export async function savePolicy(
  path: string,
  policy: DisclosurePolicy,
  rules: DisclosureRules,
): Promise<DisclosurePolicy> {
  const response = await fetch(path, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ schema_version: 1, expected_revision: policy.revision, rules }),
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok)
    throw new Error(
      response.status === 409
        ? 'Политика уже изменена. Откройте окно заново.'
        : 'Не удалось сохранить политику.',
    );
  return decodePolicy(await boundedJson(response, 8192));
}

export type DisclosureAuditEvent = {
  event_id: string;
  actor_id: string;
  link_id: string | null;
  created_at: number;
  document_version: number;
  book_revision: number;
  link_revision: number;
  action: 'copy' | 'cut' | 'export';
  format: 'clipboard' | 'xlsx' | 'pxel';
  allowed: boolean;
  reason: string;
};
export type DisclosureAuditPage = {
  schema_version: 1;
  events: DisclosureAuditEvent[];
  next_cursor: string | null;
};
export function decodeAuditPage(value: unknown): DisclosureAuditPage {
  const item = object(value);
  if (
    item.schema_version !== 1 ||
    !Array.isArray(item.events) ||
    item.events.length > 100 ||
    (item.next_cursor !== null && !isUuid(item.next_cursor))
  )
    throw new Error('Некорректный журнал выдачи.');
  const events = item.events.map((value) => {
    const event = object(value);
    if (
      !isUuid(event.event_id) ||
      !isUuid(event.actor_id) ||
      (event.link_id !== null && !isUuid(event.link_id)) ||
      !['created_at', 'document_version', 'book_revision', 'link_revision'].every((key) =>
        integer(event[key]),
      ) ||
      !['copy', 'cut', 'export'].includes(String(event.action)) ||
      !['clipboard', 'xlsx', 'pxel'].includes(String(event.format)) ||
      typeof event.allowed !== 'boolean' ||
      ![
        'allowed',
        'role_denied',
        'copy_denied',
        'format_denied',
        'anonymization_required',
        'policy_changed',
        'version_changed',
        'privacy_analysis_failed',
      ].includes(String(event.reason))
    )
      throw new Error('Некорректный журнал выдачи.');
    return event as DisclosureAuditEvent;
  });
  if (new Set(events.map((event) => event.event_id)).size !== events.length)
    throw new Error('Повторная запись журнала выдачи.');
  return { schema_version: 1, events, next_cursor: item.next_cursor as string | null };
}
