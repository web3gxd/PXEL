import { afterEach, expect, it, vi } from 'vitest';
import { decodeCsvPreview, publishCsv, readCsvFile, type CsvPreview } from './csv-import';

const preview: CsvPreview = {
  schema_version: 1,
  title: 'Учёт',
  rows: 2,
  columns: 2,
  column_types: ['text', 'number'],
  sample: [
    ['Код', 'Сумма'],
    ['001', '12.50'],
  ],
  preview_digest: 'a'.repeat(64),
};
afterEach(() => vi.unstubAllGlobals());

it('rejects unknown versions, inconsistent samples, types and resource bounds atomically', () => {
  expect(decodeCsvPreview(preview)).toEqual(preview);
  for (const value of [
    null,
    { ...preview, schema_version: 2 },
    { ...preview, rows: 100001 },
    { ...preview, columns: 1001 },
    { ...preview, rows: 1000, columns: 101 },
    { ...preview, column_types: ['number'] },
    { ...preview, column_types: ['formula', 'number'] },
    { ...preview, sample: [['001', '12.50']] },
    {
      ...preview,
      sample: [
        ['x'.repeat(257), '12.50'],
        ['001', '12.50'],
      ],
    },
    { ...preview, preview_digest: 'unknown' },
  ])
    expect(() => decodeCsvPreview(value)).toThrow();
});

it('encodes UTF-8 bytes unchanged and assigns fresh stable import identities', async () => {
  const file = new File(['\uFEFFКод;Сумма\r\n001;12.50'], 'Учёт.csv', { type: 'text/csv' });
  const payload = await readCsvFile(file);
  expect(payload.title).toBe('Учёт');
  expect(payload.column_types).toEqual([]);
  expect(payload.header).toBe(false);
  const decoded = Uint8Array.from(atob(payload.file_base64), (ch) => ch.charCodeAt(0));
  expect([...decoded]).toEqual([...new Uint8Array(await file.arrayBuffer())]);
  expect(payload.document_id).toMatch(/^[0-9a-f-]{14}7[0-9a-f-]{21}$/);
  expect(payload.request_id).not.toBe(payload.document_id);
  expect((await readCsvFile(file)).document_id).not.toBe(payload.document_id);
});

it('refuses empty and oversized files before reading bytes', async () => {
  await expect(readCsvFile(new File([], 'empty.csv'))).rejects.toThrow('16 МиБ');
  const arrayBuffer = vi.fn();
  await expect(
    readCsvFile({ size: 16 * 1024 * 1024 + 1, arrayBuffer } as unknown as File),
  ).rejects.toThrow('16 МиБ');
  expect(arrayBuffer).not.toHaveBeenCalled();
});

it('retries identical publication and refuses a receipt for a different document', async () => {
  const payload = {
    ...(await readCsvFile(new File(['001'], 'test.csv'))),
    preview_digest: preview.preview_digest,
  };
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new Error('network'))
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ document_id: payload.document_id, version: '1' })),
    )
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ document_id: payload.request_id, version: '1' })),
    );
  vi.stubGlobal('fetch', fetch);
  const signal = new AbortController().signal;
  await expect(publishCsv(payload, signal)).rejects.toThrow('network');
  expect(await publishCsv(payload, signal)).toBe(payload.document_id);
  expect(fetch.mock.calls[0]?.[1]?.body).toBe(fetch.mock.calls[1]?.[1]?.body);
  await expect(publishCsv(payload, signal)).rejects.toThrow('подтверждение');
});
