import { describe, expect, it } from 'vitest';
import { parseDelimited, serializeDelimited } from './tabular';
import multilineQuestionAnswer from './fixtures/multiline-question-answer.tsv?raw';

describe('spreadsheet clipboard quoting', () => {
  const example = [['Вопрос\nПервая строка\nВторая строка', 'Ответ\nПервый пункт\nВторой пункт']];
  it('keeps the question and answer inside two cells', () => {
    expect(parseDelimited(multilineQuestionAnswer)).toEqual(example);
    expect(
      parseDelimited(
        '"Вопрос\r\nПервая строка\r\nВторая строка"\t"Ответ\nПервый пункт\nВторой пункт"\r\n',
      ),
    ).toEqual(example);
  });
  it.each([
    example,
    [['"quoted"', 'a\tb', '😀中文']],
    [
      ['a', '', ''],
      ['', '', ''],
    ],
    [['']],
    [['a'], [''], ['']],
  ])('round trips a complete rectangle %#', (...rows) => {
    const matrix = rows as string[][];
    expect(parseDelimited(serializeDelimited(matrix), '\t', true)).toEqual(matrix);
  });
  it('accepts CSV escapes and CRLF', () =>
    expect(parseDelimited('"a,b","x""y",\r\n1,2,3\r\n', ',')).toEqual([
      ['a,b', 'x"y', ''],
      ['1', '2', '3'],
    ]));
  it('does not invent a trailing record and preserves explicit blank records', () => {
    expect(parseDelimited('a\t\n')).toEqual([['a', '']]);
    expect(parseDelimited('a\n\n')).toEqual([['a'], ['']]);
  });
  it('pads ragged records without losing empty columns', () =>
    expect(parseDelimited('a\tb\nc')).toEqual([
      ['a', 'b'],
      ['c', ''],
    ]));
  it.each(['"unclosed', '"a"b'])('rejects malformed quoting %s', (value) =>
    expect(() => parseDelimited(value)).toThrow(),
  );
  it('bounds decoded cells before allocation grows', () =>
    expect(() => parseDelimited('\t'.repeat(1000))).toThrow());
});
