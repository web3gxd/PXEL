import { describe, expect, it } from 'vitest';
import { backingSize, columnLabel, visibleRange } from './viewport';

describe('viewport boundaries', () => {
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
