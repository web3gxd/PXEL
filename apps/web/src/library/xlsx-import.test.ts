import { expect, it, vi } from 'vitest';
import { decodeXlsxPreview, readXlsxFile } from './xlsx-import';

const preview = {
  schema_version: 1,
  selected_sheets: [0],
  preview_digest: 'a'.repeat(64),
  sheets: [
    {
      index: 0,
      name: 'Учёт',
      cells: 1,
      styles: 0,
      merges: 0,
      sample: [{ row: 1, column: 1, value: { Text: '001' } }],
    },
  ],
  issues: [{ code: 'WorkbookMetadata', sheet_index: null }],
};
it('validates typed sparse samples and refuses unknown semantics and inconsistent selection', () => {
  expect(decodeXlsxPreview(preview)).toEqual(preview);
  for (const value of [
    null,
    { ...preview, schema_version: 2 },
    { ...preview, selected_sheets: [] },
    { ...preview, selected_sheets: [1] },
    { ...preview, selected_sheets: [0, 0] },
    { ...preview, preview_digest: 'not-a-digest' },
    { ...preview, issues: [{ code: 'Unknown', sheet_index: null }] },
    { ...preview, issues: [{ code: 'CellFormatting', sheet_index: 1 }] },
    {
      ...preview,
      sheets: [{ ...preview.sheets[0], sample: [{ row: 0, column: 1, value: { Text: 'x' } }] }],
    },
    {
      ...preview,
      sheets: [
        { ...preview.sheets[0], sample: [{ row: 1, column: 1, value: { Text: 'x'.repeat(257) } }] },
      ],
    },
    {
      ...preview,
      sheets: [{ ...preview.sheets[0], sample: [{ row: 1, column: 1, value: { Unknown: 1 } }] }],
    },
  ])
    expect(() => decodeXlsxPreview(value)).toThrow('предпросмотр');
});
it('encodes exact bytes and refuses oversized files before reading', async () => {
  await expect(readXlsxFile(new File([], 'empty.xlsx'))).rejects.toThrow('32 МиБ');
  const file = new File([new Uint8Array([0, 255, 17])], 'book.xlsx');
  expect(atob((await readXlsxFile(file)).file_base64)).toBe('\x00\xff\x11');
  const arrayBuffer = vi.fn();
  await expect(
    readXlsxFile({ size: 32 * 1024 * 1024 + 1, arrayBuffer } as unknown as File),
  ).rejects.toThrow('32 МиБ');
  expect(arrayBuffer).not.toHaveBeenCalled();
});
