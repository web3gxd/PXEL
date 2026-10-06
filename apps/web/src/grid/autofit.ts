import type { CellStyle } from '../document/styles';

type Measure = (text: string) => number;
const fontPixels = (style: CellStyle) => (style.font_size_hundredth_points / 100) * (4 / 3);
export function fitColumnWidth(text: string, measure: Measure) {
  return Math.min(
    2000,
    Math.max(24, Math.ceil(Math.max(...text.split(/\r?\n/).map(measure)) + 14)),
  );
}
export function fitRowHeight(text: string, style: CellStyle, width: number, measure: Measure) {
  let lines = 0;
  for (const line of text.split(/\r?\n/)) {
    if (style.text_overflow !== 'wrap') {
      lines++;
      continue;
    }
    const available = Math.max(1, width - 14);
    let segment = '';
    lines++;
    for (const char of line) {
      if (segment && measure(segment + char) > available) {
        lines++;
        segment = char;
      } else segment += char;
      if (lines * fontPixels(style) * 1.2 >= 1000) return 1000;
    }
  }
  return Math.min(1000, Math.max(28, Math.ceil(lines * fontPixels(style) * 1.2 + 8)));
}
