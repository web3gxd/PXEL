import { boundedJson, isUuid7, decodeDisclosureStamp, type DisclosureStamp } from '../sharing/api';
import { authorizeClipboard } from '../sharing/disclosure';
import { uuidV7 } from '../document/store';
import { refreshAccount } from '../auth/sessionEvents';
export type Metadata = { schema_version: 1; revision: number; title: string };
export type Preference = {
  schema_version: 1;
  revision: number;
  favorite: boolean;
  opened_at: number;
};
export type Layout = { schema_version: 1; revision: number; layout: 'cards' | 'list' };
export type Entry = {
  document_id: string;
  version: number;
  role: 'owner' | 'editor' | 'viewer';
  created_at: number;
  metadata: Metadata;
  preference: Preference;
  disclosure_stamp: DisclosureStamp;
};
export type Page = {
  schema_version: 1;
  documents: Entry[];
  next_cursor: string | null;
  layout: Layout;
};
const integer = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
export function decodeMetadata(value: unknown): Metadata {
  const v = value as Metadata;
  if (
    !v ||
    v.schema_version !== 1 ||
    !integer(v.revision) ||
    typeof v.title !== 'string' ||
    v.title.includes('\0') ||
    [...v.title].length > 256 ||
    !v.title.trim()
  )
    throw new Error('Некорректное название книги.');
  return v;
}
export function decodePreference(value: unknown): Preference {
  const v = value as Preference;
  if (
    !v ||
    v.schema_version !== 1 ||
    !integer(v.revision) ||
    !integer(v.opened_at) ||
    typeof v.favorite !== 'boolean'
  )
    throw new Error('Некорректные настройки книги.');
  return v;
}
export function decodeLayout(value: unknown): Layout {
  const v = value as Layout;
  if (!v || v.schema_version !== 1 || !integer(v.revision) || !['cards', 'list'].includes(v.layout))
    throw new Error('Некорректный вид библиотеки.');
  return v;
}
export function decodePage(value: unknown): Page {
  const v = value as Page;
  if (
    !v ||
    v.schema_version !== 1 ||
    !Array.isArray(v.documents) ||
    v.documents.length > 100 ||
    (v.next_cursor !== null && !isUuid7(v.next_cursor))
  )
    throw new Error('Некорректная библиотека.');
  decodeLayout(v.layout);
  const seen = new Set<string>();
  for (const e of v.documents) {
    if (
      !e ||
      !isUuid7(e.document_id) ||
      seen.has(e.document_id) ||
      !integer(e.version) ||
      !integer(e.created_at) ||
      !['owner', 'editor', 'viewer'].includes(e.role)
    )
      throw new Error('Некорректная книга в библиотеке.');
    seen.add(e.document_id);
    decodeMetadata(e.metadata);
    decodePreference(e.preference);
    decodeDisclosureStamp(e.disclosure_stamp);
  }
  return v;
}
async function request(
  path: string,
  method = 'GET',
  body?: unknown,
  signal?: AbortSignal,
): Promise<unknown> {
  const response = await fetch(path, {
    method,
    cache: 'no-store',
    headers: { 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: signal ?? AbortSignal.timeout(20_000),
  });
  if (response.status === 401) refreshAccount();
  if (!response.ok)
    throw new Error(
      response.status === 409
        ? 'Настройки уже изменены. Обновите библиотеку.'
        : response.status === 401
          ? 'Требуется авторизация.'
          : 'Не удалось выполнить запрос библиотеки.',
    );
  return boundedJson(response, 1024 * 1024);
}
export async function readPage(cursor: string | null, signal?: AbortSignal) {
  return decodePage(
    await request(
      '/api/documents?schema_version=1' + (cursor ? '&before=' + encodeURIComponent(cursor) : ''),
      'GET',
      undefined,
      signal,
    ),
  );
}
export async function favorite(entry: Entry, value: boolean) {
  return decodePreference(
    await request(`/api/documents/${entry.document_id}/favorite`, 'PUT', {
      schema_version: 1,
      expected_revision: entry.preference.revision,
      favorite: value,
    }),
  );
}
export async function setLayout(layout: Layout, value: Layout['layout']) {
  return decodeLayout(
    await request('/api/library/layout', 'PUT', {
      schema_version: 1,
      expected_revision: layout.revision,
      layout: value,
    }),
  );
}
export async function readMetadata(id: string, signal?: AbortSignal) {
  return decodeMetadata(await request(`/api/documents/${id}/metadata`, 'GET', undefined, signal));
}
export async function rename(id: string, metadata: Metadata, title: string) {
  return decodeMetadata(
    await request(`/api/documents/${id}/metadata`, 'PUT', {
      schema_version: 1,
      expected_revision: metadata.revision,
      title,
    }),
  );
}
export async function opened(id: string) {
  await request(`/api/documents/${id}/opened`, 'POST', { schema_version: 1 });
}
export async function createBook(
  template?: 'budget' | 'tasks' | 'sales',
  identity = { document_id: uuidV7(), request_id: uuidV7() },
): Promise<string> {
  const id = identity.document_id;
  const value = (await request(
    template ? '/api/library/templates' : '/api/documents',
    'POST',
    template
      ? { schema_version: 1, ...identity, template }
      : { document_id: id, operation_id: identity.request_id },
  )) as { document_id?: string };
  if (value.document_id !== id) throw new Error('Не удалось создать книгу.');
  return id;
}
export function visible(
  entries: Entry[],
  query: string,
  section: 'all' | 'recent' | 'favorites' | 'shared',
) {
  const search = query.trim().toLocaleLowerCase('ru');
  return entries
    .filter(
      (e) =>
        (!search || e.metadata.title.toLocaleLowerCase('ru').includes(search)) &&
        (section !== 'favorites' || e.preference.favorite) &&
        (section !== 'shared' || e.role !== 'owner') &&
        (section !== 'recent' || e.preference.opened_at > 0),
    )
    .toSorted((a, b) =>
      section === 'recent'
        ? b.preference.opened_at - a.preference.opened_at
        : b.created_at - a.created_at || b.document_id.localeCompare(a.document_id),
    );
}
export function mergeEntries(previous: Entry[], incoming: Entry[], append: boolean) {
  const old = new Map(previous.map((entry) => [entry.document_id, entry]));
  const merged = incoming.map((entry) => {
    const before = old.get(entry.document_id);
    return before
      ? {
          ...entry,
          metadata:
            before.metadata.revision > entry.metadata.revision ? before.metadata : entry.metadata,
          preference:
            before.preference.revision > entry.preference.revision
              ? before.preference
              : entry.preference,
        }
      : entry;
  });
  const ids = new Set(incoming.map((entry) => entry.document_id));
  return append ? [...previous.filter((entry) => !ids.has(entry.document_id)), ...merged] : merged;
}
export async function copyTitle(entry: Entry, text: string) {
  const authorized = async () => {
    const receipt = await authorizeClipboard(
      `/api/documents/${entry.document_id}/clipboard/authorize`,
      'copy',
      entry.disclosure_stamp,
      text,
    );
    if (receipt.anonymized && receipt.text === undefined)
      throw new Error('Безопасный текст не предоставлен.');
    return receipt.text ?? text;
  };
  if (typeof ClipboardItem !== 'undefined' && navigator.clipboard?.write) {
    await navigator.clipboard.write([
      new ClipboardItem({
        'text/plain': authorized().then((t) => new Blob([t], { type: 'text/plain' })),
      }),
    ]);
  } else {
    const text = await authorized();
    if (!navigator.clipboard?.writeText) throw new Error('Буфер обмена недоступен.');
    await navigator.clipboard.writeText(text);
  }
}
