import { describe, expect, it } from 'vitest';
import { DEFAULT_STYLE } from '../document/styles';
import { fitColumnWidth, fitRowHeight } from './autofit';

const measure = (text: string) => Array.from(text).length * 8;
describe('autofit dimensions', () => {
  it('uses the longest explicit line and bounded padding for column width', () => {
    expect(fitColumnWidth('short\nlonger text', measure)).toBe(102);
    expect(fitColumnWidth('x'.repeat(1000), measure)).toBe(2000);
    expect(fitColumnWidth('', measure)).toBe(24);
  });
  it('accounts for line breaks, font size and wrapping in row height', () => {
    expect(fitRowHeight('a\nb\nc', DEFAULT_STYLE, 112, measure)).toBeGreaterThan(28);
    expect(
      fitRowHeight('abcdefghij', { ...DEFAULT_STYLE, text_overflow: 'wrap' }, 30, measure),
    ).toBeGreaterThan(fitRowHeight('abcdefghij', DEFAULT_STYLE, 30, measure));
    expect(
      fitRowHeight('a', { ...DEFAULT_STYLE, font_size_hundredth_points: 7200 }, 112, measure),
    ).toBeGreaterThan(90);
    expect(fitRowHeight('\n'.repeat(1000), DEFAULT_STYLE, 112, measure)).toBe(1000);
  });
});
