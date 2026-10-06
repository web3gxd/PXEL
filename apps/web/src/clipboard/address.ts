import type { Position } from '../document/store';

export function parseCellAddress(input: string): Position | undefined {
  const match = /^\$?([A-Z]{1,3})\$?([1-9]\d{0,6})$/.exec(input.trim().toUpperCase());
  if (!match) return undefined;
  let column = 0;
  for (const letter of match[1] ?? '') column = column * 26 + letter.charCodeAt(0) - 64;
  const row = Number(match[2]);
  if (!Number.isSafeInteger(row) || row > 1_000_000 || column > 1000) return undefined;
  return { row: row - 1, column: column - 1 };
}
