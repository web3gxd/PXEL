import { expect, it } from 'vitest';
import { decodeSyntax } from './diagnostics';
it('maps UTF-8 spans to browser UTF-16 without splitting emoji or Cyrillic', () => {
  const source = '=IF(TRUE,"Я😀",A1+2)';
  const byte = (position: number) => new TextEncoder().encode(source.slice(0, position)).length;
  const raw = {
    schema_version: 1,
    tokens: [
      { start: byte(4), end: byte(8), kind: 'boolean' },
      { start: byte(9), end: byte(14), kind: 'text' },
      { start: byte(15), end: byte(17), kind: 'reference' },
      { start: byte(18), end: byte(19), kind: 'number' },
    ],
  };
  expect(decodeSyntax(source, raw).map((token) => source.slice(token.start, token.end))).toEqual([
    'TRUE',
    '"Я😀"',
    'A1',
    '2',
  ]);
  expect(() => decodeSyntax(source, { ...raw, schema_version: 2 })).toThrow();
  expect(() =>
    decodeSyntax(source, { ...raw, tokens: [{ start: 10, end: 11, kind: 'text' }] }),
  ).toThrow();
  expect(() => decodeSyntax(source, { ...raw, tokens: [...raw.tokens, raw.tokens[0]] })).toThrow();
});
