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

/**
 * Maps a custom scrollbar's normalized position to the sheet's linear offset.
 * The used segment gets an adaptive share, followed by a linear empty-range
 * segment. Both segments meet exactly at the used boundary; the inverse uses
 * the same breakpoint, so dragging never jumps or reverses direction.
 */
export class AxisScrollMapping {
  readonly totalOffset: number;
  readonly usedOffset: number;
  readonly usedShare: number;
  readonly isAdaptive: boolean;

  constructor(totalOffset: number, usedOffset: number) {
    this.totalOffset = Number.isFinite(totalOffset) ? Math.max(0, totalOffset) : 0;
    this.usedOffset = Number.isFinite(usedOffset)
      ? Math.max(0, Math.min(this.totalOffset, usedOffset))
      : 0;
    const ratio = this.totalOffset > 0 ? this.usedOffset / this.totalOffset : 0;
    this.isAdaptive = this.totalOffset > 0 && ratio > 0 && ratio < 0.9;
    if (!this.isAdaptive) this.usedShare = 1;
    else {
      const ordinaryShare = 0.65 + 0.3 * (1 - Math.sqrt(ratio));
      const fade = Math.max(0, Math.min(1, (ratio - 0.6) / 0.3));
      const smoothFade = fade * fade * (3 - 2 * fade);
      this.usedShare = ordinaryShare + (ratio - ordinaryShare) * smoothFade;
    }
  }

  offsetAt(position: number) {
    const normalized = Math.max(0, Math.min(1, Number.isFinite(position) ? position : 0));
    if (!this.isAdaptive) return normalized * this.totalOffset;
    if (normalized <= this.usedShare) return (normalized / this.usedShare) * this.usedOffset;
    return (
      this.usedOffset +
      ((normalized - this.usedShare) / (1 - this.usedShare)) * (this.totalOffset - this.usedOffset)
    );
  }

  positionAt(offset: number) {
    const clamped = Math.max(0, Math.min(this.totalOffset, Number.isFinite(offset) ? offset : 0));
    if (!this.isAdaptive || this.totalOffset === 0)
      return this.totalOffset === 0 ? 0 : clamped / this.totalOffset;
    if (clamped <= this.usedOffset) return (clamped / this.usedOffset) * this.usedShare;
    return (
      this.usedShare +
      ((clamped - this.usedOffset) / (this.totalOffset - this.usedOffset)) * (1 - this.usedShare)
    );
  }
}

/** Nearest-rank 95th percentile; sparse tail outliers remain reachable linearly. */
export function robustUsedEnd(indices: readonly number[], extent: number): number | undefined {
  if (!Number.isInteger(extent) || extent <= 0 || indices.length === 0) return undefined;
  const sorted = [
    ...new Set(indices.filter((index) => Number.isInteger(index) && index >= 0 && index < extent)),
  ].sort((left, right) => left - right);
  if (!sorted.length) return undefined;
  if (sorted.length < 20) return sorted[sorted.length - 1];
  return sorted[Math.max(0, Math.ceil(sorted.length * 0.95) - 1)];
}

export type AxisOverride = { axis_id: string; size_css_px: number };
export type AxisLayout = Pick<
  AxisMetrics,
  | 'count'
  | 'totalSize'
  | 'sizeAt'
  | 'offsetAt'
  | 'indexAt'
  | 'nearestVisible'
  | 'indicesInRange'
  | 'visibleRange'
>;

/** A single size delta shares the prefix index; pointer frames never rebuild an axis. */
export function withAxisSize(base: AxisLayout, index: number, size: number): AxisLayout {
  if (index < 0 || index >= base.count || base.sizeAt(index) === 0 || base.sizeAt(index) === size)
    return base;
  const delta = size - base.sizeAt(index);
  const next: AxisLayout = {
    count: base.count,
    totalSize: base.totalSize + delta,
    sizeAt: (position) => (position === index ? size : base.sizeAt(position)),
    offsetAt: (position) => base.offsetAt(position) + (position > index ? delta : 0),
    indexAt: (offset) => {
      const target = Math.max(0, Math.min(next.totalSize, Number.isFinite(offset) ? offset : 0));
      let low = 0,
        high = next.count;
      while (low < high) {
        const middle = (low + high + 1) >>> 1;
        if (next.offsetAt(middle) <= target) low = middle;
        else high = middle - 1;
      }
      return base.nearestVisible(Math.min(next.count - 1, low), 1);
    },
    nearestVisible: (position, direction) => base.nearestVisible(position, direction),
    indicesInRange: (start, end) => base.indicesInRange(start, end),
    visibleRange: (scroll, viewport, overscan = GRID.overscan) => {
      if (!next.count || !Number.isFinite(scroll) || !Number.isFinite(viewport))
        return { start: 0, end: 0 };
      return {
        start: Math.max(0, next.indexAt(Math.max(0, scroll)) - overscan),
        end: Math.min(
          next.count,
          next.indexAt(Math.max(0, scroll) + Math.max(0, viewport)) + 1 + overscan,
        ),
      };
    },
  };
  return next;
}
export class AxisMetrics {
  private readonly offsets: Float64Array;
  private readonly sizes: Float64Array;
  private readonly visible: number[] = [];
  constructor(
    ids: readonly string[],
    overrides: readonly AxisOverride[] | undefined,
    defaultSize: number,
    hidden?: ReadonlySet<number>,
  ) {
    this.sizes = new Float64Array(ids.length);
    this.offsets = new Float64Array(ids.length + 1);
    this.sizes.fill(defaultSize);
    if (overrides?.length) {
      const positions = new Map(ids.map((id, index) => [id, index]));
      for (const override of overrides) {
        const index = positions.get(override.axis_id);
        if (
          index !== undefined &&
          Number.isFinite(override.size_css_px) &&
          override.size_css_px > 0
        )
          this.sizes[index] = override.size_css_px;
      }
    }
    for (let index = 0; index < ids.length; index++) {
      if (hidden?.has(index)) this.sizes[index] = 0;
      else this.visible.push(index);
      this.offsets[index + 1] = (this.offsets[index] ?? 0) + (this.sizes[index] ?? defaultSize);
    }
  }
  get count() {
    return this.sizes.length;
  }
  get totalSize() {
    return this.offsets[this.offsets.length - 1] ?? 0;
  }
  sizeAt(index: number) {
    return index >= 0 && index < this.count ? (this.sizes[index] ?? 0) : 0;
  }
  offsetAt(index: number) {
    return this.offsets[Math.max(0, Math.min(this.count, index))] ?? 0;
  }
  indexAt(offset: number) {
    if (!this.count) return 0;
    const target = Math.max(0, Math.min(this.totalSize, Number.isFinite(offset) ? offset : 0));
    let low = 0,
      high = this.count;
    while (low < high) {
      const middle = (low + high + 1) >>> 1;
      if ((this.offsets[middle] ?? 0) <= target) low = middle;
      else high = middle - 1;
    }
    return this.nearestVisible(Math.min(this.count - 1, low), 1);
  }
  private visiblePosition(index: number) {
    let low = 0,
      high = this.visible.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (this.visible[middle]! < index) low = middle + 1;
      else high = middle;
    }
    return low;
  }
  nearestVisible(index: number, direction: 1 | -1) {
    const position = this.visiblePosition(index);
    if (this.visible[position] === index) return index;
    return (
      this.visible[
        direction === 1 ? Math.min(position, this.visible.length - 1) : Math.max(0, position - 1)
      ] ?? 0
    );
  }
  *indicesInRange(start: number, end: number) {
    for (
      let i = this.visiblePosition(start);
      i < this.visible.length && this.visible[i]! < end;
      i++
    )
      yield this.visible[i]!;
  }
  visibleRange(scroll: number, viewport: number, overscan: number = GRID.overscan): VisibleRange {
    if (!this.count || !Number.isFinite(scroll) || !Number.isFinite(viewport))
      return { start: 0, end: 0 };
    const start = Math.max(0, this.indexAt(Math.max(0, scroll)) - overscan);
    const end = Math.min(
      this.count,
      this.indexAt(Math.max(0, scroll) + Math.max(0, viewport)) + 1 + overscan,
    );
    return { start, end };
  }
}

const metricsCache = new WeakMap<
  readonly string[],
  WeakMap<object, WeakMap<object, AxisMetrics>>
>();
const noOverrides = Object.freeze([]) as readonly AxisOverride[];
const noHidden = new Set<number>();
export function axisMetrics(
  ids: readonly string[],
  overrides: readonly AxisOverride[] | undefined,
  defaultSize: number,
  hidden?: ReadonlySet<number>,
) {
  const key = overrides ?? noOverrides;
  let byOverride = metricsCache.get(ids);
  if (!byOverride) {
    byOverride = new WeakMap();
    metricsCache.set(ids, byOverride);
  }
  let byHidden = byOverride.get(key);
  if (!byHidden) {
    byHidden = new WeakMap();
    byOverride.set(key, byHidden);
  }
  const hiddenKey = hidden ?? noHidden;
  let metrics = byHidden.get(hiddenKey);
  if (!metrics) {
    metrics = new AxisMetrics(ids, overrides, defaultSize, hidden);
    byHidden.set(hiddenKey, metrics);
  }
  return metrics;
}

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
