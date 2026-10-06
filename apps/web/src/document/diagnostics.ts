export type FormulaToken = {
  start: number;
  end: number;
  kind: 'reference' | 'number' | 'text' | 'boolean';
};
/** Rust spans use bytes; DOM strings use UTF-16 code units, including surrogate pairs. */
export function decodeSyntax(source: string, raw: unknown): FormulaToken[] {
  const report = raw as { schema_version?: unknown; tokens?: unknown };
  if (
    !report ||
    report.schema_version !== 1 ||
    !Array.isArray(report.tokens) ||
    report.tokens.length > 4096
  )
    throw new Error('InvalidSyntax');
  const positions = new Map<number, number>([[0, 0]]);
  let bytes = 0,
    characters = 0;
  for (const character of source) {
    bytes += new TextEncoder().encode(character).length;
    characters += character.length;
    positions.set(bytes, characters);
  }
  if (bytes > 32 * 1024) throw new Error('InvalidSyntax');
  let previous = 0;
  return report.tokens.map((raw) => {
    const token = raw as { start: number; end: number; kind: FormulaToken['kind'] };
    if (
      !token ||
      !Number.isInteger(token.start) ||
      !Number.isInteger(token.end) ||
      token.start < previous ||
      token.end <= token.start ||
      !positions.has(token.start) ||
      !positions.has(token.end) ||
      !['reference', 'number', 'text', 'boolean'].includes(token.kind)
    )
      throw new Error('InvalidSyntax');
    previous = token.end;
    return { start: positions.get(token.start)!, end: positions.get(token.end)!, kind: token.kind };
  });
}
export type HardcodeWarning = {
  sheet_id: string;
  row_id: string;
  column_id: string;
  row: number;
  column: number;
  fingerprint: string;
  series_axis: 'row' | 'column';
  formulas: number;
  dominant: number;
};
export type HardcodeReport = {
  schema_version: 1;
  document_version: string;
  formulas: number;
  warnings: HardcodeWarning[];
};
