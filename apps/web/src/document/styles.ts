import { create } from '@bufbuild/protobuf';
import {
  StyleChangeSchema,
  StylePatchSchema,
  type StyleChange as WireChange,
} from '../generated/pxel/wire/v1/session_pb';

export type Rgb = readonly [number, number, number];
export type Border = {
  line: 'thin' | 'medium' | 'thick' | 'dashed' | 'dotted' | 'double';
  color: Rgb;
};
export type CellStyle = {
  font_family: string;
  font_size_hundredth_points: number;
  font_bold: boolean;
  font_italic: boolean;
  font_underline: 'none' | 'single' | 'double';
  font_strike: boolean;
  font_color: Rgb;
  fill: Rgb;
  border_top: Border | null;
  border_right: Border | null;
  border_bottom: Border | null;
  border_left: Border | null;
  horizontal: 'general' | 'left' | 'center' | 'right';
  vertical: 'top' | 'center' | 'bottom';
  wrap: boolean;
  number_format:
    'general' | 'integer' | 'decimal2' | 'percent0' | 'percent2' | 'scientific2' | 'date_iso';
};
export type StyleChange =
  | { property: keyof CellStyle; reset: true }
  | { [K in keyof CellStyle]: { property: K; value: NonNullable<CellStyle[K]> } }[keyof CellStyle];
export type StyledCell = { row_id: string; column_id: string; style: CellStyle };
export const DEFAULT_STYLE: Readonly<CellStyle> = Object.freeze({
  font_family: 'Arial',
  font_size_hundredth_points: 1000,
  font_bold: false,
  font_italic: false,
  font_underline: 'none',
  font_strike: false,
  font_color: [0, 0, 0] as const,
  fill: [255, 255, 255] as const,
  border_top: null,
  border_right: null,
  border_bottom: null,
  border_left: null,
  horizontal: 'general',
  vertical: 'bottom',
  wrap: false,
  number_format: 'general',
});
const properties = Object.keys(DEFAULT_STYLE) as (keyof CellStyle)[];
const invalid = () => new Error('Неподдерживаемый формат оформления ячеек');
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid();
  return value as Record<string, unknown>;
}
function exact(value: Record<string, unknown>, keys: readonly string[]) {
  if (Object.keys(value).length !== keys.length || keys.some((key) => !Object.hasOwn(value, key)))
    throw invalid();
}
function rgb(value: unknown): Rgb {
  if (
    !Array.isArray(value) ||
    value.length !== 3 ||
    !value.every((v) => Number.isInteger(v) && v >= 0 && v <= 255)
  )
    throw invalid();
  return [value[0] as number, value[1] as number, value[2] as number];
}
function border(value: unknown): Border | null {
  if (value === null) return null;
  const v = object(value);
  exact(v, ['line', 'color']);
  if (!['thin', 'medium', 'thick', 'dashed', 'dotted', 'double'].includes(v.line as string))
    throw invalid();
  return { line: v.line as Border['line'], color: rgb(v.color) };
}
export function decodeStyle(input: unknown): CellStyle {
  const v = object(input);
  exact(v, properties);
  if (
    typeof v.font_family !== 'string' ||
    !v.font_family ||
    new TextEncoder().encode(v.font_family).length > 128 ||
    v.font_family.trim() !== v.font_family ||
    /\p{Cc}/u.test(v.font_family)
  )
    throw invalid();
  if (
    !Number.isInteger(v.font_size_hundredth_points) ||
    Number(v.font_size_hundredth_points) < 100 ||
    Number(v.font_size_hundredth_points) > 20000
  )
    throw invalid();
  for (const key of ['font_bold', 'font_italic', 'font_strike', 'wrap'])
    if (typeof v[key] !== 'boolean') throw invalid();
  for (const [key, options] of [
    ['font_underline', ['none', 'single', 'double']],
    ['horizontal', ['general', 'left', 'center', 'right']],
    ['vertical', ['top', 'center', 'bottom']],
    [
      'number_format',
      ['general', 'integer', 'decimal2', 'percent0', 'percent2', 'scientific2', 'date_iso'],
    ],
  ] as const)
    if (!options.includes(v[key] as never)) throw invalid();
  return {
    ...v,
    font_color: rgb(v.font_color),
    fill: rgb(v.fill),
    border_top: border(v.border_top),
    border_right: border(v.border_right),
    border_bottom: border(v.border_bottom),
    border_left: border(v.border_left),
  } as CellStyle;
}
export function applyStyle(base: Readonly<CellStyle>, changes: readonly StyleChange[]): CellStyle {
  if (!changes.length || changes.length > 16) throw invalid();
  const result: Record<string, unknown> = { ...base };
  const seen = new Set<string>();
  for (const change of changes) {
    if (!change || !properties.includes(change.property) || seen.has(change.property))
      throw invalid();
    exact(object(change), 'reset' in change ? ['property', 'reset'] : ['property', 'value']);
    if ('reset' in change && change.reset !== true) throw invalid();
    if ('value' in change && change.value === null) throw invalid();
    seen.add(change.property);
    result[change.property] = 'reset' in change ? DEFAULT_STYLE[change.property] : change.value;
  }
  return decodeStyle(result);
}
export function isDefaultStyle(style: CellStyle) {
  return properties.every(
    (key) => JSON.stringify(style[key]) === JSON.stringify(DEFAULT_STYLE[key]),
  );
}
export type AuthoredMetadata = {
  authored_projection_schema_version?: 4;
  style_schema_version?: 1;
  style_profile_version?: 1;
  last_axis_change_version?: string;
  retired_row_ids?: string[];
  retired_column_ids?: string[];
  row_activation_floors?: { row_id: string; version: string }[];
  column_activation_floors?: { column_id: string; version: string }[];
};
export function decodeAuthoredMetadata(
  input: unknown,
  rows: ReadonlySet<string>,
  columns: ReadonlySet<string>,
) {
  const p = object(input);
  const fields = [
    'style_schema_version',
    'style_profile_version',
    'last_axis_change_version',
    'retired_row_ids',
    'retired_column_ids',
    'row_activation_floors',
    'column_activation_floors',
  ];
  if (p.authored_projection_schema_version === undefined) {
    if (fields.some((field) => Object.hasOwn(p, field))) throw invalid();
    return;
  }
  if (
    p.authored_projection_schema_version !== 4 ||
    p.style_schema_version !== 1 ||
    p.style_profile_version !== 1
  )
    throw invalid();
  const version = (v: unknown) => {
    if (typeof v !== 'string' || !/^(0|[1-9]\d{0,18})$/.test(v) || BigInt(v) > 9223372036854775807n)
      throw invalid();
    return BigInt(v);
  };
  const head = version(p.version),
    last = version(p.last_axis_change_version);
  if (last < 1n || last > head) throw invalid();
  const axis = (retired: unknown, floors: unknown, key: string, live: ReadonlySet<string>) => {
    if (
      !Array.isArray(retired) ||
      retired.length > 1000000 ||
      !Array.isArray(floors) ||
      floors.length > retired.length
    )
      throw invalid();
    const ids = new Set<string>();
    let previous = '';
    for (const id of retired) {
      if (
        typeof id !== 'string' ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id) ||
        id <= previous ||
        /^0+-0+-0+-0+-0+$/.test(id)
      )
        throw invalid();
      ids.add(id);
      previous = id;
    }
    const activated = new Set<string>();
    previous = '';
    for (const item of floors) {
      const floor = object(item);
      exact(floor, [key, 'version']);
      const id = floor[key],
        v = version(floor.version);
      if (typeof id !== 'string' || id <= previous || !ids.has(id) || v < 1n || v > last)
        throw invalid();
      activated.add(id);
      previous = id;
    }
    for (const id of ids) if (live.has(id) && !activated.has(id)) throw invalid();
  };
  axis(p.retired_row_ids, p.row_activation_floors, 'row_id', rows);
  axis(p.retired_column_ids, p.column_activation_floors, 'column_id', columns);
}
const action = <T>(change: { reset: true } | { value: T }) =>
  'reset' in change
    ? { case: 'reset' as const, value: true }
    : { case: 'set' as const, value: change.value };
export function encodeStylePatch(changes: readonly StyleChange[]) {
  applyStyle(DEFAULT_STYLE, changes);
  const wire = changes.map((c): WireChange => {
    let kind: WireChange['kind'];
    const reset = 'reset' in c;
    const mapped = <T>(value: T) => action(reset ? { reset: true } : { value });
    switch (c.property) {
      case 'font_family':
        kind = {
          case: 'fontFamily',
          value: { action: mapped(reset ? '' : c.value) },
        } as WireChange['kind'];
        break;
      case 'font_size_hundredth_points':
        kind = {
          case: 'fontSize',
          value: { action: mapped(reset ? 0 : c.value) },
        } as WireChange['kind'];
        break;
      case 'font_bold':
      case 'font_italic':
      case 'font_strike':
      case 'wrap': {
        const name = {
          font_bold: 'fontBold',
          font_italic: 'fontItalic',
          font_strike: 'fontStrike',
          wrap: 'wrap',
        } as const;
        kind = {
          case: name[c.property],
          value: { action: mapped(reset ? false : c.value) },
        } as WireChange['kind'];
        break;
      }
      case 'font_color':
      case 'fill':
        kind = {
          case: c.property === 'fill' ? 'fill' : 'fontColor',
          value: { action: mapped(Uint8Array.from(reset ? [] : c.value)) },
        } as WireChange['kind'];
        break;
      case 'font_underline':
        kind = {
          case: 'fontUnderline',
          value: { action: mapped(reset ? 0 : { none: 1, single: 2, double: 3 }[c.value]) },
        } as WireChange['kind'];
        break;
      case 'horizontal':
        kind = {
          case: 'horizontal',
          value: {
            action: mapped(reset ? 0 : { general: 1, left: 2, center: 3, right: 4 }[c.value]),
          },
        } as WireChange['kind'];
        break;
      case 'vertical':
        kind = {
          case: 'vertical',
          value: { action: mapped(reset ? 0 : { top: 1, center: 2, bottom: 3 }[c.value]) },
        } as WireChange['kind'];
        break;
      case 'number_format':
        kind = {
          case: 'numberFormat',
          value: {
            action: mapped(
              reset
                ? 0
                : {
                    general: 1,
                    integer: 2,
                    decimal2: 3,
                    percent0: 4,
                    percent2: 5,
                    scientific2: 6,
                    date_iso: 7,
                  }[c.value],
            ),
          },
        } as WireChange['kind'];
        break;
      case 'border_top':
      case 'border_right':
      case 'border_bottom':
      case 'border_left': {
        const name = {
          border_top: 'borderTop',
          border_right: 'borderRight',
          border_bottom: 'borderBottom',
          border_left: 'borderLeft',
        } as const;
        const value = reset
          ? undefined
          : {
              line: { thin: 1, medium: 2, thick: 3, dashed: 4, dotted: 5, double: 6 }[c.value.line],
              color: Uint8Array.from(c.value.color),
            };
        kind = { case: name[c.property], value: { action: mapped(value) } } as WireChange['kind'];
        break;
      }
    }
    return create(StyleChangeSchema, { kind });
  });
  return create(StylePatchSchema, { schemaVersion: 1, profileVersion: 1, changes: wire });
}
export function colorCss(color: Rgb) {
  return `#${color.map((v) => v.toString(16).padStart(2, '0')).join('')}`;
}
export function canvasFont(style: Readonly<CellStyle>) {
  const name = style.font_family.replaceAll('\\', '\\\\').replaceAll('"', '\\"');
  return `${style.font_italic ? 'italic ' : ''}${style.font_bold ? 'bold ' : ''}${style.font_size_hundredth_points / 75}px "${name}", sans-serif`;
}
const numberFormats = new Map<string, Intl.NumberFormat>();
export function formatNumber(value: number, format: CellStyle['number_format']): string {
  if (!Number.isFinite(value)) return '#FORMAT!';
  if (format === 'general') return String(value || 0);
  if (format === 'date_iso') {
    const day = Math.floor(value);
    if (day < 1 || day > 2958465) return '#FORMAT!';
    if (day === 60) return '1900-02-29';
    return new Date(Date.UTC(1899, 11, 31) + (day < 60 ? day : day - 1) * 86400000)
      .toISOString()
      .slice(0, 10);
  }
  if (format === 'scientific2') return (value || 0).toExponential(2).replace('e', 'E');
  let formatter = numberFormats.get(format);
  if (!formatter) {
    const digits = format === 'decimal2' || format === 'percent2' ? 2 : 0;
    formatter = new Intl.NumberFormat('en-US', {
      useGrouping: false,
      style: format.startsWith('percent') ? 'percent' : 'decimal',
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    });
    numberFormats.set(format, formatter);
  }
  return formatter.format(value || 0).replace(/^-0(?=\D|$)(?=[.0%]*$)/, '0');
}
