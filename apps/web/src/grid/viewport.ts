export const GRID = {
  rowCount: 1_000,
  columnCount: 26,
  rowHeight: 28,
  columnWidth: 112,
  rowHeaderWidth: 46,
  columnHeaderHeight: 28,
  overscan: 2,
} as const;

export type VisibleRange = { start: number; end: number };

/** A half-open range, bounded by allocated axes rather than the potential sheet. */
export function visibleRange(
  scroll: number,
  viewport: number,
  size: number,
  count: number,
  overscan: number = GRID.overscan,
): VisibleRange {
  if (!Number.isFinite(scroll) || !Number.isFinite(viewport) || size <= 0 || count <= 0) {
    return { start: 0, end: 0 };
  }
  const first = Math.floor(Math.max(0, scroll) / size);
  return {
    start: Math.min(count, Math.max(0, first - overscan)),
    end: Math.min(
      count,
      Math.max(0, Math.ceil((Math.max(0, scroll) + Math.max(0, viewport)) / size) + overscan),
    ),
  };
}

export function columnLabel(index: number): string {
  let value = index + 1;
  let label = '';
  while (value > 0) {
    label = String.fromCharCode(65 + ((value - 1) % 26)) + label;
    value = Math.floor((value - 1) / 26);
  }
  return label;
}

export function backingSize(cssPixels: number, dpr: number): number {
  return Math.max(1, Math.round(cssPixels * dpr));
}
