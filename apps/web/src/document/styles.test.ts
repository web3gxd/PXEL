import { describe, expect, it } from 'vitest';
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { StylePatchSchema } from '../generated/pxel/wire/v1/session_pb';
import {
  applyStyle,
  canvasFont,
  DEFAULT_STYLE,
  decodeStyle,
  encodeStylePatch,
  formatNumber,
  isDefaultStyle,
  type CellStyle,
  type StyleChange,
} from './styles';
import { decodeProjection, uuidV7, type Projection } from './store';

export function authoredProjection(): Projection {
  return {
    authored_projection_schema_version: 4,
    style_schema_version: 1,
    style_profile_version: 1,
    document_id: uuidV7(),
    version: '1',
    state_hash: '0'.repeat(64),
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
    calculation_context: null,
    sheets: [
      {
        id: uuidV7(),
        name: 'Лист 1',
        rows: [uuidV7()],
        columns: [uuidV7()],
        cells: [],
        styles: [],
      },
    ],
  };
}
describe('style profile boundary', () => {
  it('validates every leaf and rejects unknown fields, enums, numbers and malformed RGB', () => {
    expect(decodeStyle(structuredClone(DEFAULT_STYLE))).toEqual(DEFAULT_STYLE);
    for (const patch of [
      { font_bold: 1 },
      { font_italic: 'true' },
      { font_strike: null },
      { wrap: 0 },
      { font_family: '' },
      { font_family: ' Arial' },
      { font_family: 'A\nB' },
      { font_family: 'я'.repeat(65) },
      { font_size_hundredth_points: 99 },
      { font_size_hundredth_points: 100.5 },
      { font_size_hundredth_points: 20001 },
      { font_color: [0, 0] },
      { fill: [-1, 0, 0] },
      { fill: [1.5, 0, 0] },
      { fill: [256, 0, 0] },
      { font_underline: 'triple' },
      { horizontal: 'justify' },
      { vertical: 'middle' },
      { number_format: '0.000' },
      { border_top: { line: 'thin' } },
      { border_left: { line: 'url', color: [0, 0, 0] } },
      { border_right: { line: 'thin', color: [0, 0, 0], url: 'remote' } },
      { surprise: true },
    ])
      expect(() => decodeStyle({ ...DEFAULT_STYLE, ...patch })).toThrow();
    expect(() => decodeStyle({ font_bold: true })).toThrow();
  });
  it('encodes all sixteen typed leaf Sets and resets without content payloads', () => {
    const style: CellStyle = {
      ...DEFAULT_STYLE,
      font_family: 'Noto Sans',
      font_size_hundredth_points: 1250,
      font_bold: true,
      font_italic: true,
      font_underline: 'double',
      font_strike: true,
      font_color: [1, 2, 3],
      fill: [4, 5, 6],
      border_top: { line: 'thin', color: [0, 0, 0] },
      border_right: { line: 'medium', color: [0, 0, 0] },
      border_bottom: { line: 'double', color: [0, 0, 0] },
      border_left: { line: 'dashed', color: [0, 0, 0] },
      horizontal: 'right',
      vertical: 'top',
      wrap: true,
      number_format: 'date_iso',
    };
    const changes = Object.entries(style).map(([property, value]) => ({
      property,
      value,
    })) as StyleChange[];
    const patch = encodeStylePatch(changes);
    const restored = fromBinary(StylePatchSchema, toBinary(StylePatchSchema, patch));
    expect(restored.changes.map((c) => c.kind.case)).toEqual([
      'fontFamily',
      'fontSize',
      'fontBold',
      'fontItalic',
      'fontUnderline',
      'fontStrike',
      'fontColor',
      'fill',
      'borderTop',
      'borderRight',
      'borderBottom',
      'borderLeft',
      'horizontal',
      'vertical',
      'wrap',
      'numberFormat',
    ]);
    expect(restored.changes.every((c) => c.kind.value?.action.case === 'set')).toBe(true);
    expect(applyStyle(DEFAULT_STYLE, changes)).toEqual(style);
    const resets = changes.map((c) => ({ property: c.property, reset: true as const }));
    expect(
      encodeStylePatch(resets).changes.every((c) => c.kind.value?.action.case === 'reset'),
    ).toBe(true);
    expect(applyStyle(style, resets)).toEqual(DEFAULT_STYLE);
    expect(
      encodeStylePatch([{ property: 'font_bold', value: false }]).changes[0]?.kind.value?.action,
    ).toEqual({ case: 'set', value: false });
    expect(() => encodeStylePatch([])).toThrow();
    expect(() => encodeStylePatch([changes[0]!, changes[0]!])).toThrow();
    expect(() =>
      applyStyle(style, [{ property: 'border_top', value: null } as unknown as StyleChange]),
    ).toThrow();
    expect(
      isDefaultStyle(decodeStyle(Object.fromEntries(Object.entries(DEFAULT_STYLE).reverse()))),
    ).toBe(true);
    expect(create(StylePatchSchema, {}).schemaVersion).toBe(0);
  });
  it('requires full schema4 metadata and rejects legacy styles or inconsistent revival', () => {
    const projection = authoredProjection();
    const sheet = projection.sheets[0]!;
    sheet.styles = [
      {
        row_id: sheet.rows[0]!,
        column_id: sheet.columns[0]!,
        style: { ...DEFAULT_STYLE, font_bold: true },
      },
    ];
    expect(decodeProjection(structuredClone(projection)).sheets[0]?.styles).toHaveLength(1);
    for (const patch of [
      { authored_projection_schema_version: 3 },
      { style_schema_version: 2 },
      { style_profile_version: 0 },
      { last_axis_change_version: '2' },
      { retired_row_ids: [sheet.rows[0]] },
      { row_activation_floors: [{ row_id: sheet.rows[0], version: '1' }] },
    ])
      expect(() => decodeProjection({ ...structuredClone(projection), ...patch })).toThrow();
    const legacy = {
      document_id: projection.document_id,
      version: '1',
      state_hash: projection.state_hash,
      sheets: projection.sheets,
    };
    expect(() => decodeProjection(legacy)).toThrow();
    const revived = {
      ...projection,
      version: '3',
      last_axis_change_version: '3',
      retired_row_ids: [sheet.rows[0]!],
      row_activation_floors: [{ row_id: sheet.rows[0]!, version: '3' }],
    };
    expect(decodeProjection(structuredClone(revived)).row_activation_floors).toEqual(
      revived.row_activation_floors,
    );
    for (const styles of [
      [sheet.styles[0], sheet.styles[0]],
      [{ ...sheet.styles[0], row_id: uuidV7() }],
      [{ ...sheet.styles[0], style: DEFAULT_STYLE }],
      undefined,
    ])
      expect(() => decodeProjection({ ...projection, sheets: [{ ...sheet, styles }] })).toThrow();
  });
  it('requires bounded exact formula wire metadata for authored projections', () => {
    const projection = authoredProjection(),
      sheet = projection.sheets[0]!;
    const formula = {
      source: '=1',
      canonical: '=1',
      ast_schema: 1,
      compatibility_profile: 'pxel-excel-subset-v1',
      bound_at_version: '1',
    };
    for (const bound_formula_wire_hex of [undefined, '', '0', 'GG', '00'.repeat(524289)]) {
      sheet.cells = [
        {
          row_id: sheet.rows[0]!,
          column_id: sheet.columns[0]!,
          value: {
            type: 'formula',
            value: {
              ...formula,
              ...(bound_formula_wire_hex !== undefined ? { bound_formula_wire_hex } : {}),
            },
          },
        },
      ];
      expect(() => decodeProjection(structuredClone(projection))).toThrow();
    }
    sheet.cells[0]!.value = {
      type: 'formula',
      value: { ...formula, bound_formula_wire_hex: '0a0131' },
    };
    expect(decodeProjection(projection).sheets[0]?.cells).toHaveLength(1);
  });
  it('quotes arbitrary font names as one CSS family without loading URLs', () => {
    const font = canvasFont({
      ...DEFAULT_STYLE,
      font_family: 'a";url(https://example.invalid)\\b',
      font_bold: true,
    });
    expect(font).toBe(
      'bold 13.333333333333334px "a\\";url(https://example.invalid)\\\\b", sans-serif',
    );
  });
});
describe('profile1 display formatting (authored values remain unchanged)', () => {
  it.each([
    [1.005, 'decimal2', '1.01'],
    [-1.005, 'decimal2', '-1.01'],
    [2.5, 'integer', '3'],
    [-2.5, 'integer', '-3'],
    [-0, 'general', '0'],
    [-0.0001, 'decimal2', '0.00'],
    [0.12345, 'percent2', '12.35%'],
    [0.125, 'percent0', '13%'],
    [1234, 'scientific2', '1.23E+3'],
    [0, 'scientific2', '0.00E+0'],
    [1, 'date_iso', '1900-01-01'],
    [59.75, 'date_iso', '1900-02-28'],
    [60, 'date_iso', '1900-02-29'],
    [61, 'date_iso', '1900-03-01'],
    [1462, 'date_iso', '1904-01-01'],
    [2958465, 'date_iso', '9999-12-31'],
    [0, 'date_iso', '#FORMAT!'],
    [-1, 'date_iso', '#FORMAT!'],
    [2958466, 'date_iso', '#FORMAT!'],
  ] as const)('%s with %s → %s', (value, format, expected) =>
    expect(formatNumber(value, format)).toBe(expected),
  );
});
