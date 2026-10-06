import { describe, expect, it } from 'vitest';
import { parseMarkdown } from './markdown';
import noTable from './fixtures/no-table.md?raw';
import singleTable from './fixtures/single-table.md?raw';
import multipleTables from './fixtures/multiple-tables.md?raw';
import tableNextToCode from './fixtures/table-next-to-code.md?raw';
import invalidTable from './fixtures/invalid-table.md?raw';
import largeTable from './fixtures/large.md?raw';

describe('GFM table extraction from Markdown AST', () => {
  it.each([
    '# Heading\n\nParagraph\n\n- list',
    '> quote',
    '```ts\na | b\n\n```',
    'Plain `inline | code`',
  ])('preserves structured Markdown as a whole document: %s', (text) =>
    expect(parseMarkdown(text).recognized).toBe(true),
  );
  it('sends undecorated plain lines through plain-text row parsing', () =>
    expect(parseMarkdown('first row\nsecond row').recognized).toBe(false));
  it('finds GFM alignments, escaped pipes and inline text', () => {
    const parsed = parseMarkdown(
      '| Name | Value |\n| :--- | ---: |\n| **one** | `a\\|b` |\n| link [here](https://example.invalid) | x<br>y |',
    );
    expect(parsed.tables).toHaveLength(1);
    expect(parsed.tables[0]?.heading).toBe('');
    expect(parsed.tables[0]?.alignment).toEqual(['left', 'right']);
    expect(parsed.tables[0]?.cells).toEqual([
      ['Name', 'Value'],
      ['one', 'a|b'],
      ['link here', 'x\ny'],
    ]);
  });
  it('recognizes a document heading followed by an aligned table', () => {
    expect(
      parseMarkdown(
        '# Inventory\n\n| Item | Count |\n| :--- | ---: |\n| Apples | 3 |\n| Pears | 4 |',
      ).tables,
    ).toHaveLength(1);
  });
  it('enumerates separate tables and tracks their nearest heading', () => {
    const parsed = parseMarkdown(
      '# First\n\n| a |\n| - |\n| x |\n\n## Second\n\n| b |\n| - |\n| y |',
    );
    expect(
      parsed.tables.map((table) => [table.number, table.heading, table.rows, table.columns]),
    ).toEqual([
      [1, 'First', 2, 1],
      [2, 'Second', 2, 1],
    ]);
  });
  it('does not treat delimiter lookalikes or code pipes as tables', () =>
    expect(parseMarkdown('`a | b`\n\n```\nx | y\n```\n\n|---|no header').tables).toHaveLength(0));
  it('bounds oversized tables and source bytes', () => {
    const rows = ['| a |', '| - |', ...Array.from({ length: 1000 }, () => '| b |')].join('\n');
    expect(parseMarkdown(rows).tables[0]?.tooLarge).toBe(true);
    expect(() => parseMarkdown('x'.repeat(16 * 1024 * 1024 + 1))).toThrow();
  });
  it('uses the representative Markdown fixtures', () => {
    expect(parseMarkdown(noTable).tables).toHaveLength(0);
    expect(parseMarkdown(singleTable).tables[0]?.cells?.[0]).toEqual(['Question', 'Answer']);
    expect(parseMarkdown(multipleTables).tables).toHaveLength(2);
    expect(parseMarkdown(tableNextToCode).tables).toHaveLength(1);
    expect(parseMarkdown(invalidTable).tables).toHaveLength(0);
    const large = parseMarkdown(largeTable).tables[0];
    expect(large).toMatchObject({ rows: 1002, columns: 1, tooLarge: true });
    expect(large?.sample[1]).toEqual(['value 0']);
  });
});
