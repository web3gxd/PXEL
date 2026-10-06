import { describe, expect, it, vi } from 'vitest';
import {
  AxisMetrics,
  axisMetrics,
  AxisScrollMapping,
  backingSize,
  columnLabel,
  robustUsedEnd,
  visibleRange,
  withAxisSize,
} from './viewport';

describe('viewport boundaries', () => {
  it('refreshes cached geometry when the sheet default width changes', () => {
    const ids = ['a', 'b'];
    const overrides = [{ axis_id: 'b', size_css_px: 200 }];
    expect(axisMetrics(ids, overrides, 112).totalSize).toBe(312);
    expect(axisMetrics(ids, overrides, 80).totalSize).toBe(280);
  });
  it('skips hidden rows in offsets, hit tests, and navigation without changing identities', () => {
    const axis = new AxisMetrics(['a', 'b', 'c', 'd', 'e'], undefined, 28, new Set([1, 2, 4]));
    expect(axis.count).toBe(5);
    expect(axis.totalSize).toBe(56);
    expect(axis.sizeAt(1)).toBe(0);
    expect(axis.indexAt(28)).toBe(3);
    expect(axis.indexAt(56)).toBe(3);
    expect([...axis.indicesInRange(0, 5)]).toEqual([0, 3]);
    expect(axis.nearestVisible(1, 1)).toBe(3);
    expect(axis.nearestVisible(2, -1)).toBe(0);
  });
  it('uses half-open ranges and includes overscan on both sides', () => {
    expect(visibleRange(100, 100, 20, 1_000, 2)).toEqual({ start: 3, end: 12 });
    expect(visibleRange(101, 100, 20, 1_000, 0)).toEqual({ start: 5, end: 11 });
  });

  it('clamps at both sheet edges including scroll rubber-banding', () => {
    expect(visibleRange(-14, 100, 20, 8, 2)).toEqual({ start: 0, end: 7 });
    expect(visibleRange(150, 100, 20, 8, 2)).toEqual({ start: 5, end: 8 });
    expect(visibleRange(100_000, 100, 20, 8, 2)).toEqual({ start: 8, end: 8 });
  });

  it('does not scale rendered work with allocated sheet size', () => {
    expect(visibleRange(20_000, 700, 28, 1_000_000)).toEqual(visibleRange(20_000, 700, 28, 10_000));
  });

  it('handles absent measurements safely', () => {
    expect(visibleRange(Number.NaN, 100, 20, 8)).toEqual({ start: 0, end: 0 });
    expect(visibleRange(0, 100, 0, 8)).toEqual({ start: 0, end: 0 });
  });
});

describe('variable axis metrics', () => {
  it('previews size deltas with the same geometry as a rebuilt axis, including hidden rows', () => {
    const ids = ['a', 'b', 'c', 'd', 'e'];
    const hidden = new Set([1, 4]);
    const base = new AxisMetrics(ids, [{ axis_id: 'd', size_css_px: 40 }], 28, hidden);
    const preview = withAxisSize(base, 2, 90);
    const expected = new AxisMetrics(
      ids,
      [
        { axis_id: 'c', size_css_px: 90 },
        { axis_id: 'd', size_css_px: 40 },
      ],
      28,
      hidden,
    );
    for (let index = -1; index <= ids.length + 1; index++) {
      expect(preview.sizeAt(index)).toBe(expected.sizeAt(index));
      expect(preview.offsetAt(index)).toBe(expected.offsetAt(index));
    }
    for (let offset = -1; offset <= expected.totalSize + 1; offset++) {
      expect(preview.indexAt(offset)).toBe(expected.indexAt(offset));
      expect(preview.visibleRange(offset, 50)).toEqual(expected.visibleRange(offset, 50));
    }
    expect([...preview.indicesInRange(0, 5)]).toEqual([0, 2, 3]);
    expect(withAxisSize(base, 1, 50)).toBe(base);
    expect(withAxisSize(base, -1, 50)).toBe(base);
    expect(base.sizeAt(2)).toBe(28);
  });

  it('keeps a resize at 100000 rows logarithmic without rebuilding prefix storage', () => {
    const ids = Array.from({ length: 100_000 }, (_, index) => String(index));
    const base = new AxisMetrics(ids, undefined, 28);
    const offsetAt = vi.spyOn(base, 'offsetAt');
    const preview = withAxisSize(base, 50_000, 1000);
    expect(preview.indexAt(1_400_999)).toBe(50_000);
    expect(offsetAt.mock.calls.length).toBeLessThanOrEqual(18);
    expect(preview.totalSize).toBe(2_800_972);
    expect(base.totalSize).toBe(2_800_000);
  });
  it('maps offsets, cell sizes and visible ranges across sparse overrides', () => {
    const axis = new AxisMetrics(
      ['r1', 'r2', 'r3', 'r4'],
      [
        { axis_id: 'r2', size_css_px: 40 },
        { axis_id: 'r4', size_css_px: 12 },
      ],
      28,
    );
    expect(axis.totalSize).toBe(108);
    expect([0, 1, 2, 3, 4].map(axis.offsetAt.bind(axis))).toEqual([0, 28, 68, 96, 108]);
    expect([0, 27, 28, 67, 68, 95, 96, 107].map(axis.indexAt.bind(axis))).toEqual([
      0, 0, 1, 1, 2, 2, 3, 3,
    ]);
    expect(axis.sizeAt(3)).toBe(12);
    expect(axis.visibleRange(28, 40, 0)).toEqual({ start: 1, end: 3 });
  });

  it('retains logarithmic offset lookup at the allocated row limit', () => {
    const ids = Array.from({ length: 100_000 }, (_, index) => String(index));
    const axis = new AxisMetrics(ids, [{ axis_id: '99999', size_css_px: 1000 }], 28);
    expect(axis.totalSize).toBe(2_800_972);
    expect(axis.indexAt(axis.offsetAt(99_999))).toBe(99_999);
    expect(axis.indexAt(axis.totalSize)).toBe(99_999);
  });
});

describe('adaptive scrollbar mapping', () => {
  it.each([
    { total: 2_800_000, used: 280, label: '20 used rows' },
    { total: 2_800_000, used: 28_000, label: '1000 used rows' },
    { total: 2_800_000, used: 2_800_000, label: '100000 used rows' },
  ])('is continuous, monotone and invertible for $label', ({ total, used }) => {
    const mapping = new AxisScrollMapping(total, used);
    const samples = Array.from({ length: 1001 }, (_, index) => index / 1000);
    const offsets = samples.map((position) => mapping.offsetAt(position));
    expect(offsets[0]).toBe(0);
    expect(offsets.at(-1)).toBe(total);
    expect(offsets.every((offset, index) => index === 0 || offset >= offsets[index - 1]!)).toBe(
      true,
    );
    for (const position of samples) {
      expect(mapping.positionAt(mapping.offsetAt(position))).toBeCloseTo(position, 10);
    }
    expect(mapping.offsetAt(mapping.usedShare)).toBeCloseTo(mapping.usedOffset, 9);
  });

  it('uses a larger scrollbar share for a small used range and stays linear near the extent', () => {
    const small = new AxisScrollMapping(100_000, 1_000);
    const medium = new AxisScrollMapping(100_000, 30_000);
    const almostFull = new AxisScrollMapping(100_000, 95_000);
    expect(small.usedShare).toBeGreaterThan(medium.usedShare);
    expect(medium.usedShare).toBeLessThan(almostFull.usedShare);
    expect(almostFull.isAdaptive).toBe(false);
    expect(almostFull.offsetAt(0.5)).toBe(50_000);
  });

  it('falls back to linear scrolling when there is no movable used portion', () => {
    const mapping = new AxisScrollMapping(10_000, 0);
    expect(mapping.isAdaptive).toBe(false);
    expect(mapping.offsetAt(0.25)).toBe(2_500);
    expect(mapping.positionAt(7_500)).toBe(0.75);
  });

  it('fades into linear mode without a mapping jump near the used-range threshold', () => {
    const almostAdaptive = new AxisScrollMapping(100_000, 89_999);
    const linear = new AxisScrollMapping(100_000, 90_000);
    for (const position of [0.1, 0.5, 0.9])
      expect(Math.abs(almostAdaptive.offsetAt(position) - linear.offsetAt(position))).toBeLessThan(
        2,
      );
  });

  it('clamps non-finite and extreme offsets safely', () => {
    const empty = new AxisScrollMapping(0, 0);
    expect(empty.offsetAt(0.5)).toBe(0);
    expect(empty.positionAt(10)).toBe(0);
    const extreme = new AxisScrollMapping(Number.MAX_VALUE, Number.NaN);
    expect(extreme.offsetAt(Number.NaN)).toBe(0);
    expect(extreme.positionAt(Number.POSITIVE_INFINITY)).toBe(0);
  });

  it('keeps isolated outliers accessible outside a robust core range', () => {
    const rows = [...Array.from({ length: 20 }, (_, index) => index), 99_999];
    expect(robustUsedEnd(rows, 100_000)).toBe(19);
    expect(robustUsedEnd([99_999], 100_000)).toBe(99_999);
    expect(robustUsedEnd([], 100_000)).toBeUndefined();
  });
});

it('formats column labels across alphabet boundaries', () => {
  expect([0, 25, 26, 51, 52, 701, 702].map(columnLabel)).toEqual([
    'A',
    'Z',
    'AA',
    'AZ',
    'BA',
    'ZZ',
    'AAA',
  ]);
});

it('sizes a backing store for fractional and integer DPR without truncation', () => {
  expect(backingSize(801, 1.25)).toBe(1001);
  expect(backingSize(801, 2)).toBe(1602);
});
