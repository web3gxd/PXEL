export type PrivacyRect = { left: number; top: number; width: number; height: number };

/** Preserve real cell borders while clipping to headers/frozen panes/viewport. */
export function visiblePrivacyRect(
  cell: PrivacyRect,
  viewport: PrivacyRect,
): PrivacyRect | undefined {
  if (![...Object.values(cell), ...Object.values(viewport)].every(Number.isFinite))
    return undefined;
  const left = Math.max(cell.left + 1, viewport.left);
  const top = Math.max(cell.top + 1, viewport.top);
  const right = Math.min(cell.left + cell.width - 1, viewport.left + viewport.width);
  const bottom = Math.min(cell.top + cell.height - 1, viewport.top + viewport.height);
  if (right <= left || bottom <= top) return undefined;
  return { left, top, width: right - left, height: bottom - top };
}
