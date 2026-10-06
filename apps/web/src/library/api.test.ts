import { afterEach, expect, it, vi } from 'vitest';
import { decodePage, mergeEntries, visible, createBook, type Entry } from './api';
const id = '019a1234-1234-7000-8000-000000000001';
const entry: Entry = {
  document_id: id,
  version: 1,
  role: 'owner',
  created_at: 1,
  metadata: { schema_version: 1, revision: 1, title: 'Бюджет' },
  preference: { schema_version: 1, revision: 1, favorite: false, opened_at: 0 },
  disclosure_stamp: { document_version: 1, book_revision: 0, link_revision: 0 },
};
const page = {
  schema_version: 1,
  documents: [entry],
  next_cursor: null,
  layout: { schema_version: 1, revision: 0, layout: 'cards' },
};
afterEach(() => vi.unstubAllGlobals());
it('validates catalog versions identities bounds and per-book disclosure stamps atomically', () => {
  expect(decodePage(page).documents[0]?.metadata.title).toBe('Бюджет');
  for (const value of [
    { ...page, schema_version: 2 },
    { ...page, documents: [entry, entry] },
    { ...page, documents: [{ ...entry, disclosure_stamp: null }] },
    { ...page, documents: [{ ...entry, metadata: { ...entry.metadata, title: '\0' } }] },
    { ...page, documents: Array(101).fill(entry) },
    { ...page, next_cursor: 'unknown' },
  ])
    expect(() => decodePage(value)).toThrow();
});
it('filters favorites recent and shared without changing the source and searches Unicode names', () => {
  const shared: Entry = {
    ...entry,
    document_id: id.replace(/1$/, '2'),
    role: 'viewer',
    metadata: { ...entry.metadata, title: 'Отчёт' },
    preference: { ...entry.preference, favorite: true, opened_at: 20 },
  };
  const all = [entry, shared];
  expect(visible(all, ' ОТЧЁТ ', 'all')).toEqual([shared]);
  for (const section of ['favorites', 'recent', 'shared'] as const)
    expect(visible(all, '', section)).toEqual([shared]);
  expect(all).toEqual([entry, shared]);
});
it('preserves concurrent preference changes when a subsequent catalog page arrives and removes duplicates', () => {
  const updated = { ...entry, preference: { ...entry.preference, revision: 3, favorite: true } };
  expect(mergeEntries([updated], [entry], false)[0]?.preference).toEqual(updated.preference);
  expect(mergeEntries([updated], [entry], true)).toHaveLength(1);
  expect(mergeEntries([updated], [], false)).toEqual([]);
});
it('reuses the same creation identity after an uncertain network result', async () => {
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new Error('network'))
    .mockResolvedValueOnce(new Response(JSON.stringify({ document_id: id })));
  vi.stubGlobal('fetch', fetch);
  const identity = { document_id: id, request_id: id.replace(/1$/, '3') };
  await expect(createBook('budget', identity)).rejects.toThrow();
  expect(await createBook('budget', identity)).toBe(id);
  expect(fetch.mock.calls[0]?.[1]?.body).toBe(fetch.mock.calls[1]?.[1]?.body);
});
