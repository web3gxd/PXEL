import { describe, expect, it } from 'vitest';
import { DEFAULT_STYLE } from '../document/styles';
import type { Sheet } from '../document/store';
import { axisScrollMapping } from './scrollbar';
import { AxisMetrics } from './viewport';

const sheet = (): Sheet => ({
  id: 's1',
  name: 'Data',
  rows: Array.from({ length: 1_000 }, (_, index) => `r${index}`),
  columns: Array.from({ length: 40 }, (_, index) => `c${index}`),
  cells: [
    ...Array.from({ length: 100 }, (_, index) => ({
      row_id: `r${index}`,
      column_id: 'c0',
      value:
        index === 0
          ? { type: 'formula' as const, value: { source: '=1+1' } }
          : { type: 'text' as const, value: `value ${index}` },
    })),
    { row_id: 'r999', column_id: 'c39', value: { type: 'number', value: 1 } },
    { row_id: 'r700', column_id: 'c30', value: { type: 'blank' } },
  ],
  styles: [
    { row_id: 'r998', column_id: 'c39', style: { ...DEFAULT_STYLE, font_bold: true } },
    { row_id: 'r750', column_id: 'c31', style: { ...DEFAULT_STYLE } },
  ],
});

describe('used-range-aware scrollbar model', () => {
  it('counts values, formulas and meaningful styles but keeps distant outliers on the linear tail', () => {
    const data = sheet();
    const rows = new AxisMetrics(data.rows, undefined, 28);
    const mapping = axisScrollMapping(data, 'row', rows, 280);
    expect(mapping.isAdaptive).toBe(true);
    expect(mapping.usedOffset).toBe(rows.offsetAt(97) - 280);
    expect(mapping.offsetAt(1)).toBe(rows.totalSize - 280);
  });

  it('uses ordinary linear movement when the sheet is empty or the used range fills it', () => {
    const empty = { ...sheet(), cells: [], styles: [] };
    const rows = new AxisMetrics(empty.rows, undefined, 28);
    const noData = axisScrollMapping(empty, 'row', rows, 280);
    expect(noData.isAdaptive).toBe(false);

    const full = {
      ...empty,
      cells: empty.rows.map((row_id, index) => ({
        row_id,
        column_id: 'c0',
        value: { type: 'text' as const, value: String(index) },
      })),
    };
    const fullRange = axisScrollMapping(full, 'row', rows, 280);
    expect(fullRange.isAdaptive).toBe(false);
  });
});
