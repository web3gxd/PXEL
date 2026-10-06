import { expect, it } from 'vitest';
import { decodeComposition, validPassword } from './pxel-file';
it('counts Unicode scalar passwords without trimming or normalization', () => {
  expect(validPassword('😀'.repeat(12))).toBe(true);
  expect(validPassword('я'.repeat(128))).toBe(true);
  expect(validPassword('x'.repeat(11))).toBe(false);
  expect(validPassword('x'.repeat(129))).toBe(false);
  expect(validPassword(' '.repeat(12))).toBe(true);
});
it('requires a supported complete file composition before publication', () => {
  const input = {
    schema_version: 1,
    title: 'Book',
    sheets: [{ name: 'Data', rows: 100, columns: 5, cells: 2 }],
    formulas: 1,
    anonymized: false,
    features: ['Формулы'],
    omitted_filters: 0,
  };
  expect(decodeComposition(input).formulas).toBe(1);
  expect(() => decodeComposition({ ...input, schema_version: 2 })).toThrow();
  expect(() => decodeComposition({ ...input, sheets: [] })).toThrow();
  expect(() => decodeComposition({ ...input, formulas: -1 })).toThrow();
});
