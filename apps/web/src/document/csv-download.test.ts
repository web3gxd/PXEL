import { afterEach, expect, it, vi } from 'vitest';
import { downloadCsv } from './csv-download';
afterEach(() => vi.unstubAllGlobals());
const options = {
  sheetId: '019a1234-5678-789a-8bcd-0123456789ac',
  version: '42',
  delimiter: 'semicolon' as const,
  safeText: true,
  anonymized: false,
};
it('requests one stable sheet and exact confirmed head, then returns a bounded CSV', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValue(
      new Response('\uFEFF"001"\r\n', { headers: { 'Content-Type': 'text/csv; charset=utf-8' } }),
    );
  vi.stubGlobal('fetch', fetcher);
  const blob = await downloadCsv(
    '/api/documents/book/export.csv',
    options,
    new AbortController().signal,
  );
  expect(await blob.text()).toBe('"001"\r\n'); // Blob.text decodes the UTF-8 BOM.
  const url = new URL(String(fetcher.mock.calls[0]?.[0]), 'http://localhost');
  expect(Object.fromEntries(url.searchParams)).toEqual({
    schema_version: '1',
    sheet_id: options.sheetId,
    document_version: '42',
    delimiter: 'semicolon',
    safe_text: 'true',
    anonymized: 'false',
  });
});
it('refuses stale, denied or unexpected downloads without exposing body contents', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response('secret marker', { status: 409 }))
    .mockResolvedValueOnce(new Response('secret marker', { status: 403 }))
    .mockResolvedValueOnce(
      new Response('secret marker', { headers: { 'Content-Type': 'text/html' } }),
    );
  vi.stubGlobal('fetch', fetcher);
  for (let i = 0; i < 3; i++)
    await expect(downloadCsv('/same', options, new AbortController().signal)).rejects.toThrow();
});
it('caps streamed bytes even when Content-Length is absent', async () => {
  let cancelled = false;
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(new Uint8Array(16 * 1024 * 1024));
            controller.enqueue(new Uint8Array(1));
          },
          cancel() {
            cancelled = true;
          },
        }),
        { headers: { 'Content-Type': 'text/csv' } },
      ),
    ),
  );
  await expect(downloadCsv('/same', options, new AbortController().signal)).rejects.toThrow(
    'лимит',
  );
  expect(cancelled).toBe(true);
});
