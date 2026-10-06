export type SharePermission = 'view' | 'comment' | 'edit';
export type ShareTtl = 'hour' | 'day' | 'week' | 'indefinite';
export type Share = {
  anonymized_at_creation?: boolean;
  link_id: string;
  document_id: string;
  permission: SharePermission;
  secret_kind: 'pin' | 'password';
  status: 'active' | 'revoked' | 'expired' | 'disabled';
  created_at: number;
  expires_at: number | null;
};
export type GuestSession = {
  linkId: string;
  documentId: string;
  actorId: string;
  permission: SharePermission;
  expiresAt: number;
  admittedHeadVersion: string;
};
export type EncryptionInfo = {
  encrypted: true;
  algorithm: 'XChaCha20-Poly1305';
  kek_rotated_at: number | null;
};
export type DisclosureStamp = {
  document_version: number;
  book_revision: number;
  link_revision: number;
};
export function decodeDisclosureStamp(value: unknown): DisclosureStamp {
  const item = record(value);
  if (
    !['document_version', 'book_revision', 'link_revision'].every(
      (key) =>
        typeof item[key] === 'number' &&
        Number.isSafeInteger(item[key]) &&
        (item[key] as number) >= 0,
    )
  )
    throw new SharingError('invalid_response');
  return item as DisclosureStamp;
}
export type Capabilities = {
  schema_version: 2 | 3;
  can_edit: boolean;
  can_comment: boolean;
  can_share: boolean;
  can_export: boolean;
  can_export_pxel?: boolean;
  can_export_csv?: boolean;
  can_copy: boolean;
  anonymized: boolean;
  disclosure_stamp: DisclosureStamp;
  is_guest: boolean;
};
export class SharingError extends Error {
  constructor(
    readonly code: 'unavailable' | 'locked' | 'owner_only' | 'network' | 'invalid_response',
    readonly retryAfter = 0,
  ) {
    super(
      code === 'locked'
        ? 'Слишком много попыток. Подождите перед повторным вводом.'
        : code === 'owner_only'
          ? 'Управлять доступом может только владелец таблицы.'
          : code === 'unavailable'
            ? 'Ссылка или код недоступны. Обратитесь к владельцу.'
            : 'Не удалось выполнить запрос. Повторите попытку.',
    );
  }
}
export const isUuid = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(value) &&
  value !== '00000000-0000-0000-0000-000000000000';
export const isUuid7 = (value: unknown): value is string =>
  isUuid(value) && value[14] === '7' && /^[89ab]$/i.test(value[19] ?? '');
const record = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new SharingError('invalid_response');
  return value as Record<string, unknown>;
};
const timestamp = (value: unknown): value is number =>
  Number.isSafeInteger(value) && typeof value === 'number' && value > 0;
export const isPermission = (value: unknown): value is SharePermission =>
  value === 'view' || value === 'comment' || value === 'edit';
export function decodeCapabilities(value: unknown): Capabilities {
  const item = record(value);
  if (item.schema_version !== 2 && item.schema_version !== 3)
    throw new SharingError('invalid_response');
  if (
    (item.schema_version === 3 && typeof item.can_export_csv !== 'boolean') ||
    (item.schema_version === 2 && item.can_export_csv !== undefined)
  )
    throw new SharingError('invalid_response');
  if (item.can_export_pxel !== undefined && typeof item.can_export_pxel !== 'boolean')
    throw new SharingError('invalid_response');
  decodeDisclosureStamp(item.disclosure_stamp);
  for (const key of [
    'can_edit',
    'can_comment',
    'can_share',
    'can_export',
    'can_copy',
    'anonymized',
    'is_guest',
  ])
    if (typeof item[key] !== 'boolean') throw new SharingError('invalid_response');
  return item as Capabilities;
}
export function decodeEncryption(value: unknown): EncryptionInfo {
  const item = record(value);
  if (
    item.encrypted !== true ||
    item.algorithm !== 'XChaCha20-Poly1305' ||
    (item.kek_rotated_at !== null && !timestamp(item.kek_rotated_at))
  )
    throw new SharingError('invalid_response');
  return item as EncryptionInfo;
}
export function decodeShare(value: unknown, documentId: string): Share {
  const item = record(value);
  if (
    !isUuid7(item.link_id) ||
    item.document_id !== documentId ||
    !isPermission(item.permission) ||
    (item.secret_kind !== 'pin' && item.secret_kind !== 'password') ||
    !['active', 'revoked', 'expired', 'disabled'].includes(String(item.status)) ||
    !timestamp(item.created_at) ||
    (item.expires_at !== null &&
      (!timestamp(item.expires_at) || item.expires_at <= item.created_at))
  )
    throw new SharingError('invalid_response');
  return item as Share;
}
export function decodeGuest(value: unknown, linkId: string): GuestSession {
  const item = record(value);
  if (
    !isUuid(item.document_id) ||
    !isUuid(item.actor_id) ||
    !isPermission(item.permission) ||
    !timestamp(item.expires_at) ||
    typeof item.admitted_head_version !== 'string' ||
    !/^(0|[1-9]\d{0,18})$/.test(item.admitted_head_version) ||
    BigInt(item.admitted_head_version) > 9223372036854775807n
  )
    throw new SharingError('invalid_response');
  return {
    linkId,
    documentId: item.document_id,
    actorId: item.actor_id,
    permission: item.permission,
    expiresAt: item.expires_at,
    admittedHeadVersion: item.admitted_head_version,
  };
}
export async function boundedJson(response: Response, limit = 1024 * 1024): Promise<unknown> {
  const reader = response.body?.getReader();
  if (!reader) throw new SharingError('invalid_response');
  let size = 0;
  const parts: Uint8Array[] = [];
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      size += chunk.value.byteLength;
      if (size > limit) throw new SharingError('invalid_response');
      parts.push(chunk.value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const part of parts) {
      bytes.set(part, offset);
      offset += part.byteLength;
    }
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)) as unknown;
  } catch {
    throw new SharingError('invalid_response');
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
async function request(path: string, init: RequestInit = {}, guest = false): Promise<unknown> {
  let response;
  try {
    response = await fetch(path, {
      ...init,
      credentials: 'same-origin',
      signal: init.signal
        ? AbortSignal.any([init.signal, AbortSignal.timeout(10_000)])
        : AbortSignal.timeout(10_000),
      cache: 'no-store',
    });
  } catch {
    throw new SharingError('network');
  }
  if (!response.ok) {
    if (guest && response.status === 429) {
      const error = record(await boundedJson(response, 4096));
      if (
        error.code === 'share_locked' &&
        Number.isSafeInteger(error.retry_after_seconds) &&
        typeof error.retry_after_seconds === 'number' &&
        error.retry_after_seconds > 0 &&
        error.retry_after_seconds <= 86400
      )
        throw new SharingError('locked', error.retry_after_seconds);
    }
    throw new SharingError(
      guest ? 'unavailable' : response.status === 403 ? 'owner_only' : 'network',
    );
  }
  return response.status === 204 ? null : boundedJson(response);
}
const json = (body: unknown, method = 'POST'): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
});
const ownerPath = (id: string) => `/api/documents/${encodeURIComponent(id)}/shares`;
export const guestPath = (id: string) => `/api/shared/${encodeURIComponent(id)}`;
export async function listShares(documentId: string, signal?: AbortSignal): Promise<Share[]> {
  const item = record(await request(ownerPath(documentId), { signal: signal ?? null }));
  if (!Array.isArray(item.shares) || item.shares.length > 100)
    throw new SharingError('invalid_response');
  const shares = item.shares.map((value) => decodeShare(value, documentId));
  if (new Set(shares.map((share) => share.link_id)).size !== shares.length)
    throw new SharingError('invalid_response');
  return shares;
}
export async function createShare(
  documentId: string,
  body: {
    request_id: string;
    permission: SharePermission;
    ttl: ShareTtl;
    confirm_indefinite: boolean;
    anonymized?: boolean;
    pin?: string;
    password?: string;
  },
): Promise<{ share: Share; url?: string }> {
  const result = record(await request(ownerPath(documentId), json(body)));
  const share = decodeShare(result.share, documentId);
  if (result.token === undefined && result.url === undefined) return { share };
  if (
    typeof result.token !== 'string' ||
    !/^[0-9a-f]{64}$/.test(result.token) ||
    result.url !== `/share/${share.link_id}#${result.token}`
  )
    throw new SharingError('invalid_response');
  return { share, url: new URL(result.url, location.origin).href };
}
export async function revokeShare(
  documentId: string,
  linkId: string,
  requestId: string,
): Promise<void> {
  await request(
    `${ownerPath(documentId)}/${encodeURIComponent(linkId)}`,
    json({ request_id: requestId }, 'DELETE'),
  );
}
export async function guestSession(linkId: string, signal?: AbortSignal): Promise<GuestSession> {
  return decodeGuest(
    await request(`${guestPath(linkId)}/session`, { signal: signal ?? null }, true),
    linkId,
  );
}
export async function unlockShare(
  linkId: string,
  token: string,
  secret: string,
): Promise<GuestSession> {
  return decodeGuest(
    await request(`${guestPath(linkId)}/session`, json({ token, secret }), true),
    linkId,
  );
}
export type Comment = {
  comment_id: string;
  sheet_id: string;
  row_id: string;
  column_id: string;
  author_id: string;
  text: string;
  can_delete: boolean;
};
export type Comments = { revision: string; comments: Comment[]; next_cursor?: string };
export function decodeComments(value: unknown): Comments {
  const item = record(value);
  if (
    typeof item.revision !== 'string' ||
    !/^(0|[1-9]\d{0,18})$/.test(item.revision) ||
    !Array.isArray(item.comments) ||
    item.comments.length > 100 ||
    (item.next_cursor !== undefined &&
      (typeof item.next_cursor !== 'string' || item.next_cursor.length > 256))
  )
    throw new SharingError('invalid_response');
  const comments = item.comments.map((value) => {
    const comment = record(value);
    if (
      !['comment_id', 'sheet_id', 'row_id', 'column_id', 'author_id'].every((key) =>
        isUuid(comment[key]),
      ) ||
      typeof comment.text !== 'string' ||
      [...comment.text].length < 1 ||
      new TextEncoder().encode(comment.text).length > 4096 ||
      typeof comment.can_delete !== 'boolean'
    )
      throw new SharingError('invalid_response');
    return comment as Comment;
  });
  if (new Set(comments.map((comment) => comment.comment_id)).size !== comments.length)
    throw new SharingError('invalid_response');
  return {
    revision: item.revision,
    comments,
    ...(typeof item.next_cursor === 'string' ? { next_cursor: item.next_cursor } : {}),
  };
}
export async function commentsRequest(
  path: string,
  body?: unknown,
  method = 'POST',
  signal?: AbortSignal,
): Promise<Comments> {
  return decodeComments(
    await request(
      path,
      body === undefined
        ? { signal: signal ?? null }
        : { ...json(body, method), signal: signal ?? null },
      path.startsWith('/api/shared/'),
    ),
  );
}
export type OwnerNotification = {
  notification_id: string;
  link_id: string;
  document_id: string;
  kind: 'share_disabled';
  created_at: number;
};
export async function notifications(signal?: AbortSignal): Promise<OwnerNotification[]> {
  const item = record(await request('/api/notifications', { signal: signal ?? null }));
  if (!Array.isArray(item.notifications) || item.notifications.length > 100)
    throw new SharingError('invalid_response');
  return item.notifications.map((value) => {
    const notification = record(value);
    if (
      !['notification_id', 'link_id', 'document_id'].every((key) => isUuid(notification[key])) ||
      notification.kind !== 'share_disabled' ||
      !timestamp(notification.created_at)
    )
      throw new SharingError('invalid_response');
    return notification as OwnerNotification;
  });
}
