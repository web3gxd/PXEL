import { describe, expect, it } from 'vitest';
import { fillPlan, numericSeries } from './fill';

describe('fill gesture planning', () => {
  const source = { startRow: 3, endRow: 4, startColumn: 2, endColumn: 2 };
  it('extends complete repeated blocks in all four directions without writing the seed', () => {
    expect(fillPlan(source, { row: 7, column: 2 }, 20, 10)?.destination).toEqual({
      startRow: 5,
      endRow: 8,
      startColumn: 2,
      endColumn: 2,
    });
    expect(fillPlan(source, { row: 1, column: 2 }, 20, 10)?.destination).toEqual({
      startRow: 1,
      endRow: 2,
      startColumn: 2,
      endColumn: 2,
    });
    expect(fillPlan(source, { row: 4, column: 4 }, 20, 10)?.destination).toEqual({
      startRow: 3,
      endRow: 4,
      startColumn: 3,
      endColumn: 4,
    });
    expect(fillPlan(source, { row: 4, column: 0 }, 20, 10)?.destination).toEqual({
      startRow: 3,
      endRow: 4,
      startColumn: 0,
      endColumn: 1,
    });
    expect(fillPlan(source, { row: 4, column: 2 }, 20, 10)).toBeUndefined();
    expect(fillPlan(source, { row: 10000, column: 2 }, 20000, 10)).toBeUndefined();
  });
  it('continues only constant finite numeric steps and handles decimals', () => {
    expect(numericSeries([1, 3, 5], 3, 3)).toEqual([7, 9, 11]);
    expect(numericSeries([0.1, 0.2, 0.3], 3, 2)).toEqual([0.4, 0.5]);
    expect(numericSeries([3, 5], -2, 2)).toEqual([-1, 1]);
    expect(numericSeries([1, 4, 6], 3, 2)).toBeUndefined();
    expect(numericSeries([Number.MAX_VALUE, -Number.MAX_VALUE], 2, 1)).toBeUndefined();
  });
});
