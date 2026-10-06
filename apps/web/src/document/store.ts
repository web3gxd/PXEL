import { create, toBinary } from '@bufbuild/protobuf';
import { FORMULA_ENGINE_VERSION, PreviewClient, type PreviewStatus } from '../calculation/client';
import { copyFormulaInWorker } from '../calculation/formula-copy';
import { readHistory } from './history';
import { authorizeClipboard } from '../sharing/disclosure';
import { filteredRows, type PersonalFilters } from './filters';
import { readPersonalView, savePersonalView, type PersonalView } from './personal-view';
import { validSortBounds, type SortBounds, type SortKey } from './sort-range';
import { AssistantEngine, type Suggestion, type Observation } from '../assistant/engine';
import { fillPlan, numericSeries, type FillBounds } from '../grid/fill';
import type { HardcodeReport } from './diagnostics';
import {
  decodeCapabilities,
  decodeEncryption,
  guestPath,
  type Capabilities,
  type EncryptionInfo,
  type GuestSession,
} from '../sharing/api';
import { parseDelimited, serializeDelimited, type CellMatrix } from '../clipboard/tabular';
import {
  DEFAULT_STYLE,
  applyStyle,
  decodeStyle,
  decodeAuthoredMetadata,
  encodeStylePatch,
  formatNumber,
  isDefaultStyle,
  type CellStyle,
  type StyleChange,
  type StyledCell,
  type AuthoredMetadata,
} from './styles';
import type {
  Stamp,
  PreviewCell,
  PreviewContext,
  PreviewMoveGeometry,
  PreviewStyleMutation,
  PreviewValue,
} from '../calculation/protocol';
import {
  ClientEnvelopeSchema,
  CanonicalOperationSchema,
  CellValueSchema,
  SetCellValueSchema,
  CalculationCommittedSchema,
  BoundFormulaSchema,
} from '../generated/pxel/wire/v1/session_pb';
import type {
  CanonicalOperation,
  ServerEnvelope,
  CalculationCommitted,
} from '../generated/pxel/wire/v1/session_pb';

export type FormulaDescriptor = {
  source: string;
  current_text?: string;
  authored_source?: string;
  canonical?: string;
  ast_schema?: number;
  compatibility_profile?: string;
  bound_at_version?: string;
  bound_formula_wire_hex?: string;
};
export type Value =
  | { type: 'blank' }
  | { type: 'number'; value: number }
  | { type: 'text'; value: string }
  | { type: 'boolean'; value: boolean }
  | { type: 'formula'; value: FormulaDescriptor };
export type CalculatedValue =
  Exclude<Value, { type: 'formula' }> | { type: 'error'; value: string };
export type MergeRange = {
  first_row_id: string;
  last_row_id: string;
  first_column_id: string;
  last_column_id: string;
};
export type Sheet = {
  frozen_rows?: number;
  id: string;
  name: string;
  rows: string[];
  columns: string[];
  cells: { row_id: string; column_id: string; value: Value }[];
  styles?: StyledCell[];
  row_heights?: { axis_id: string; size_css_px: number }[];
  column_widths?: { axis_id: string; size_css_px: number }[];
  merges?: MergeRange[];
};
export type Projection = AuthoredMetadata & {
  representation?: 'anonymized';
  privacy_revision?: number;
  document_id: string;
  version: string;
  state_hash: string;
  calculation_context?: PreviewContext | null;
  sheets: Sheet[];
  capabilities?: Capabilities;
  encryption?: EncryptionInfo;
};
export type Position = { row: number; column: number };
export type RemotePresence = {
  sessionId: string;
  actorId: string;
  anchor: Position;
  focus: Position;
};
type PendingCell = {
  kind: 'cell';
  id: string;
  row: string;
  column: string;
  value: Value;
  bytes: Uint8Array;
  attempts: number;
};
type PendingBatch = {
  kind: 'batch';
  id: string;
  writes: { row: string; column: string; value: Value }[];
  styles?: { row: string; column: string; changes: StyleChange[] }[];
  bytes: Uint8Array;
  attempts: number;
};
type PendingExtension = {
  kind: 'extendRows';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingInsertion = {
  kind: 'insertRows';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingDeletion = {
  kind: 'deleteRows';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingColumnInsertion = {
  kind: 'insertColumns';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingColumnDeletion = {
  kind: 'deleteColumns';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingAddSheet = {
  kind: 'addSheet';
  id: string;
  sheetId: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingUndo = {
  kind: 'undo';
  direction: 'undo' | 'redo';
  id: string;
  target: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingRangeCopy = {
  kind: 'copyRange';
  id: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingRangeMove = {
  kind: 'moveRange';
  id: string;
  cutPayload: string;
  bytes: Uint8Array;
  attempts: number;
};
type PendingStyle = {
  kind: 'style';
  id: string;
  sheet: string;
  targets: ReadonlySet<string>;
  changes: readonly StyleChange[];
  bytes: Uint8Array;
  attempts: number;
};
type PendingAxisSize = { kind: 'axisSize'; id: string; bytes: Uint8Array; attempts: number };
type PendingMerge = { kind: 'merge'; id: string; bytes: Uint8Array; attempts: number };
type PendingSort = { kind: 'sortRange'; id: string; bytes: Uint8Array; attempts: number };
type Pending =
  | PendingCell
  | PendingBatch
  | PendingExtension
  | PendingInsertion
  | PendingDeletion
  | PendingColumnInsertion
  | PendingColumnDeletion
  | PendingAddSheet
  | PendingUndo
  | PendingRangeCopy
  | PendingRangeMove
  | PendingStyle
  | PendingAxisSize
  | PendingMerge
  | PendingSort;
export function uuidBytes(id: string) {
  return Uint8Array.from(id.replaceAll('-', '').match(/../g) ?? [], (pair) => parseInt(pair, 16));
}
export function bytesUuid(bytes: Uint8Array) {
  const s = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
  return `${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20)}`;
}
export function uuidV7() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  let time = BigInt(Date.now());
  for (let i = 5; i >= 0; i--) {
    b[i] = Number(time & 255n);
    time >>= 8n;
  }
  b[6] = 0x70 | ((b[6] ?? 0) & 15);
  b[8] = 0x80 | ((b[8] ?? 0) & 63);
  return bytesUuid(b);
}
export function parseInput(text: string): Value {
  if (text === '') return { type: 'blank' };
  if (text.startsWith('=')) return { type: 'formula', value: { source: text } };
  if (/^(TRUE|FALSE)$/i.test(text))
    return { type: 'boolean', value: text.toUpperCase() === 'TRUE' };
  if (/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(text) && Number.isFinite(Number(text)))
    return { type: 'number', value: Number(text) || 0 };
  return { type: 'text', value: text };
}
export function displayValue(value: Value | CalculatedValue | undefined) {
  if (value?.type === 'formula') return value.value.source;
  return !value || value.type === 'blank'
    ? ''
    : value.type === 'boolean'
      ? value.value
        ? 'TRUE'
        : 'FALSE'
      : String(value.value);
}
const key = (row: string, column: string) => `${row}/${column}`;
const EMPTY_FILTER_STATE = {
  hidden: new Set<number>() as ReadonlySet<number>,
  adjusted_merges: 0,
  waiting: false,
};
const rejectionText: Record<number, string> = {
  1: 'Нет прав на изменение таблицы.',
  2: 'Таблица не найдена.',
  3: 'Изменение не соответствует ограничениям таблицы.',
  4: 'Версия таблицы изменилась. Откройте ее заново.',
  5: 'Не удалось согласовать изменение. Повторите ввод.',
  6: 'Повторное изменение не совпадает с ранее отправленным.',
  7: 'Сервер занят. Повторите ввод чуть позже.',
  8: 'Хранилище временно недоступно.',
  9: 'Строка или столбец уже удалены.',
  12: 'Для вставки строк в таблицу с формулами пока нужен перенос ссылок.',
  13: 'Отмена невозможна: ячейка была изменена после вашей правки или появились новые ссылки.',
  14: 'Эту операцию пока нельзя отменить.',
  15: 'Срок отмены этой операции истек. Последние 10 000 изменений доступны для отмены.',
  16: 'Изменение слишком большое для надежной записи в журнал.',
  17: 'Источник диапазона изменился. Скопируйте его ещё раз.',
  18: 'Диапазон пересекает часть ссылки в формуле. Выберите другой диапазон перемещения.',
};
const isUuid = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
const formulaProfile = 'pxel-excel-subset-v1';
const byteLength = (text: string) => new TextEncoder().encode(text).length;
function decodeFormula(input: unknown, authored = false): FormulaDescriptor {
  const value = input as Partial<FormulaDescriptor> | undefined;
  if (
    !value ||
    typeof value.source !== 'string' ||
    !value.source.startsWith('=') ||
    value.source.length < 2 ||
    byteLength(value.source) > 32768 ||
    (value.current_text !== undefined &&
      (typeof value.current_text !== 'string' ||
        !value.current_text.startsWith('=') ||
        byteLength(value.current_text) > 32768)) ||
    typeof value.canonical !== 'string' ||
    !value.canonical.startsWith('=') ||
    byteLength(value.canonical) > 65536 ||
    value.ast_schema !== 1 ||
    value.compatibility_profile !== formulaProfile ||
    typeof value.bound_at_version !== 'string' ||
    !/^\d+$/.test(value.bound_at_version) ||
    ((authored || value.bound_formula_wire_hex !== undefined) &&
      (typeof value.bound_formula_wire_hex !== 'string' ||
        !value.bound_formula_wire_hex.length ||
        value.bound_formula_wire_hex.length > 1048576 ||
        value.bound_formula_wire_hex.length % 2 !== 0 ||
        !/^[0-9a-f]+$/.test(value.bound_formula_wire_hex)))
  )
    throw new Error('Неподдерживаемое представление формулы');
  return value.current_text && value.current_text !== value.canonical
    ? { ...value, authored_source: value.source, source: value.current_text }
    : (value as FormulaDescriptor);
}
function decodeCalculatedValue(input: unknown): CalculatedValue {
  if (!input || typeof input !== 'object') throw new Error('Некорректный результат расчета');
  const value = input as CalculatedValue;
  if (
    value.type === 'blank' ||
    (value.type === 'number' && typeof value.value === 'number' && Number.isFinite(value.value)) ||
    (value.type === 'boolean' && typeof value.value === 'boolean') ||
    (value.type === 'text' &&
      typeof value.value === 'string' &&
      byteLength(value.value) <= 32768) ||
    (value.type === 'error' &&
      [
        '#NULL!',
        '#DIV/0!',
        '#VALUE!',
        '#REF!',
        '#NAME?',
        '#NUM!',
        '#N/A',
        '#SPILL!',
        '#CYCLE!',
      ].includes(value.value))
  )
    return value;
  throw new Error('Некорректный результат расчета');
}
export function decodeProjection(input: unknown): Projection {
  const invalid = () => new Error('Ответ сервера не соответствует формату таблицы');
  if (!input || typeof input !== 'object') throw invalid();
  if ('redacted_projection_schema_version' in input) return decodeRedactedProjection(input);
  const p = input as Partial<Projection>;
  if (p.capabilities !== undefined) decodeCapabilities(p.capabilities);
  if (p.encryption !== undefined) decodeEncryption(p.encryption);
  if (
    !isUuid(p.document_id) ||
    typeof p.version !== 'string' ||
    !/^\d+$/.test(p.version) ||
    typeof p.state_hash !== 'string' ||
    !/^[0-9a-f]{64}$/.test(p.state_hash) ||
    !Array.isArray(p.sheets) ||
    p.sheets.length < 1 ||
    p.sheets.length > 64
  )
    throw invalid();
  if (p.calculation_context != null) {
    const c = p.calculation_context;
    if (
      c.schema_version !== 1 ||
      !isUuid(c.context_id) ||
      typeof c.epoch !== 'string' ||
      !/^[1-9]\d{0,19}$/.test(c.epoch) ||
      BigInt(c.epoch) > BigInt(p.version) ||
      BigInt(c.epoch) > 18446744073709551615n ||
      typeof c.effective_timestamp_utc_ms !== 'string' ||
      !/^\d{1,19}$/.test(c.effective_timestamp_utc_ms) ||
      BigInt(c.effective_timestamp_utc_ms) > 9223372036854775807n ||
      !Number.isInteger(c.utc_offset_minutes) ||
      Math.abs(c.utc_offset_minutes) > 840 ||
      !Number.isInteger(c.civil_day_since_unix_epoch) ||
      c.civil_day_since_unix_epoch < -2147483648 ||
      c.civil_day_since_unix_epoch > 2147483647 ||
      typeof c.random_seed !== 'string' ||
      !/^[0-9a-f]{64}$/.test(c.random_seed) ||
      typeof c.engine_version !== 'string' ||
      !/^[A-Za-z0-9._-]{1,64}$/.test(c.engine_version) ||
      typeof c.compatibility_profile !== 'string' ||
      !/^[A-Za-z0-9._-]{1,64}$/.test(c.compatibility_profile) ||
      !['open', 'explicit', 'timer', 'volatile_dependency', 'engine_upgrade'].includes(c.reason)
    )
      throw invalid();
    const ms = BigInt(c.effective_timestamp_utc_ms) + BigInt(c.utc_offset_minutes) * 60000n;
    const day = (ms >= 0n ? ms : ms - 86399999n) / 86400000n;
    if (day !== BigInt(c.civil_day_since_unix_epoch)) throw invalid();
  }
  const sheetIds = new Set<string>();
  const sheetNames = new Set<string>();
  const rowIds = new Set<string>();
  const columnIds = new Set<string>();
  let totalCells = 0;
  let totalStyles = 0;
  let totalAxisSizes = 0;
  let styleBytes = 0;
  for (const sheet of p.sheets) {
    if (
      !sheet ||
      !isUuid(sheet.id) ||
      typeof sheet.name !== 'string' ||
      !Array.isArray(sheet.rows) ||
      !Array.isArray(sheet.columns) ||
      !Array.isArray(sheet.cells) ||
      !sheet.rows.length ||
      !sheet.columns.length ||
      sheet.rows.length > 1000000 ||
      sheet.columns.length > 16384 ||
      sheet.cells.length > 100000
    )
      throw invalid();
    const foldedName = sheet.name.toLowerCase();
    if (
      !sheet.name ||
      Array.from(sheet.name).length > 31 ||
      sheetIds.has(sheet.id) ||
      sheetNames.has(foldedName)
    )
      throw invalid();
    sheetIds.add(sheet.id);
    sheetNames.add(foldedName);
    totalCells += sheet.cells.length;
    if ((p.authored_projection_schema_version ?? 1) < 4 && totalCells > 100000) throw invalid();
    if (
      !sheet.rows.every(isUuid) ||
      !sheet.columns.every(isUuid) ||
      new Set(sheet.rows).size !== sheet.rows.length ||
      new Set(sheet.columns).size !== sheet.columns.length ||
      sheet.rows.some((id) => rowIds.has(id)) ||
      sheet.columns.some((id) => columnIds.has(id))
    )
      throw invalid();
    sheet.rows.forEach((id) => rowIds.add(id));
    sheet.columns.forEach((id) => columnIds.add(id));
    const rows = new Set(sheet.rows),
      columns = new Set(sheet.columns),
      cells = new Set<string>();
    for (const cell of sheet.cells) {
      if (
        !cell ||
        !rows.has(cell.row_id) ||
        !columns.has(cell.column_id) ||
        cells.has(key(cell.row_id, cell.column_id))
      )
        throw invalid();
      cells.add(key(cell.row_id, cell.column_id));
      const v = cell.value;
      if (v?.type === 'formula') {
        v.value = decodeFormula(v.value, (p.authored_projection_schema_version ?? 1) >= 4);
        continue;
      }
      if (
        !v ||
        !(
          v.type === 'blank' ||
          (v.type === 'number' && typeof v.value === 'number' && Number.isFinite(v.value)) ||
          (v.type === 'text' && typeof v.value === 'string') ||
          (v.type === 'boolean' && typeof v.value === 'boolean')
        )
      )
        throw invalid();
    }
    if ((p.authored_projection_schema_version ?? 1) >= 4) {
      if (!Array.isArray(sheet.styles) || sheet.styles.length > 100000) throw invalid();
      const styled = new Set<string>();
      for (const cell of sheet.styles) {
        if (
          !cell ||
          !rows.has(cell.row_id) ||
          !columns.has(cell.column_id) ||
          styled.has(key(cell.row_id, cell.column_id)) ||
          Object.keys(cell).length !== 3
        )
          throw invalid();
        const style = decodeStyle(cell.style);
        if (isDefaultStyle(style)) throw invalid();
        cell.style = style;
        styled.add(key(cell.row_id, cell.column_id));
        cells.add(key(cell.row_id, cell.column_id));
        totalStyles++;
        styleBytes += 512 + byteLength(style.font_family);
      }
      if (cells.size > 100000 || totalStyles > 100000 || styleBytes > 64 * 1024 * 1024)
        throw invalid();
    } else if (sheet.styles !== undefined) throw invalid();
    if ((p.authored_projection_schema_version ?? 1) >= 5) {
      if (!Array.isArray(sheet.row_heights) || !Array.isArray(sheet.column_widths)) throw invalid();
      const rowsSized = new Set<string>(),
        columnsSized = new Set<string>();
      for (const entry of sheet.row_heights) {
        if (
          !entry ||
          !rows.has(entry.axis_id) ||
          rowsSized.has(entry.axis_id) ||
          !Number.isInteger(entry.size_css_px) ||
          entry.size_css_px < 12 ||
          entry.size_css_px > 1000 ||
          entry.size_css_px === 28
        )
          throw invalid();
        rowsSized.add(entry.axis_id);
        totalAxisSizes++;
      }
      for (const entry of sheet.column_widths) {
        if (
          !entry ||
          !columns.has(entry.axis_id) ||
          columnsSized.has(entry.axis_id) ||
          !Number.isInteger(entry.size_css_px) ||
          entry.size_css_px < 24 ||
          entry.size_css_px > 2000 ||
          entry.size_css_px === 112
        )
          throw invalid();
        columnsSized.add(entry.axis_id);
        totalAxisSizes++;
      }
    } else if (sheet.row_heights !== undefined || sheet.column_widths !== undefined)
      throw invalid();
    if (totalAxisSizes > 1_000_000) throw invalid();
    if ((p.authored_projection_schema_version ?? 0) >= 6) {
      if (!Array.isArray(sheet.merges) || sheet.merges.length > 10_000) throw invalid();
      const mergeCells = new Set<string>();
      let mergedCellCount = 0;
      for (const merge of sheet.merges) {
        if (
          !merge ||
          !rows.has(merge.first_row_id) ||
          !rows.has(merge.last_row_id) ||
          !columns.has(merge.first_column_id) ||
          !columns.has(merge.last_column_id)
        )
          throw invalid();
        const firstRow = sheet.rows.indexOf(merge.first_row_id),
          lastRow = sheet.rows.indexOf(merge.last_row_id),
          firstColumn = sheet.columns.indexOf(merge.first_column_id),
          lastColumn = sheet.columns.indexOf(merge.last_column_id),
          area = (lastRow - firstRow + 1) * (lastColumn - firstColumn + 1);
        if (firstRow > lastRow || firstColumn > lastColumn || area < 2 || area > 10_000)
          throw invalid();
        mergedCellCount += area;
        if (mergedCellCount > 100_000) throw invalid();
        for (let row = firstRow; row <= lastRow; row++)
          for (let column = firstColumn; column <= lastColumn; column++) {
            const cell = key(sheet.rows[row]!, sheet.columns[column]!);
            if (mergeCells.has(cell)) throw invalid();
            mergeCells.add(cell);
            if ((row !== firstRow || column !== firstColumn) && cells.has(cell)) throw invalid();
          }
      }
    } else if (sheet.merges !== undefined) throw invalid();
    if (p.authored_projection_schema_version === 7) {
      if (
        !Number.isInteger(sheet.frozen_rows) ||
        sheet.frozen_rows! < 0 ||
        sheet.frozen_rows! > 1000
      )
        throw invalid();
    } else if (sheet.frozen_rows !== undefined) throw invalid();
  }
  decodeAuthoredMetadata(p, rowIds, columnIds);
  return p as Projection;
}
export function decodeRedactedProjection(input: unknown): Projection {
  if (!input || typeof input !== 'object' || Array.isArray(input))
    throw new Error('Некорректная обезличенная проекция.');
  const p = input as Record<string, unknown>;
  const allowed = [
    'redacted_projection_schema_version',
    'style_schema_version',
    'style_profile_version',
    'document_id',
    'version',
    'privacy_revision',
    'sheets',
    'capabilities',
    'encryption',
  ];
  if (
    p.redacted_projection_schema_version !== 1 ||
    Object.keys(p).some((k) => !allowed.includes(k)) ||
    !Number.isSafeInteger(p.privacy_revision) ||
    (p.privacy_revision as number) < 0 ||
    !Array.isArray(p.sheets)
  )
    throw new Error('Неподдерживаемая обезличенная проекция.');
  const caps = decodeCapabilities(p.capabilities);
  if (!caps.anonymized || caps.can_edit)
    throw new Error('Некорректные права обезличенного представления.');
  for (const s of p.sheets) {
    if (
      !s ||
      typeof s !== 'object' ||
      !Array.isArray((s as Sheet).cells) ||
      (s as Sheet).cells.some(
        (c) =>
          !c.value ||
          !['blank', 'number', 'text', 'boolean'].includes(c.value.type) ||
          Object.keys(c).some((k) => !['row_id', 'column_id', 'value'].includes(k)) ||
          Object.keys(c.value).some((k) => !['type', 'value'].includes(k)),
      )
    )
      throw new Error('В обезличенном представлении допустимы только значения.');
  }
  const result = decodeProjection({
    authored_projection_schema_version: 7,
    style_schema_version: p.style_schema_version,
    style_profile_version: p.style_profile_version,
    document_id: p.document_id,
    version: p.version,
    state_hash: '0'.repeat(64),
    last_axis_change_version: '0',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
    sheets: p.sheets,
    capabilities: caps,
    encryption: p.encryption,
  });
  return {
    ...result,
    representation: 'anonymized',
    privacy_revision: p.privacy_revision as number,
  };
}

export class DocumentStore {
  readonly assistant = new AssistantEngine();
  autofillPreview: FillBounds | undefined;
  constructor(readonly transport: { guest?: GuestSession } = {}) {
    try {
      this.assistant.setEnabled(
        globalThis.localStorage?.getItem('pxel.assistant.enabled') !== 'false',
      );
    } catch {
      /* Storage may be unavailable. Session settings still work. */
    }
  }
  setAssistantEnabled(enabled: boolean) {
    this.assistant.setEnabled(enabled);
    try {
      globalThis.localStorage?.setItem('pxel.assistant.enabled', String(enabled));
    } catch {
      /* Session setting remains effective. */
    }
    this.notify();
  }
  observeAssistant(action: Observation) {
    if (!this.privacyMask && this.canEdit) this.assistant.observe(action);
  }
  get assistantSuggestion() {
    if (
      !this.assistant.enabled ||
      !this.canEdit ||
      this.privacyMask ||
      this.editing ||
      this.pending.length
    )
      return undefined;
    return this.assistant.suggest((position) => this.value(position));
  }
  applyAssistantSuggestion(tip: Suggestion, count: number) {
    if (
      !this.assistant.enabled ||
      !tip.source ||
      !tip.axis ||
      !this.sheet ||
      !Number.isInteger(count) ||
      count < 1 ||
      count > 1000
    )
      return false;
    const current = this.assistantSuggestion;
    if (
      !current ||
      current.id !== tip.id ||
      current.axis !== tip.axis ||
      JSON.stringify(current.source) !== JSON.stringify(tip.source)
    )
      return false;
    const source =
      tip.id === 'repeat'
        ? {
            startRow: tip.source.endRow,
            endRow: tip.source.endRow,
            startColumn: tip.source.endColumn,
            endColumn: tip.source.endColumn,
          }
        : tip.source;
    const target = {
      row: source.endRow + (tip.axis === 'row' ? count : 0),
      column: source.endColumn + (tip.axis === 'column' ? count : 0),
    };
    if (target.row >= this.sheet.rows.length || target.column >= this.sheet.columns.length)
      return false;
    for (let i = 1; i <= count; i++) {
      const value = this.value({
        row: source.endRow + (tip.axis === 'row' ? i : 0),
        column: source.endColumn + (tip.axis === 'column' ? i : 0),
      });
      if (value && value.type !== 'blank') {
        this.error = 'Помощник заполняет только пустые ячейки. Выберите другой диапазон.';
        this.notify();
        return false;
      }
    }
    const ok = this.autofill(source, target, this.projection?.version ?? '');
    if (ok) this.assistant.reset(false);
    return ok;
  }
  personalView: PersonalView = { schema_version: 1, revision: 0, settings: { filters: [] } };
  viewLoading = false;
  viewError = '';
  private viewGeneration = 0;
  private viewDocument: string | undefined;
  private filterCache:
    | {
        sheet: Sheet;
        version: string;
        calculated: string | undefined;
        settings: PersonalFilters;
        value: { hidden: ReadonlySet<number>; adjusted_merges: number; waiting: boolean };
      }
    | undefined;
  confirmedScalar(position: Position): CalculatedValue | undefined {
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    if (!row || !column) return undefined;
    const value = this.confirmed.get(key(row, column));
    return value?.type === 'formula'
      ? this.calculatedVersion === this.projection?.version
        ? this.calculated.get(key(row, column))
        : undefined
      : value;
  }
  get filterState() {
    const sheet = this.sheet,
      version = this.projection?.version ?? '0',
      settings = this.personalView.settings;
    if (!sheet) return { hidden: new Set<number>(), adjusted_merges: 0, waiting: false };
    if (!settings.filters.some((filter) => filter.sheet_id === sheet.id)) return EMPTY_FILTER_STATE;
    const cached = this.filterCache;
    if (
      cached?.sheet === sheet &&
      cached.version === version &&
      cached.calculated === this.calculatedVersion &&
      cached.settings === settings
    )
      return cached.value;
    const filteredColumns = new Set(
      settings.filters
        .filter((f) => f.sheet_id === sheet.id)
        .flatMap((f) => f.conditions.map((c) => c.column_id)),
    );
    const waiting =
      this.calculatedVersion !== version &&
      sheet.cells.some((c) => filteredColumns.has(c.column_id) && c.value.type === 'formula');
    const result = waiting
      ? { hidden: new Set<number>(), adjusted_merges: 0 }
      : filteredRows(sheet, settings, (r, c) => this.confirmedScalar({ row: r, column: c }));
    const value = { ...result, waiting };
    this.filterCache = { sheet, version, calculated: this.calculatedVersion, settings, value };
    return value;
  }
  async reloadPersonalView() {
    const doc = this.projection?.document_id;
    if (!doc || !this.projection?.capabilities || this.guestAccessClosed) return;
    const generation = ++this.viewGeneration;
    this.viewDocument = doc;
    this.viewLoading = true;
    this.viewError = '';
    this.notify();
    try {
      const view = await readPersonalView(this.documentUrl(doc, '/personal-view'));
      if (
        this.viewGeneration === generation &&
        this.projection?.document_id === doc &&
        !this.guestAccessClosed
      )
        this.personalView = view;
    } catch (error) {
      if (this.viewGeneration === generation)
        this.viewError =
          error instanceof Error ? error.message : 'Не удалось загрузить личные фильтры.';
    } finally {
      if (this.viewGeneration === generation) {
        this.viewLoading = false;
        this.notify();
      }
    }
  }
  async saveFilters(settings: PersonalFilters) {
    const doc = this.projection?.document_id,
      version = Number(this.projection?.version);
    if (
      !doc ||
      !Number.isSafeInteger(version) ||
      this.viewLoading ||
      this.pending.length ||
      this.editing ||
      this.guestAccessClosed
    )
      throw new Error('Дождитесь сохранения книги.');
    const generation = ++this.viewGeneration;
    this.viewLoading = true;
    this.viewError = '';
    this.notify();
    try {
      const view = await savePersonalView(
        this.documentUrl(doc, '/personal-view'),
        this.personalView.revision,
        version,
        { ...this.personalView.settings, ...settings },
      );
      if (
        this.projection?.document_id !== doc ||
        this.viewGeneration !== generation ||
        this.guestAccessClosed
      )
        throw new Error('Книга изменилась.');
      this.personalView = view;
    } finally {
      if (this.viewGeneration === generation) {
        this.viewLoading = false;
        this.notify();
      }
    }
  }
  setFrozenRows(rows: number) {
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (
      !this.canEdit ||
      !sheet ||
      !projection ||
      !actor ||
      !this.sender ||
      this.pending.length ||
      this.editing ||
      !Number.isInteger(rows) ||
      rows < 0 ||
      rows > Math.min(1000, sheet.rows.length)
    )
      return false;
    if (rows === (sheet.frozen_rows ?? 0)) return true;
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            setFrozenRows: { sheetId: uuidBytes(sheet.id), rows },
          },
        },
      }),
    );
    this.pending.push({ kind: 'axisSize', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  guestAccessClosed = false;
  private accessListeners = new Set<() => void>();
  get isGuest() {
    return this.transport.guest !== undefined;
  }
  get allRowsHidden() {
    return !!this.sheet?.rows.length && this.filterState.hidden.size === this.sheet.rows.length;
  }
  get canEdit() {
    return this.canAuthor && !this.allRowsHidden;
  }
  get canAuthor() {
    return (
      !this.creating &&
      this.projection?.representation !== 'anonymized' &&
      !this.guestAccessClosed &&
      (!this.isGuest || this.transport.guest?.permission === 'edit') &&
      (this.projection?.capabilities?.can_edit ?? true)
    );
  }
  get canComment() {
    return (
      !this.guestAccessClosed &&
      (!this.isGuest || this.transport.guest?.permission !== 'view') &&
      (this.projection?.capabilities?.can_comment ?? true)
    );
  }
  get canShare() {
    return !this.isGuest && this.projection?.capabilities?.can_share === true;
  }
  get canExport() {
    return !this.guestAccessClosed && (this.projection?.capabilities?.can_export ?? false);
  }
  get canCopy() {
    return (
      !this.guestAccessClosed &&
      !this.allRowsHidden &&
      (this.projection?.capabilities?.can_copy ?? !this.isGuest)
    );
  }
  sortRangePreview: SortBounds | undefined;
  hardcodeReport: HardcodeReport | undefined;
  get selectedHardcodeWarning() {
    if (this.allRowsHidden) return undefined;
    if (this.hardcodeReport?.document_version !== this.projection?.version) return undefined;
    const sheet = this.sheet,
      dismissed = new Set(this.personalView.settings.hardcode_dismissals ?? []);
    return this.hardcodeReport?.warnings.find(
      (warning) =>
        warning.sheet_id === sheet?.id &&
        warning.row_id === sheet.rows[this.selected.row] &&
        warning.column_id === sheet.columns[this.selected.column] &&
        !dismissed.has(warning.fingerprint),
    );
  }
  confirmedCells(sheet: Sheet): Sheet['cells'] {
    const rows = new Set(sheet.rows),
      columns = new Set(sheet.columns);
    return [...this.confirmed].flatMap(([address, value]) => {
      const [row_id = '', column_id = ''] = address.split('/');
      return rows.has(row_id) && columns.has(column_id) ? [{ row_id, column_id, value }] : [];
    });
  }
  previewSortBounds(bounds?: SortBounds) {
    const preview =
      this.sheet && bounds && validSortBounds(this.sheet, bounds) ? { ...bounds } : undefined;
    if (JSON.stringify(preview) === JSON.stringify(this.sortRangePreview)) return;
    this.sortRangePreview = preview;
    this.notify();
  }
  sortRange(
    bounds: SortBounds,
    headerRows: number,
    keys: readonly SortKey[],
    expectedVersion: string,
  ) {
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (
      !this.canAuthor ||
      !sheet ||
      !projection ||
      !actor ||
      this.pending.length ||
      this.editing ||
      projection.version !== expectedVersion ||
      !validSortBounds(sheet, bounds) ||
      !Number.isInteger(headerRows) ||
      headerRows < 0 ||
      headerRows > bounds.endRow - bounds.startRow ||
      !keys.length ||
      keys.length > 50 ||
      new Set(keys.map((k) => k.column)).size !== keys.length ||
      keys.some(
        (k) =>
          !Number.isInteger(k.column) ||
          k.column < bounds.startColumn ||
          k.column > bounds.endColumn,
      )
    )
      return false;
    const keyColumns = new Set(keys.map((key) => sheet.columns[key.column]));
    const rowPositions = new Map(sheet.rows.map((row, index) => [row, index]));
    const formulaKey = this.confirmedCells(sheet).some(
      (cell) =>
        cell.value.type === 'formula' &&
        keyColumns.has(cell.column_id) &&
        (rowPositions.get(cell.row_id) ?? -1) >= bounds.startRow + headerRows &&
        (rowPositions.get(cell.row_id) ?? -1) <= bounds.endRow,
    );
    if (formulaKey && this.calculatedVersion !== projection.version) return false;
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            sortRange: {
              schemaVersion: 1,
              source: {
                sheetId: uuidBytes(sheet.id),
                rowId: uuidBytes(sheet.rows[bounds.startRow]!),
                columnId: uuidBytes(sheet.columns[bounds.startColumn]!),
              },
              rows: bounds.endRow - bounds.startRow + 1,
              columns: bounds.endColumn - bounds.startColumn + 1,
              headerRows,
              keys: keys.map((key) => ({
                columnId: uuidBytes(sheet.columns[key.column]!),
                descending: key.descending,
              })),
            },
          },
        },
      }),
    );
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'sortRange', id, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  async authorizeClipboard(action: 'copy' | 'cut', anonymize = false, text?: string) {
    const projection = this.projection;
    const policyStamp = projection?.capabilities?.disclosure_stamp;
    if (!projection || !policyStamp) throw new Error('Дождитесь загрузки прав копирования.');
    const documentVersion = Number(projection.version);
    if (!Number.isSafeInteger(documentVersion))
      throw new Error('Версия недоступна для копирования.');
    const stamp = { ...policyStamp, document_version: documentVersion };
    const version = projection.version;
    const receipt = await authorizeClipboard(
      this.documentUrl(projection.document_id, '/clipboard/authorize'),
      anonymize || this.privacyMask ? 'copy_anonymized' : action,
      stamp,
      text,
    );
    if (
      this.projection?.document_id !== projection.document_id ||
      this.projection.version !== version ||
      this.pending.length ||
      this.guestAccessClosed
    )
      throw new Error('Версия таблицы изменилась. Повторите копирование.');
    if (!this.canCopy || (action === 'cut' && !this.canEdit))
      throw new Error('Копирование запрещено владельцем.');
    const { projection: raw, ...rest } = receipt;
    return { ...rest, ...(raw === undefined ? {} : { projection: decodeRedactedProjection(raw) }) };
  }
  documentUrl(documentId: string, suffix = '') {
    const guest = this.transport.guest;
    if (guest) {
      if (documentId !== guest.documentId) throw new Error('Ответ относится к другой таблице');
      return `${guestPath(guest.linkId)}${suffix || '/document'}`;
    }
    return `/api/documents/${encodeURIComponent(documentId)}${suffix}`;
  }
  onGuestAccessClosed(listener: () => void) {
    this.accessListeners.add(listener);
    return () => this.accessListeners.delete(listener);
  }
  closeGuestAccess() {
    if (!this.isGuest || this.guestAccessClosed) return;
    this.guestAccessClosed = true;
    this.dispose();
    this.actor = undefined;
    this.projection = undefined;
    this.activeSheetId = undefined;
    this.confirmed.clear();
    this.confirmedStyles.clear();
    this.calculated.clear();
    this.lastDisplayedResults.clear();
    this.calculatedVersion = undefined;
    this.calculationStatus = 'idle';
    this.calculationError = '';
    this.pending = [];
    this.quarantinedPending = undefined;
    this.undoHistory = [];
    this.redoHistory = [];
    this.historyVersions.clear();
    this.historyDirty = false;
    this.historyLoading = false;
    this.editing = undefined;
    this.cutIntent = undefined;
    this.movePreview = undefined;
    this.previewOverlay.clear();
    this.previewDirty.clear();
    this.previewStyleOverlay.clear();
    this.previewSheetByRow.clear();
    this.authoredPreviewHash = undefined;
    this.submittedAt.clear();
    this.mergeCache.clear();
    this.loading = false;
    this.recalculating = false;
    this.exporting = false;
    this.error = 'Доступ по ссылке закрыт. Обратитесь к владельцу таблицы.';
    this.notify();
    this.accessListeners.forEach((listener) => listener());
  }
  private async fetchDocument(path: string, init?: RequestInit) {
    if (this.guestAccessClosed) throw new Error('Доступ по ссылке закрыт.');
    const response = await fetch(path, { ...init, credentials: 'same-origin' });
    if (this.isGuest && [401, 403, 404].includes(response.status)) this.closeGuestAccess();
    return response;
  }
  previewStatus: PreviewStatus = 'idle';
  private previewValues: ReadonlyMap<string, CalculatedValue> = new Map();
  private previewStamp: Stamp | undefined;
  private previewRevision = 0n;
  private previewOverlay = new Set<string>();
  private previewDirty = new Set<string>();
  private previewSheetByRow = new Map<string, string>();
  private movePreview:
    | { operationId: string; stamp: Stamp; writes: ReadonlyMap<string, PreviewValue> | undefined }
    | undefined;
  private previewStyleOverlay = new Map<string, Set<keyof CellStyle>>();
  private authoredPreviewHash: string | undefined;
  get previewAuthoredHash() {
    return this.previewStatus === 'ready' && this.matchesPreview(this.previewStamp)
      ? this.authoredPreviewHash
      : undefined;
  }
  private preview = new PreviewClient((status, values, stamp, moveWrites, authoredHash) => {
    if (
      status === 'ready' &&
      this.matchesPreview(stamp) &&
      authoredHash &&
      !this.editing &&
      !this.pending.length &&
      authoredHash !== this.projection?.state_hash
    ) {
      this.previewStatus = 'failed';
      this.previewValues = new Map();
      this.authoredPreviewHash = undefined;
      this.previewStamp = undefined;
      this.notify();
      return;
    }
    this.previewStatus = status;
    this.previewValues = values ?? new Map();
    this.previewStamp = stamp;
    this.authoredPreviewHash =
      status === 'ready' && this.matchesPreview(stamp) ? authoredHash : undefined;
    if (this.movePreview) {
      this.movePreview.writes =
        status === 'ready' &&
        this.matchesPreview(stamp) &&
        this.movePreview.stamp.pending_revision === stamp?.pending_revision &&
        this.pending.some(
          (pending) => pending.kind === 'moveRange' && pending.id === this.movePreview?.operationId,
        )
          ? moveWrites
          : undefined;
    }
    this.notify();
  });
  projection: Projection | undefined;
  privacyAnalysisPending = false;
  privacyAnalysisError = '';
  privacyReport: import('./privacy').PrivacyReport | undefined;
  privacyPolicy: import('./privacy').PrivacyPolicy | undefined;
  private privacyMaskProjection: Projection | undefined;
  private privacyMaskEnabled = true;
  private privacyMaskCells = new Map<string, Value>();
  get privacyMask() {
    return this.privacyMaskProjection;
  }
  set privacyMask(value: Projection | undefined) {
    this.privacyMaskProjection = value;
    this.privacyMaskCells = new Map(
      value?.sheets.flatMap((s) =>
        s.cells.map((c) => [`${s.id}/${c.row_id}/${c.column_id}`, c.value] as const),
      ) ?? [],
    );
  }
  displaySheetName(sheetId = this.activeSheetId) {
    return (
      this.privacyMask?.sheets.find((s) => s.id === sheetId)?.name ??
      this.projection?.sheets.find((s) => s.id === sheetId)?.name ??
      'Пустой лист'
    );
  }
  private privacyFindingList: { sheet_id: string; row_id: string; column_id: string }[] = [];
  private privacyFindingKeys = new Set<string>();
  privacyFindingRevision = 0;
  get privacyFindings() {
    return this.privacyFindingList;
  }
  set privacyFindings(value: { sheet_id: string; row_id: string; column_id: string }[]) {
    this.privacyFindingList = value;
    this.privacyFindingKeys = new Set(value.map((f) => `${f.sheet_id}/${f.row_id}/${f.column_id}`));
    this.privacyFindingRevision++;
  }
  setPrivacyMask(projection: Projection | undefined) {
    if (this.pending.length || this.editing) throw new Error('Сначала завершите изменения.');
    this.validatePrivacyMask(projection);
    this.privacyMaskEnabled = !!projection;
    this.privacyMask = projection;
    this.notify();
  }
  private validatePrivacyMask(projection: Projection | undefined) {
    if (
      projection &&
      (projection.representation !== 'anonymized' ||
        projection.document_id !== this.projection?.document_id ||
        projection.version !== this.projection.version)
    )
      throw new Error('Локальная маска устарела.');
  }
  refreshPrivacyMask(projection: Projection | undefined) {
    this.validatePrivacyMask(projection);
    this.privacyMask = this.privacyMaskEnabled ? projection : undefined;
    this.notify();
  }
  privacyFindingAt(position: Position) {
    const sheet = this.sheet;
    return this.privacyFindingKeys.has(
      `${sheet?.id}/${sheet?.rows[position.row]}/${sheet?.columns[position.column]}`,
    );
  }
  privacyVeilAt(position: Position) {
    if (
      !this.privacyMask ||
      this.isGuest ||
      !this.canShare ||
      this.projection?.representation === 'anonymized' ||
      this.projection?.capabilities?.is_guest
    )
      return false;
    const value = this.value(position);
    return (
      this.privacyMask.version === this.projection?.version &&
      value?.type === 'text' &&
      value.value.includes('[СКРЫТО')
    );
  }
  privacyOriginalDisplay(position: Position) {
    if (!this.privacyVeilAt(position)) return undefined;
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    if (!row || !column) return undefined;
    const original = this.confirmed.get(key(row, column));
    if (original?.type === 'formula') {
      if (
        this.calculatedVersion !== this.projection?.version ||
        this.calculationStatus !== 'complete'
      )
        return undefined;
      return displayValue(this.calculated.get(key(row, column)));
    }
    return displayValue(original);
  }
  activeSheetId: string | undefined;
  confirmed = new Map<string, Value>();
  confirmedStyles = new Map<string, CellStyle>();
  pending: Pending[] = [];
  private quarantinedPending: { actor: string; document: string; intents: Pending[] } | undefined;
  private get pendingCount() {
    return this.pending.length + (this.quarantinedPending?.intents.length ?? 0);
  }
  private undoHistory: string[] = [];
  private redoHistory: string[] = [];
  private historyVersions = new Map<string, bigint>();
  private historyThrough = 0n;
  // Only classified events or discovered history advance this watermark.
  // HTTP projection refresh does not reveal authorship of skipped versions.
  private historyObservedThrough = 0n;
  private historyRequiredThrough = 0n;
  private historyDirty = true;
  private historyDenied = false;
  private historyLoading = false;
  private historyGeneration = 0;
  private historyAttempts = 0;
  private historyError = '';
  private historyAbort: AbortController | undefined;
  private historyTimer: ReturnType<typeof setTimeout> | undefined;
  selected: Position = { row: 0, column: 0 };
  selectionAnchor: Position = { row: 0, column: 0 };
  private selectionAxis: { kind: 'column'; index: number } | undefined;
  remotePresence: RemotePresence[] = [];
  editing: { position: Position; text: string; source: 'cell' | 'bar' } | undefined;
  // Opening the leave-confirmation dialog must not implicitly submit a draft.
  navigationGuardActive = false;
  error = '';
  loading = false;
  exporting = false;
  recalculating = false;
  calculated = new Map<string, CalculatedValue>();
  // Presentation fallback only: never a calculated projection of the new head.
  private lastDisplayedResults = new Map<string, CalculatedValue>();
  calculatedVersion: string | undefined;
  calculationStatus: 'idle' | 'pending' | 'complete' | 'failed' = 'idle';
  calculationError = '';
  readonly performanceMetrics = {
    optimisticUpdateMs: 0,
    serverConfirmationMs: 0,
    calculationPatchMs: 0,
    recalculatedCells: 0,
    dirtyCells: 0,
  };
  private readonly submittedAt = new Map<string, number>();
  private calculationStartedAt = 0;
  private calculationTimer: ReturnType<typeof setTimeout> | undefined;
  private calculationRequestedVersion: string | undefined;
  private calculationAssembly:
    | {
        nextSequence: number;
        bytes: number;
        values: Map<string, CalculatedValue>;
        seen: Set<string>;
        patchCells: number;
        fullSnapshot: boolean;
        baseInputVersion: bigint | undefined;
        expectedFormulaCount: number;
        evaluatedCells: number;
        dirtyCells: number;
      }
    | undefined;
  private revision = 0;
  private listeners = new Set<() => void>();
  private mergeCacheSheetId = '';
  private mergeCacheVersion = '';
  private mergeCacheMerges: MergeRange[] | undefined;
  private mergeCache = new Map<
    string,
    { startRow: number; endRow: number; startColumn: number; endColumn: number }
  >();
  private actor: Uint8Array | undefined;
  private sender: ((bytes: Uint8Array) => boolean) | undefined;
  private subscribedDocument: string | undefined;
  private presenceTimer: ReturnType<typeof setTimeout> | undefined;
  private headRefreshTimer: ReturnType<typeof setTimeout> | undefined;
  private headRefreshVersion = 0n;
  private headRefreshAttempts = 0;
  private inflight: string | undefined;
  private formulaCopyInFlight = false;
  private cutIntent: { raw: string; version: string; stale: boolean } | undefined;
  private sequence = 0n;
  private syncGeneration = 0;
  private connectionGeneration = 0;
  private syncing = false;
  private creation: { document_id: string; operation_id: string } | undefined;
  private creating = false;
  private recalculationRequest: { document_id: string; operation_id: string } | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  snapshot = () => this.revision;
  notify = () => {
    const hidden = this.filterState.hidden;
    if (hidden.has(this.selected.row) && this.sheet) {
      let row = this.selected.row;
      while (row < this.sheet.rows.length - 1 && hidden.has(row)) row++;
      while (row > 0 && hidden.has(row)) row--;
      if (!hidden.has(row)) {
        this.selected = { ...this.selected, row };
        this.selectionAnchor = { ...this.selected };
      }
    }
    this.revision++;
    this.listeners.forEach((fn) => fn());
  };
  get sheet() {
    return (
      this.projection?.sheets.find((sheet) => sheet.id === this.activeSheetId) ??
      this.projection?.sheets[0]
    );
  }
  private pruneHistory() {
    const head = BigInt(this.projection?.version ?? '0');
    const keep = (id: string) => {
      const version = this.historyVersions.get(id);
      return version !== undefined && head - version <= 10000n;
    };
    this.undoHistory = this.undoHistory.filter(keep);
    this.redoHistory = this.redoHistory.filter(keep);
    const retained = new Set([...this.undoHistory, ...this.redoHistory]);
    for (const id of this.historyVersions.keys())
      if (!retained.has(id)) this.historyVersions.delete(id);
  }
  private scheduleHistory(delay = 0) {
    clearTimeout(this.historyTimer);
    if (!this.canEdit) return;
    if (
      !this.historyDirty ||
      this.historyDenied ||
      !this.sender ||
      this.pending.length ||
      this.historyLoading ||
      this.syncing ||
      this.headRefreshVersion > BigInt(this.projection?.version ?? '0')
    )
      return;
    this.historyTimer = setTimeout(() => void this.discoverHistory(), delay);
  }
  private dirtyHistory(through = BigInt(this.projection?.version ?? '0')) {
    this.historyDenied = false;
    if (this.historyDirty && through <= this.historyRequiredThrough) {
      this.scheduleHistory();
      return;
    }
    this.historyDirty = true;
    this.historyRequiredThrough =
      this.historyRequiredThrough > through ? this.historyRequiredThrough : through;
    this.historyGeneration++;
    this.historyAbort?.abort();
    this.scheduleHistory();
  }
  private observeHistoryVersion(version: bigint) {
    if (version > this.historyObservedThrough + 1n) this.dirtyHistory(version);
    if (version > this.historyObservedThrough) this.historyObservedThrough = version;
  }
  private async discoverHistory() {
    clearTimeout(this.historyTimer);
    if (!this.canEdit) return;
    const document = this.projection?.document_id;
    const actor = this.actor && bytesUuid(this.actor);
    if (
      !this.historyDirty ||
      this.historyDenied ||
      this.historyLoading ||
      !document ||
      !actor ||
      !this.sender ||
      this.pending.length ||
      this.syncing ||
      this.headRefreshVersion > BigInt(this.projection?.version ?? '0')
    )
      return;
    const generation = this.historyGeneration,
      connection = this.connectionGeneration,
      sequence = this.sequence;
    const current = () =>
      generation === this.historyGeneration &&
      connection === this.connectionGeneration &&
      document === this.projection?.document_id &&
      actor === (this.actor && bytesUuid(this.actor));
    this.historyLoading = true;
    const abort = new AbortController();
    this.historyAbort = abort;
    this.notify();
    let retry = 0;
    let denied = false;
    try {
      const response = await this.fetchDocument(this.documentUrl(document, '/undo-history'), {
        signal: AbortSignal.any([abort.signal, AbortSignal.timeout(10000)]),
      });
      if (!response.ok) {
        denied = response.status === 401 || response.status === 403;
        throw new Error(
          denied ? 'Нет права на историю правок.' : 'Не удалось загрузить историю правок.',
        );
      }
      const history = await readHistory(response);
      if (!current()) return;
      if (history.document_id !== document || history.actor_id !== actor)
        throw new Error('История относится к другой таблице или пользователю.');
      if (sequence !== this.sequence || this.pending.length) return;
      const through = BigInt(history.through_version);
      if (through < this.historyRequiredThrough || through < this.historyThrough)
        throw new Error('История правок ещё не обновилась.');
      if (through > BigInt(this.projection?.version ?? '0') && !(await this.refresh(through)))
        throw new Error('Не удалось согласовать историю с таблицей.');
      if (!current() || sequence !== this.sequence || this.pending.length) return;
      this.undoHistory = history.undo.map((entry) => entry.operation_id);
      this.redoHistory = history.redo.map((entry) => entry.operation_id);
      this.historyVersions = new Map(
        [...history.undo, ...history.redo].map((entry) => [
          entry.operation_id,
          BigInt(entry.committed_version),
        ]),
      );
      this.historyThrough = through;
      if (through > this.historyObservedThrough) this.historyObservedThrough = through;
      this.historyDirty = false;
      this.historyAttempts = 0;
      if (this.error === this.historyError) this.error = '';
      this.historyError = '';
      this.pruneHistory();
    } catch (error) {
      if (current()) {
        this.historyDenied = denied;
        this.historyError =
          error instanceof Error ? error.message : 'Не удалось загрузить историю правок.';
        this.error = this.historyError;
        retry = Math.min(30000, 1000 * 2 ** Math.min(this.historyAttempts++, 5));
      }
    } finally {
      this.historyLoading = false;
      if (this.historyAbort === abort) this.historyAbort = undefined;
      if (!denied || !current()) this.scheduleHistory(retry);
      this.notify();
    }
  }
  selectSheet(sheetId: string) {
    if (this.sheet?.id === sheetId) return true;
    if (!this.projection?.sheets.some((sheet) => sheet.id === sheetId)) return false;
    if (this.editing) this.commit();
    if (this.editing) return false;
    this.activeSheetId = sheetId;
    this.assistant.reset();
    this.selected = { row: 0, column: 0 };
    this.selectionAnchor = { row: 0, column: 0 };
    this.selectionAxis = undefined;
    this.remotePresence = [];
    this.syncPreview(true);
    this.schedulePresence();
    this.notify();
    return true;
  }
  value(position: Position) {
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    if (!row || !column) return undefined;
    if (this.privacyMask) {
      if (this.privacyMask.version !== this.projection?.version)
        return { type: 'text' as const, value: '[МАСКА УСТАРЕЛА]' };
      return this.privacyMaskCells.get(`${this.sheet?.id}/${row}/${column}`);
    }
    return this.authoredValue(position);
  }
  private authoredValue(position: Position) {
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    if (!row || !column) return undefined;
    for (let i = this.pending.length - 1; i >= 0; i--) {
      const op = this.pending[i];
      if (op?.kind === 'cell' && op.row === row && op.column === column) return op.value;
      if (op?.kind === 'batch') {
        const write = op.writes.find((item) => item.row === row && item.column === column);
        if (write) return write.value;
      }
    }
    return this.movePreviewValue(row, column) ?? this.confirmed.get(key(row, column));
  }
  style(position: Position): Readonly<CellStyle> {
    const sheet = this.sheet,
      row = sheet?.rows[position.row],
      column = sheet?.columns[position.column];
    if (!sheet || !row || !column) return DEFAULT_STYLE;
    return this.styleAt(sheet.id, row, column);
  }
  private styleAt(sheetId: string, row: string, column: string): Readonly<CellStyle> {
    const cellKey = key(row, column);
    let result = this.confirmedStyles.get(`${sheetId}/${cellKey}`) ?? DEFAULT_STYLE;
    for (const pending of this.pending)
      if (pending.kind === 'style' && pending.sheet === sheetId && pending.targets.has(cellKey))
        result = applyStyle(result, pending.changes);
      else if (
        pending.kind === 'batch' &&
        pending.styles?.some((write) => key(write.row, write.column) === cellKey)
      ) {
        const write = pending.styles.find((item) => key(item.row, item.column) === cellKey);
        if (write) result = applyStyle(result, write.changes);
      }
    return result;
  }
  display(position: Position) {
    const value = this.displayContent(position);
    return value?.type === 'number'
      ? formatNumber(value.value, this.style(position).number_format)
      : displayValue(value);
  }
  displayContent(position: Position): Value | CalculatedValue | undefined {
    if (this.privacyMask) return this.value(position);
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    const value = row && column ? this.previewValue(row, column) : this.value(position);
    if (value?.type !== 'formula') return value;
    if (
      row &&
      column &&
      this.previewStatus === 'ready' &&
      this.matchesPreview(this.previewStamp) &&
      (this.editing || this.pending.length || this.calculationStatus !== 'complete')
    ) {
      const preview = this.previewValues.get(`${this.sheet?.id}/${row}/${column}`);
      if (preview) return preview;
    }
    if (
      !row ||
      !column ||
      this.movePreviewValue(row, column) ||
      this.pending.some(
        (pending) =>
          (pending.kind === 'cell' && pending.row === row && pending.column === column) ||
          (pending.kind === 'batch' &&
            pending.writes.some((item) => item.row === row && item.column === column)),
      )
    )
      return row && column ? this.lastDisplayedResults.get(key(row, column)) : undefined;
    if (
      this.calculatedVersion !== this.projection?.version ||
      this.calculationStatus !== 'complete'
    )
      return this.lastDisplayedResults.get(key(row, column));
    return this.calculated.get(key(row, column));
  }
  get previewVisible() {
    return (
      this.previewStatus === 'ready' &&
      this.matchesPreview(this.previewStamp) &&
      (this.previewValues.size > 0 || (this.activeMovePreview?.writes?.size ?? 0) > 0) &&
      Boolean(this.editing || this.pending.length || this.calculationStatus !== 'complete')
    );
  }
  private matchesPreview(stamp: Stamp | undefined) {
    return Boolean(
      stamp &&
      stamp.document_id === this.projection?.document_id &&
      stamp.input_version === this.projection.version &&
      stamp.pending_revision === String(this.previewRevision),
    );
  }
  private get activeMovePreview() {
    const preview = this.movePreview;
    return this.previewStatus === 'ready' &&
      preview &&
      this.matchesPreview(preview.stamp) &&
      this.matchesPreview(this.previewStamp) &&
      this.pending.some(
        (pending) => pending.kind === 'moveRange' && pending.id === preview.operationId,
      )
      ? preview
      : undefined;
  }
  private movePreviewValue(row: string, column: string): Value | undefined {
    return this.activeMovePreview?.writes?.get(
      `${this.previewSheetByRow.get(row)}/${row}/${column}`,
    );
  }
  private startMovePreview(operationId: string, geometry: PreviewMoveGeometry) {
    const projection = this.projection;
    if (!projection || !this.pending.some((pending) => pending.id === operationId)) return;
    const stamp = {
      document_id: projection.document_id,
      input_version: projection.version,
      pending_revision: String(++this.previewRevision),
    };
    this.movePreview = { operationId, stamp, writes: undefined };
    if (!this.preview.move(stamp, geometry)) this.movePreview = undefined;
  }
  private previewValue(row: string, column: string): Value {
    const edit = this.editing;
    if (
      edit &&
      this.sheet?.rows[edit.position.row] === row &&
      this.sheet?.columns[edit.position.column] === column
    )
      return parseInput(edit.text);
    for (let i = this.pending.length - 1; i >= 0; i--) {
      const pending = this.pending[i];
      if (pending?.kind === 'cell' && pending.row === row && pending.column === column)
        return pending.value;
      if (pending?.kind === 'batch') {
        const write = pending.writes.find((item) => item.row === row && item.column === column);
        if (write) return write.value;
      }
    }
    return (
      this.movePreviewValue(row, column) ??
      this.confirmed.get(key(row, column)) ?? { type: 'blank' }
    );
  }
  private syncPreview(reset = false) {
    if (this.projection?.representation === 'anonymized') {
      this.previewStatus = 'idle';
      return;
    }
    // A move mutates only the Worker's private candidate. Any later ordinary
    // preview must rebuild from confirmed content, never from that candidate.
    if (this.movePreview) {
      this.movePreview = undefined;
      reset = true;
    }
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection) return;
    if (reset) {
      this.previewSheetByRow = new Map(
        projection.sheets.flatMap((item) => item.rows.map((row) => [row, item.id] as const)),
      );
    }
    const overlay = new Set<string>();
    for (const pending of this.pending) {
      if (pending.kind === 'cell') overlay.add(key(pending.row, pending.column));
      if (pending.kind === 'batch')
        for (const write of pending.writes) overlay.add(key(write.row, write.column));
    }
    if (this.editing) {
      const row = sheet.rows[this.editing.position.row],
        column = sheet.columns[this.editing.position.column];
      if (row && column) overlay.add(key(row, column));
    }
    const touched = new Set([...this.previewOverlay, ...overlay, ...this.previewDirty]);
    const styleOverlay = new Map<string, Set<keyof CellStyle>>();
    for (const pending of this.pending)
      if (pending.kind === 'style')
        for (const target of pending.targets) {
          const address = `${pending.sheet}/${target}`,
            leaves = styleOverlay.get(address) ?? new Set<keyof CellStyle>();
          for (const change of pending.changes) leaves.add(change.property);
          styleOverlay.set(address, leaves);
        }
      else if (pending.kind === 'batch')
        for (const write of pending.styles ?? []) {
          const address = `${sheet.id}/${key(write.row, write.column)}`;
          const leaves = styleOverlay.get(address) ?? new Set<keyof CellStyle>();
          for (const change of write.changes) leaves.add(change.property);
          styleOverlay.set(address, leaves);
        }
    const styleTouched = new Map<string, Set<keyof CellStyle>>(
      [...this.previewStyleOverlay].map(([address, leaves]) => [address, new Set(leaves)]),
    );
    for (const [address, leaves] of styleOverlay) {
      const union = styleTouched.get(address) ?? new Set<keyof CellStyle>();
      for (const leaf of leaves) union.add(leaf);
      styleTouched.set(address, union);
    }
    this.previewStyleOverlay = styleOverlay;
    this.previewOverlay = overlay;
    this.previewDirty.clear();
    if (reset) this.previewRevision = -1n;
    const stamp = {
      document_id: projection.document_id,
      input_version: projection.version,
      pending_revision: String(++this.previewRevision),
    };
    const styleMutations = (targets: Map<string, Set<keyof CellStyle>>): PreviewStyleMutation[] =>
      (projection.authored_projection_schema_version ?? 1) >= 4
        ? [...targets].flatMap(([address, leaves]) => {
            const [sheet_id = '', row_id = '', column_id = ''] = address.split('/');
            const owner = projection.sheets.find((s) => s.id === sheet_id);
            if (!owner?.rows.includes(row_id) || !owner.columns.includes(column_id)) return [];
            const style = this.styleAt(sheet_id, row_id, column_id);
            return [
              {
                sheet_id,
                row_id,
                column_id,
                changes: [...leaves].map((property) => ({
                  property,
                  value:
                    JSON.stringify(style[property]) === JSON.stringify(DEFAULT_STYLE[property])
                      ? null
                      : style[property],
                })),
              },
            ];
          })
        : [];
    if (reset) {
      const keys = new Set([
        ...this.confirmed.keys(),
        ...((projection.authored_projection_schema_version ?? 1) >= 4 ? [] : overlay),
      ]);
      const cellsBySheet = new Map(
        projection.sheets.map((item) => [item.id, [] as Sheet['cells']]),
      );
      for (const cellKey of keys) {
        const [row_id = '', column_id = ''] = cellKey.split('/');
        const owner = this.previewSheetByRow.get(row_id);
        if (!owner) continue;
        cellsBySheet.get(owner)?.push({
          row_id,
          column_id,
          value:
            (projection.authored_projection_schema_version ?? 1) >= 4
              ? this.confirmed.get(cellKey)!
              : this.previewValue(row_id, column_id),
        });
      }
      const sheets = projection.sheets.map((item) => {
        const styles = new Map(
          (item.styles ?? []).map((cell) => [key(cell.row_id, cell.column_id), cell]),
        );
        const rows = new Set(item.rows),
          columns = new Set(item.columns);
        for (const address of (projection.authored_projection_schema_version ?? 1) >= 4
          ? []
          : styleOverlay.keys()) {
          const [owner, row = '', column = ''] = address.split('/');
          if (owner !== item.id || !rows.has(row) || !columns.has(column)) continue;
          const style = this.styleAt(item.id, row, column);
          if (isDefaultStyle(style)) styles.delete(key(row, column));
          else styles.set(key(row, column), { row_id: row, column_id: column, style });
        }
        return {
          ...item,
          cells: cellsBySheet.get(item.id) ?? [],
          ...((projection.authored_projection_schema_version ?? 1) >= 4
            ? { styles: [...styles.values()] }
            : {}),
        };
      });
      if ((projection.authored_projection_schema_version ?? 1) >= 4) {
        if (
          projection.last_axis_change_version === undefined ||
          !projection.retired_row_ids ||
          !projection.retired_column_ids ||
          !projection.row_activation_floors ||
          !projection.column_activation_floors
        ) {
          this.preview.dispose();
          this.previewStatus = 'failed';
          return;
        }
        this.preview.reset({
          ...stamp,
          schema_version: (projection.authored_projection_schema_version ?? 0) >= 7 ? 5 : 4,
          sheets,
          last_axis_change_version: projection.last_axis_change_version,
          retired_row_ids: projection.retired_row_ids,
          retired_column_ids: projection.retired_column_ids,
          row_activation_floors: projection.row_activation_floors,
          column_activation_floors: projection.column_activation_floors,
          ...(projection.calculation_context
            ? { calculation_context: projection.calculation_context }
            : {}),
        });
        // Exact hydration contains only authored values with server-bound ASTs.
        // Draft source and pending leaves are separate commands, even after reset.
        if (overlay.size || styleOverlay.size)
          this.preview.patch(
            { ...stamp, pending_revision: String(++this.previewRevision) },
            [...overlay].flatMap((cellKey) => {
              const [row_id = '', column_id = ''] = cellKey.split('/'),
                sheet_id = this.previewSheetByRow.get(row_id);
              return sheet_id
                ? [{ sheet_id, row_id, column_id, value: this.previewValue(row_id, column_id) }]
                : [];
            }),
            styleMutations(styleOverlay),
          );
      } else
        this.preview.reset({
          ...stamp,
          ...(projection.calculation_context
            ? { schema_version: 3 as const, calculation_context: projection.calculation_context }
            : { schema_version: projection.sheets.length === 1 ? (1 as const) : (2 as const) }),
          sheets,
        });
    } else {
      const cells: PreviewCell[] = [...touched].flatMap((cellKey) => {
        const [row_id = '', column_id = ''] = cellKey.split('/');
        const sheet_id = this.previewSheetByRow.get(row_id);
        if (!sheet_id) return [];
        return [
          {
            sheet_id,
            row_id,
            column_id,
            value: this.previewValue(row_id, column_id),
          },
        ];
      });
      this.preview.patch(stamp, cells, styleMutations(styleTouched));
    }
  }
  private restartCalculation() {
    clearTimeout(this.calculationTimer);
    this.calculationAssembly = undefined;
    this.calculationRequestedVersion = undefined;
    this.calculationError = '';
    this.calculationStatus = [...this.confirmed.values()].some((value) => value.type === 'formula')
      ? 'pending'
      : 'idle';
    this.requestCalculation();
  }
  private requestCalculation() {
    if (
      !this.sender ||
      !this.projection ||
      this.calculationStatus !== 'pending' ||
      this.calculationRequestedVersion === this.projection.version
    )
      return;
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'requestCalculation',
          value: {
            documentId: uuidBytes(this.projection.document_id),
            inputVersion: BigInt(this.projection.version),
            baseInputVersion:
              this.calculatedVersion === undefined ? undefined : BigInt(this.calculatedVersion),
          },
        },
      }),
    );
    if (!this.sender(bytes)) {
      this.failCalculation();
      return;
    }
    this.calculationRequestedVersion = this.projection.version;
    this.calculationStartedAt = performance.now();
    this.performanceMetrics.recalculatedCells = 0;
    this.performanceMetrics.dirtyCells = 0;
    this.calculationTimer = setTimeout(() => {
      this.failCalculation();
      this.notify();
    }, 35000);
  }
  private failCalculation() {
    clearTimeout(this.calculationTimer);
    this.calculationAssembly = undefined;
    this.calculationStatus = 'failed';
    this.calculationError = 'Расчет недоступен. Формула сохранена; результат пока не подтвержден.';
  }
  private receiveCalculation(message: CalculationCommitted) {
    if (
      !this.projection ||
      bytesUuid(message.documentId) !== this.projection.document_id ||
      message.inputVersion !== BigInt(this.projection.version)
    )
      return;
    if (
      this.calculationStatus !== 'pending' ||
      this.calculationRequestedVersion !== this.projection.version
    )
      return;
    let shouldNotify = false;
    try {
      const bytes = toBinary(CalculationCommittedSchema, message).byteLength;
      if (
        message.documentId.length !== 16 ||
        message.contextId.length !== 16 ||
        bytesUuid(message.contextId) !==
          (this.projection.calculation_context?.context_id ?? this.projection.document_id) ||
        message.calculationEpoch !== BigInt(this.projection.calculation_context?.epoch ?? '0') ||
        message.formulaEngineVersion !== FORMULA_ENGINE_VERSION ||
        message.compatibilityProfile !== formulaProfile ||
        bytes > 256 * 1024 ||
        message.patchSequence > 4096 ||
        message.evaluatedCells > 100_000 ||
        message.dirtyCells > 200_000
      )
        throw new Error('Invalid calculation metadata');
      if (message.errorCode !== 0) {
        if (
          message.errorCode < 1 ||
          message.errorCode > 5 ||
          !message.isFinal ||
          message.patchSequence !== 0 ||
          message.cells.length !== 0 ||
          message.calculatedStateHash.length !== 0 ||
          message.baseInputVersion !== undefined ||
          message.evaluatedCells !== 0 ||
          message.dirtyCells !== 0
        )
          throw new Error('Invalid calculation failure');
        this.failCalculation();
        shouldNotify = true;
        return;
      }
      if (message.calculatedStateHash.length !== (message.isFinal ? 32 : 0))
        throw new Error('Invalid calculation hash');
      const formulaCount =
        this.calculationAssembly?.expectedFormulaCount ??
        [...this.confirmed.values()].filter((value) => value.type === 'formula').length;
      if (formulaCount > 100_000) throw new Error('Calculation exceeds limit');
      const baseInputVersion = message.baseInputVersion;
      if (
        (message.fullSnapshot && baseInputVersion !== undefined) ||
        (!message.fullSnapshot &&
          (baseInputVersion === undefined ||
            this.calculatedVersion === undefined ||
            baseInputVersion !== BigInt(this.calculatedVersion) ||
            baseInputVersion >= message.inputVersion))
      )
        throw new Error('Invalid calculation base');
      const assembly = this.calculationAssembly ?? {
        nextSequence: 0,
        bytes: 0,
        values: message.fullSnapshot
          ? new Map<string, CalculatedValue>()
          : new Map(this.calculated),
        seen: new Set<string>(),
        patchCells: 0,
        fullSnapshot: message.fullSnapshot,
        baseInputVersion,
        expectedFormulaCount: formulaCount,
        evaluatedCells: message.evaluatedCells,
        dirtyCells: message.dirtyCells,
      };
      if (
        assembly.fullSnapshot !== message.fullSnapshot ||
        assembly.baseInputVersion !== baseInputVersion ||
        assembly.expectedFormulaCount !== formulaCount ||
        assembly.evaluatedCells !== message.evaluatedCells ||
        assembly.dirtyCells !== message.dirtyCells
      )
        throw new Error('Inconsistent calculation chunks');
      if (message.patchSequence !== assembly.nextSequence)
        throw new Error('Invalid calculation sequence');
      assembly.bytes += bytes;
      assembly.patchCells += message.cells.length;
      if (
        assembly.bytes > 32 * 1024 * 1024 ||
        assembly.patchCells > (message.fullSnapshot ? 100_000 : 200_000) ||
        (message.fullSnapshot && assembly.values.size + message.cells.length > formulaCount)
      )
        throw new Error('Calculation exceeds limit');
      const axesBySheet = new Map(
        this.projection.sheets.map(
          (sheet) =>
            [sheet.id, { rows: new Set(sheet.rows), columns: new Set(sheet.columns) }] as const,
        ),
      );
      for (const cell of message.cells) {
        const address = cell.address,
          value = cell.value?.kind;
        const targetAxes =
          address?.sheetId.length === 16 ? axesBySheet.get(bytesUuid(address.sheetId)) : undefined;
        if (
          !address ||
          address.sheetId.length !== 16 ||
          address.rowId.length !== 16 ||
          address.columnId.length !== 16 ||
          !targetAxes ||
          !targetAxes.rows.has(bytesUuid(address.rowId)) ||
          !targetAxes.columns.has(bytesUuid(address.columnId))
        )
          throw new Error('Invalid calculation cell');
        const cellKey = key(bytesUuid(address.rowId), bytesUuid(address.columnId));
        if (assembly.seen.has(cellKey)) throw new Error('Duplicate calculation cell');
        assembly.seen.add(cellKey);
        const isFormula = this.confirmed.get(cellKey)?.type === 'formula';
        if (!cell.value) {
          if (message.fullSnapshot || isFormula || !assembly.values.has(cellKey))
            throw new Error('Invalid calculation clear');
          assembly.values.delete(cellKey);
          continue;
        }
        if (!value?.case) throw new Error('Invalid calculated value');
        if (!isFormula) throw new Error('Invalid calculation result');
        const errors = [
          '#NULL!',
          '#DIV/0!',
          '#VALUE!',
          '#REF!',
          '#NAME?',
          '#NUM!',
          '#N/A',
          '#SPILL!',
          '#CYCLE!',
        ];
        const decoded =
          value.case === 'error'
            ? decodeCalculatedValue({ type: 'error', value: errors[value.value - 1] })
            : decodeCalculatedValue({ type: value.case, value: value.value });
        assembly.values.set(cellKey, decoded);
      }
      assembly.nextSequence++;
      this.calculationAssembly = assembly;
      if (message.isFinal) {
        if (
          assembly.values.size !== formulaCount ||
          [...assembly.values.keys()].some(
            (cellKey) => this.confirmed.get(cellKey)?.type !== 'formula',
          )
        )
          throw new Error('Incomplete calculation');
        clearTimeout(this.calculationTimer);
        this.calculated = assembly.values;
        this.performanceMetrics.calculationPatchMs = performance.now() - this.calculationStartedAt;
        this.performanceMetrics.recalculatedCells = assembly.evaluatedCells;
        this.performanceMetrics.dirtyCells = assembly.dirtyCells;
        this.lastDisplayedResults = new Map(assembly.values);
        this.calculatedVersion = this.projection.version;
        this.calculationStatus = 'complete';
        this.calculationError = '';
        this.calculationAssembly = undefined;
        shouldNotify = true;
      }
    } catch {
      this.failCalculation();
      shouldNotify = true;
    } finally {
      if (shouldNotify) this.notify();
    }
  }
  load(projection: Projection) {
    if (this.guestAccessClosed) return;
    if (this.transport.guest && projection.document_id !== this.transport.guest.documentId)
      throw new Error('Ответ относится к другой таблице');
    const previousSheet = this.sheet;
    const documentChanged = this.projection?.document_id !== projection.document_id;
    const nextSheet =
      projection.sheets.find((sheet) => sheet.id === this.activeSheetId && !documentChanged) ??
      projection.sheets[0];
    const axesChanged = Boolean(
      previousSheet &&
      nextSheet &&
      (previousSheet.rows.length !== nextSheet.rows.length ||
        previousSheet.columns.length !== nextSheet.columns.length ||
        previousSheet.rows.some((row, index) => row !== nextSheet.rows[index]) ||
        previousSheet.columns.some((column, index) => column !== nextSheet.columns[index])),
    );
    if (axesChanged) {
      this.assistant.reset();
      if (this.editing) {
        this.editing = undefined;
        this.error = 'Структура таблицы изменилась. Повторите ввод в нужной ячейке.';
      }
      this.remotePresence = [];
    }
    if (documentChanged) {
      this.assistant.reset();
      this.privacyMask = undefined;
      this.privacyMaskEnabled = true;
      this.privacyFindings = [];
      this.privacyReport = undefined;
      this.privacyPolicy = undefined;
      this.privacyAnalysisPending = false;
      this.privacyAnalysisError = '';
      this.personalView = { schema_version: 1, revision: 0, settings: { filters: [] } };
      this.viewDocument = undefined;
      this.viewGeneration++;
      this.viewLoading = false;
      this.viewError = '';
      this.lastDisplayedResults.clear();
      this.calculated.clear();
      this.calculatedVersion = undefined;
      clearTimeout(this.headRefreshTimer);
      this.headRefreshVersion = 0n;
      this.headRefreshAttempts = 0;
      this.remotePresence = [];
      this.undoHistory = [];
      this.redoHistory = [];
      this.historyVersions.clear();
      this.historyThrough = 0n;
      this.historyObservedThrough = 0n;
      this.historyRequiredThrough = 0n;
      this.dirtyHistory(BigInt(projection.version));
      this.cutIntent = undefined;
    }
    const remap = (position: Position): Position => {
      if (documentChanged || !previousSheet || !nextSheet) return { row: 0, column: 0 };
      if (!axesChanged) return position;
      const priorRow = previousSheet.rows[position.row];
      const priorColumn = previousSheet.columns[position.column];
      const row = priorRow ? nextSheet.rows.indexOf(priorRow) : -1;
      const column = priorColumn ? nextSheet.columns.indexOf(priorColumn) : -1;
      return {
        row: row >= 0 ? row : Math.min(position.row, Math.max(0, nextSheet.rows.length - 1)),
        column:
          column >= 0
            ? column
            : Math.min(position.column, Math.max(0, nextSheet.columns.length - 1)),
      };
    };
    const nextSelected = remap(this.selected);
    const nextAnchor = remap(this.selectionAnchor);
    this.projection = projection;
    if (documentChanged && this.actor && projection.capabilities) void this.reloadPersonalView();
    this.pruneHistory();
    this.activeSheetId = nextSheet?.id;
    this.selected = nextSelected;
    this.selectionAnchor = nextAnchor;
    this.confirmed = new Map(
      projection.sheets.flatMap((sheet) =>
        sheet.cells.map((c) => [key(c.row_id, c.column_id), c.value] as const),
      ),
    );
    this.confirmedStyles = new Map(
      projection.sheets.flatMap((sheet) =>
        (sheet.styles ?? []).map(
          (cell) => [`${sheet.id}/${key(cell.row_id, cell.column_id)}`, cell.style] as const,
        ),
      ),
    );
    this.restartCalculation();
    this.syncPreview(true);
    this.subscribeToHead();
    this.scheduleHistory();
    this.notify();
  }
  private subscribeToHead() {
    const projection = this.projection;
    if (!projection || !this.sender || this.subscribedDocument === projection.document_id) return;
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'subscribeDocument',
          value: {
            documentId: uuidBytes(projection.document_id),
            lastSeenVersion: BigInt(projection.version),
          },
        },
      }),
    );
    if (this.sender(bytes)) this.subscribedDocument = projection.document_id;
    this.schedulePresence();
  }
  private schedulePresence() {
    clearTimeout(this.presenceTimer);
    if (!this.sender || !this.subscribedDocument || !this.sheet) return;
    this.presenceTimer = setTimeout(() => {
      const sheet = this.sheet;
      const documentId = this.projection?.document_id;
      const row = sheet?.rows[this.selected.row];
      const column = sheet?.columns[this.selected.column];
      const anchorRow = sheet?.rows[this.selectionAnchor.row];
      const anchorColumn = sheet?.columns[this.selectionAnchor.column];
      if (!sheet || !documentId || !row || !column || !anchorRow || !anchorColumn || !this.sender)
        return;
      const focus = {
        sheetId: uuidBytes(sheet.id),
        rowId: uuidBytes(row),
        columnId: uuidBytes(column),
      };
      const anchor = {
        sheetId: uuidBytes(sheet.id),
        rowId: uuidBytes(anchorRow),
        columnId: uuidBytes(anchorColumn),
      };
      const bytes = toBinary(
        ClientEnvelopeSchema,
        create(ClientEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'setPresence',
            value: { documentId: uuidBytes(documentId), anchor, focus },
          },
        }),
      );
      this.sender(bytes);
    }, 60);
  }
  private async refreshHead() {
    clearTimeout(this.headRefreshTimer);
    const target = this.headRefreshVersion;
    if (!this.sender || !this.projection || BigInt(this.projection.version) >= target) {
      this.headRefreshAttempts = 0;
      return;
    }
    if (await this.refresh(target)) {
      this.headRefreshAttempts = 0;
      if (BigInt(this.projection.version) < this.headRefreshVersion) await this.refreshHead();
      return;
    }
    if (!this.sender) return;
    const delay = Math.min(30_000, 1000 * 2 ** Math.min(this.headRefreshAttempts++, 5));
    this.headRefreshTimer = setTimeout(() => void this.refreshHead(), delay);
  }
  async refresh(minimumVersion = 0n): Promise<boolean> {
    if (this.guestAccessClosed) return false;
    const id =
      this.transport.guest?.documentId ??
      this.projection?.document_id ??
      new URL(location.href).searchParams.get('document');
    if (!id) return true;
    if (this.movePreview) this.syncPreview(true);
    const generation = ++this.syncGeneration;
    this.syncing = true;
    this.loading = true;
    this.notify();
    try {
      let response: Response | undefined;
      for (const delay of [200, 400, 800, 1600, 3200, 0]) {
        if (generation !== this.syncGeneration) return false;
        response = await this.fetchDocument(this.documentUrl(id), {
          signal: AbortSignal.timeout(10000),
        });
        if (response.status !== 429 || delay === 0) break;
        await new Promise((resolve) => setTimeout(resolve, delay));
      }
      if (!response) throw new Error('Не удалось открыть таблицу');
      if (!response.ok)
        throw new Error(
          response.status === 403 ? 'Нет доступа к таблице' : 'Не удалось открыть таблицу',
        );
      const projection = decodeProjection(await response.json());
      if (generation !== this.syncGeneration) return false;
      if (projection.document_id !== id) throw new Error('Ответ относится к другой таблице');
      const currentVersion =
        this.projection?.document_id === id ? BigInt(this.projection.version) : -1n;
      if (BigInt(projection.version) > currentVersion) this.load(projection);
      else if (BigInt(projection.version) === currentVersion && this.projection) {
        if (projection.capabilities) this.projection.capabilities = projection.capabilities;
        else delete this.projection.capabilities;
        if (projection.encryption) this.projection.encryption = projection.encryption;
        else delete this.projection.encryption;
      }
      if (!this.projection || BigInt(this.projection.version) < minimumVersion)
        throw new Error('Сервер еще не подтвердил актуальное состояние таблицы');
      this.error = '';
      return true;
    } catch (error) {
      if (generation === this.syncGeneration)
        this.error = error instanceof Error ? error.message : 'Не удалось открыть таблицу';
      return false;
    } finally {
      if (generation === this.syncGeneration) {
        this.loading = false;
        this.syncing = false;
        this.scheduleHistory();
        this.notify();
      }
    }
  }
  async createDocument() {
    if (this.isGuest) return;
    if (this.loading) return;
    this.loading = true;
    this.creating = true;
    this.notify();
    try {
      this.creation ??= { document_id: uuidV7(), operation_id: uuidV7() };
      const id = this.creation.document_id;
      const response = await fetch('/api/documents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(this.creation),
        signal: AbortSignal.timeout(10000),
      });
      if (!response.ok) throw new Error('Не удалось создать таблицу');
      const url = new URL(location.href);
      url.searchParams.set('document', id);
      this.creation = undefined;
      this.projection = undefined;
      location.assign(url);
    } catch (error) {
      this.error = error instanceof Error ? error.message : 'Не удалось создать таблицу';
    } finally {
      this.creating = false;
      this.loading = false;
      this.notify();
    }
  }
  async recalculate() {
    if (!this.canEdit) return;
    const id = this.projection?.document_id;
    if (!id || !this.sender || this.recalculating || this.pending.length) return;
    if (this.recalculationRequest?.document_id !== id)
      this.recalculationRequest = { document_id: id, operation_id: uuidV7() };
    const request = this.recalculationRequest;
    this.recalculating = true;
    this.error = '';
    this.notify();
    try {
      const response = await this.fetchDocument(this.documentUrl(id, '/recalculate'), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ operation_id: request.operation_id }),
        signal: AbortSignal.timeout(10_000),
      });
      if (!response.ok) {
        this.error =
          response.status === 403
            ? 'Нет права пересчитывать эту таблицу.'
            : 'Не удалось пересчитать таблицу. Повторите запрос.';
        return;
      }
      const receipt = (await response.json()) as {
        operation_id?: unknown;
        committed_version?: unknown;
      };
      if (
        !receipt ||
        typeof receipt !== 'object' ||
        receipt.operation_id !== request.operation_id ||
        typeof receipt.committed_version !== 'string' ||
        !/^[1-9]\d*$/.test(receipt.committed_version)
      )
        throw new Error('Invalid calculation receipt');
      // The validated receipt proves commit. A concurrent WebSocket resync can
      // supersede this HTTP refresh, but must not reuse this ID for a new epoch.
      this.recalculationRequest = undefined;
      if (!(await this.refresh(BigInt(receipt.committed_version)))) return;
    } catch {
      // Keep the request ID: a lost response may follow a durable system commit.
      this.error = 'Не удалось пересчитать таблицу. Повторите запрос.';
    } finally {
      this.recalculating = false;
      this.notify();
    }
  }
  async exportXlsx(anonymize = false) {
    if (!this.canExport) return;
    const id = this.projection?.document_id;
    if (!id || this.exporting || this.pending.length) return;
    this.exporting = true;
    this.error = '';
    this.notify();
    try {
      const response = await fetch(
        this.documentUrl(
          id,
          `/export.xlsx${anonymize || this.privacyMask ? '?anonymized=true' : ''}`,
        ),
        {
          signal: AbortSignal.timeout(30_000),
        },
      );
      if (!response.ok) {
        throw new Error(
          response.status === 403
            ? 'Экспорт запрещён политикой выдачи или права изменились.'
            : response.status === 422
              ? 'Эта таблица содержит данные вне поддерживаемого XLSX-профиля.'
              : 'Не удалось скачать XLSX.',
        );
      }
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement('a');
      link.href = url;
      link.download = 'PXEL.xlsx';
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    } catch (error) {
      this.error = error instanceof Error ? error.message : 'Не удалось скачать XLSX.';
    } finally {
      this.exporting = false;
      this.notify();
    }
  }
  async connected(actor: Uint8Array, sender: (bytes: Uint8Array) => boolean) {
    if (this.guestAccessClosed) return false;
    if (this.transport.guest && bytesUuid(actor) !== this.transport.guest.actorId) {
      this.closeGuestAccess();
      return false;
    }
    const generation = ++this.connectionGeneration;
    const previousActor = this.actor && bytesUuid(this.actor),
      nextActor = bytesUuid(actor);
    const document = this.projection?.document_id;
    if (previousActor && previousActor !== nextActor) {
      const restore =
        this.quarantinedPending?.actor === nextActor &&
        this.quarantinedPending.document === document
          ? this.quarantinedPending
          : undefined;
      if (this.pending.length && this.quarantinedPending && !restore) {
        this.disconnected();
        this.error =
          'Сначала подтвердите правки предыдущих аккаунтов. Смена аккаунта приостановлена.';
        this.notify();
        return false;
      }
      const quarantine =
        this.pending.length && document
          ? { actor: previousActor, document, intents: this.pending }
          : undefined;
      this.pending = restore?.intents ?? [];
      this.quarantinedPending = quarantine ?? (restore ? undefined : this.quarantinedPending);
      this.undoHistory = [];
      this.redoHistory = [];
      this.historyVersions.clear();
      this.historyThrough = 0n;
      this.historyObservedThrough = 0n;
      this.historyRequiredThrough = 0n;
      this.editing = undefined;
      this.cutIntent = undefined;
      this.syncPreview(true);
    }
    this.pending.forEach((op) => {
      op.attempts = 0;
    });
    this.actor = actor;
    if (previousActor !== nextActor)
      this.personalView = { schema_version: 1, revision: 0, settings: { filters: [] } };
    if (previousActor !== nextActor || this.viewDocument !== document)
      void this.reloadPersonalView();
    this.sender = undefined;
    this.inflight = undefined;
    const synced = await this.refresh();
    if (generation !== this.connectionGeneration) return;
    if (!synced) return false;
    this.sender = sender;
    this.dirtyHistory();
    this.subscribeToHead();
    if (this.previewStatus === 'idle' && this.projection) this.syncPreview(true);
    this.restartCalculation();
    this.flush();
    await this.discoverHistory();
    return true;
  }
  disconnected() {
    const hadPresence = this.remotePresence.length > 0;
    this.calculationRequestedVersion = undefined;
    this.calculationAssembly = undefined;
    clearTimeout(this.calculationTimer);
    this.connectionGeneration++;
    this.historyGeneration++;
    this.historyDirty = true;
    this.historyAbort?.abort();
    clearTimeout(this.historyTimer);
    this.syncGeneration++;
    this.syncing = false;
    this.sender = undefined;
    this.subscribedDocument = undefined;
    clearTimeout(this.presenceTimer);
    this.remotePresence = [];
    clearTimeout(this.headRefreshTimer);
    this.headRefreshAttempts = 0;
    this.headRefreshVersion = 0n;
    this.inflight = undefined;
    clearTimeout(this.timer);
    if (hadPresence) this.notify();
  }
  dispose() {
    this.assistant.reset();
    this.autofillPreview = undefined;
    this.disconnected();
    this.preview.dispose();
    this.previewValues = new Map();
    this.previewStamp = undefined;
    this.previewStatus = 'idle';
    this.movePreview = undefined;
  }
  select(position: Position, extend = false) {
    if (this.allRowsHidden) return;
    this.selectionAxis = undefined;
    const merged = this.mergeAt(position);
    if (merged) position = { row: merged.startRow, column: merged.startColumn };
    if (!extend) this.selectionAnchor = { ...position };
    this.selected = position;
    this.schedulePresence();
    this.notify();
  }
  mergeAt(position: Position) {
    const sheet = this.sheet;
    if (!sheet) return undefined;
    const mergeVersion = this.projection?.version ?? '';
    if (
      sheet.id !== this.mergeCacheSheetId ||
      mergeVersion !== this.mergeCacheVersion ||
      sheet.merges !== this.mergeCacheMerges
    ) {
      this.mergeCacheSheetId = sheet.id;
      this.mergeCacheVersion = mergeVersion;
      this.mergeCacheMerges = sheet.merges;
      this.mergeCache = new Map();
      for (const merge of sheet.merges ?? []) {
        const startRow = sheet.rows.indexOf(merge.first_row_id),
          endRow = sheet.rows.indexOf(merge.last_row_id),
          startColumn = sheet.columns.indexOf(merge.first_column_id),
          endColumn = sheet.columns.indexOf(merge.last_column_id),
          bounds = { startRow, endRow, startColumn, endColumn };
        for (let row = startRow; row <= endRow; row++)
          for (let column = startColumn; column <= endColumn; column++)
            this.mergeCache.set(key(sheet.rows[row]!, sheet.columns[column]!), bounds);
      }
    }
    const rowId = sheet.rows[position.row],
      columnId = sheet.columns[position.column];
    if (!rowId || !columnId) return undefined;
    return this.mergeCache.get(key(rowId, columnId));
  }
  selectColumn(column: number) {
    const sheet = this.sheet;
    if (!sheet || !Number.isInteger(column) || column < 0 || column >= sheet.columns.length) return;
    this.selected = { row: 0, column };
    this.selectionAnchor = { row: 0, column };
    this.selectionAxis = { kind: 'column', index: column };
    this.schedulePresence();
    this.notify();
  }
  get selectionBounds() {
    if (this.selectionAxis?.kind === 'column') {
      return {
        startRow: 0,
        endRow: Math.max(0, (this.sheet?.rows.length ?? 1) - 1),
        startColumn: this.selectionAxis.index,
        endColumn: this.selectionAxis.index,
      };
    }
    return {
      startRow: Math.min(this.selectionAnchor.row, this.selected.row),
      endRow: Math.max(this.selectionAnchor.row, this.selected.row),
      startColumn: Math.min(this.selectionAnchor.column, this.selected.column),
      endColumn: Math.max(this.selectionAnchor.column, this.selected.column),
    };
  }
  get canMergeSelection() {
    if (!this.canEdit) return false;
    const sheet = this.sheet;
    if (!sheet || this.editing || this.syncing || this.pendingCount >= 128) return false;
    const bounds = this.selectionBounds;
    return bounds.endRow > bounds.startRow || bounds.endColumn > bounds.startColumn;
  }
  get canUnmergeSelection() {
    if (!this.canEdit) return false;
    const sheet = this.sheet;
    if (!sheet || this.editing || this.syncing || this.pendingCount >= 128) return false;
    const bounds = this.selectionBounds;
    return (sheet.merges ?? []).some((merge) => {
      const firstRow = sheet.rows.indexOf(merge.first_row_id),
        lastRow = sheet.rows.indexOf(merge.last_row_id),
        firstColumn = sheet.columns.indexOf(merge.first_column_id),
        lastColumn = sheet.columns.indexOf(merge.last_column_id);
      const exact =
        firstRow === bounds.startRow &&
        lastRow === bounds.endRow &&
        firstColumn === bounds.startColumn &&
        lastColumn === bounds.endColumn;
      const anchor =
        bounds.startRow === bounds.endRow &&
        bounds.startColumn === bounds.endColumn &&
        firstRow === this.selected.row &&
        firstColumn === this.selected.column;
      return exact || anchor;
    });
  }
  setMergeSelected(merged: boolean) {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (
      !sheet ||
      !projection ||
      !actor ||
      (merged ? !this.canMergeSelection : !this.canUnmergeSelection)
    )
      return false;
    const selectedBounds = this.selectionBounds;
    const bounds = merged
      ? selectedBounds
      : (() => {
          const range = (sheet.merges ?? []).find((item) => {
            const firstRow = sheet.rows.indexOf(item.first_row_id),
              lastRow = sheet.rows.indexOf(item.last_row_id),
              firstColumn = sheet.columns.indexOf(item.first_column_id),
              lastColumn = sheet.columns.indexOf(item.last_column_id);
            return (
              (firstRow === selectedBounds.startRow &&
                lastRow === selectedBounds.endRow &&
                firstColumn === selectedBounds.startColumn &&
                lastColumn === selectedBounds.endColumn) ||
              (selectedBounds.startRow === selectedBounds.endRow &&
                selectedBounds.startColumn === selectedBounds.endColumn &&
                firstRow === this.selected.row &&
                firstColumn === this.selected.column)
            );
          });
          if (!range) return selectedBounds;
          return {
            startRow: sheet.rows.indexOf(range.first_row_id),
            endRow: sheet.rows.indexOf(range.last_row_id),
            startColumn: sheet.columns.indexOf(range.first_column_id),
            endColumn: sheet.columns.indexOf(range.last_column_id),
          };
        })();
    const range = {
      firstRowId: uuidBytes(sheet.rows[bounds.startRow]!),
      lastRowId: uuidBytes(sheet.rows[bounds.endRow]!),
      firstColumnId: uuidBytes(sheet.columns[bounds.startColumn]!),
      lastColumnId: uuidBytes(sheet.columns[bounds.endColumn]!),
    };
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            operation: {
              kind: {
                case: 'mergeWrite',
                value: {
                  sheetId: uuidBytes(sheet.id),
                  range,
                  merged,
                },
              },
            },
          },
        },
      }),
    );
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'merge', id, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  get pasteOrigin(): Position {
    const { startRow, startColumn } = this.selectionBounds;
    return { row: startRow, column: startColumn };
  }
  copySelectionTsv() {
    if (!this.canCopy) return undefined;
    const sheet = this.sheet;
    if (!sheet) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    const count = (endRow - startRow + 1) * (endColumn - startColumn + 1);
    if (count > 1_000) {
      this.error = 'Можно скопировать не более 1000 ячеек за один раз.';
      this.notify();
      return undefined;
    }
    const lines: CellMatrix = [];
    let bytes = 0;
    for (let row = startRow; row <= endRow; row++) {
      if (this.filterState.hidden.has(row)) continue;
      const fields: string[] = [];
      for (let column = startColumn; column <= endColumn; column++) {
        const value = this.value({ row, column });
        const text = value?.type === 'formula' ? value.value.current_text : displayValue(value);
        if (text === undefined) {
          this.error = 'Формула ещё не готова для копирования.';
          this.notify();
          return undefined;
        }
        bytes += byteLength(text);
        if (bytes > 512 * 1024) {
          this.error = 'Текст выделения превышает 512 КБ.';
          this.notify();
          return undefined;
        }
        fields.push(text);
      }
      lines.push(fields);
    }
    this.error = '';
    this.notify();
    return serializeDelimited(lines);
  }
  copyRedactedSelectionTsv(projection: Projection) {
    if (
      projection.representation !== 'anonymized' ||
      projection.document_id !== this.projection?.document_id ||
      projection.version !== this.projection.version
    )
      throw new Error('Безопасная копия устарела.');
    const sheet = projection.sheets.find((s) => s.id === this.activeSheetId);
    if (!sheet) throw new Error('Лист копии недоступен.');
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    if ((endRow - startRow + 1) * (endColumn - startColumn + 1) > 1000)
      throw new Error('Можно скопировать не более 1000 ячеек.');
    const cells = new Map(sheet.cells.map((c) => [key(c.row_id, c.column_id), c.value]));
    const lines: CellMatrix = [];
    for (let row = startRow; row <= endRow; row++) {
      if (this.filterState.hidden.has(row)) continue;
      const fields: string[] = [];
      for (let column = startColumn; column <= endColumn; column++)
        fields.push(
          displayValue(cells.get(key(sheet.rows[row] ?? '', sheet.columns[column] ?? ''))),
        );
      lines.push(fields);
    }
    const text = serializeDelimited(lines);
    if (byteLength(text) > 512 * 1024) throw new Error('Текст выделения превышает 512 КБ.');
    return text;
  }
  begin(text = displayValue(this.authoredValue(this.selected)), source: 'cell' | 'bar' = 'cell') {
    if (!this.canEdit) return;
    if (!this.sheet) return;
    this.editing = { position: { ...this.selected }, text, source };
    this.syncPreview();
    this.notify();
  }
  draft(text: string) {
    if (!this.canEdit) return;
    if (this.editing) {
      this.editing = { ...this.editing, text };
      this.syncPreview();
      this.notify();
    }
  }
  cancel() {
    this.editing = undefined;
    this.syncPreview();
    this.notify();
  }
  commit() {
    if (!this.canEdit) return;
    const edit = this.editing;
    if (!edit) return;
    this.editing = undefined;
    if (!this.set(edit.position, parseInput(edit.text))) this.editing = edit;
    this.notify();
  }
  set(position: Position, value: Value) {
    if (!this.canEdit) return false;
    const optimisticStartedAt = performance.now();
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const row = sheet?.rows[position.row],
      column = sheet?.columns[position.column];
    if (!sheet || !projection || !actor || !row || !column) return false;
    if (
      (value.type === 'text' && byteLength(value.value) > 32 * 1024) ||
      (value.type === 'formula' &&
        (byteLength(value.value.source) > 32 * 1024 ||
          value.value.source.length < 2 ||
          !value.value.source.startsWith('=')))
    ) {
      this.error = 'Текст ячейки превышает 32 КБ. Сократите его перед сохранением.';
      this.notify();
      return false;
    }
    if (this.pendingCount >= 128) {
      this.error = 'Слишком много неподтвержденных изменений. Дождитесь соединения.';
      this.notify();
      return false;
    }
    const id = uuidV7();
    const address = {
      sheetId: uuidBytes(sheet.id),
      rowId: uuidBytes(row),
      columnId: uuidBytes(column),
    };
    const wireValue =
      value.type === 'formula'
        ? undefined
        : create(CellValueSchema, {
            kind:
              value.type === 'blank'
                ? { case: 'blank', value: true }
                : value.type === 'number'
                  ? { case: 'number', value: value.value }
                  : value.type === 'boolean'
                    ? { case: 'boolean', value: value.value }
                    : { case: 'text', value: value.value },
          });
    const operation =
      value.type === 'formula'
        ? undefined
        : create(CanonicalOperationSchema, {
            kind:
              value.type === 'blank'
                ? { case: 'clearCell' as const, value: { address } }
                : {
                    case: 'setCellValue' as const,
                    value: { address, value: wireValue },
                  },
          });
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation,
          formulaCommand:
            value.type === 'formula' ? { address, source: value.value.source } : undefined,
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) {
      this.error = 'Содержимое ячейки слишком большое';
      this.notify();
      return false;
    }
    this.pending.push({ kind: 'cell', id, row, column, value, bytes, attempts: 0 });
    this.observeAssistant({ kind: 'edit', position });
    this.syncPreview();
    this.performanceMetrics.optimisticUpdateMs = performance.now() - optimisticStartedAt;
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  get canFormat() {
    if (!this.canEdit) return false;
    return Boolean(
      this.sheet &&
      this.actor &&
      this.sender &&
      !this.syncing &&
      !this.editing &&
      this.pendingCount < 128,
    );
  }
  selectionStyle<K extends keyof CellStyle>(property: K): CellStyle[K] | 'mixed' {
    const b = this.selectionBounds;
    if ((b.endRow - b.startRow + 1) * (b.endColumn - b.startColumn + 1) > 1000) return 'mixed';
    const first = this.style({ row: b.startRow, column: b.startColumn })[property];
    for (let row = b.startRow; row <= b.endRow; row++)
      for (let column = b.startColumn; column <= b.endColumn; column++)
        if (JSON.stringify(this.style({ row, column })[property]) !== JSON.stringify(first))
          return 'mixed';
    return first;
  }
  clearSelection() {
    if (!this.canEdit) return false;
    const b = this.selectionBounds;
    if ((b.endRow - b.startRow + 1) * (b.endColumn - b.startColumn + 1) > 1000) {
      this.error = 'Можно очистить не более 1000 ячеек за один раз.';
      this.notify();
      return false;
    }
    return this.pasteMatrix(
      Array.from({ length: b.endRow - b.startRow + 1 }, () =>
        Array<string>(b.endColumn - b.startColumn + 1).fill(''),
      ),
      this.pasteOrigin,
    );
  }
  setStyle(changes: readonly StyleChange[]) {
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (!sheet || !projection || !actor || !this.canFormat) return false;
    const bounds = this.selectionBounds;
    const count =
      (bounds.endRow - bounds.startRow + 1) * (bounds.endColumn - bounds.startColumn + 1);
    if (!Number.isInteger(count) || count < 1 || count > 1000) {
      this.error = 'Можно оформить не более 1000 ячеек за один раз.';
      this.notify();
      return false;
    }
    try {
      const patch = encodeStylePatch(changes);
      const addresses = [],
        targets = new Set<string>();
      for (let r = bounds.startRow; r <= bounds.endRow; r++)
        for (let c = bounds.startColumn; c <= bounds.endColumn; c++) {
          const row = sheet.rows[r],
            column = sheet.columns[c];
          if (!row || !column) throw new Error('Выделение находится вне листа.');
          addresses.push({
            sheetId: uuidBytes(sheet.id),
            rowId: uuidBytes(row),
            columnId: uuidBytes(column),
          });
          targets.add(key(row, column));
        }
      const id = uuidV7();
      const bytes = toBinary(
        ClientEnvelopeSchema,
        create(ClientEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'command',
            value: {
              operationId: uuidBytes(id),
              documentId: uuidBytes(projection.document_id),
              actorId: actor,
              baseVersion: BigInt(projection.version),
              clientSequence: ++this.sequence,
              ...(addresses.length === 1
                ? { setCellStyle: { address: addresses[0], patch } }
                : { setRangeStyle: { addresses, patch } }),
            },
          },
        }),
      );
      if (bytes.length > 1048576) throw new Error('Изменение оформления слишком большое.');
      this.pending.push({
        kind: 'style',
        id,
        sheet: sheet.id,
        targets,
        changes: structuredClone(changes),
        bytes,
        attempts: 0,
      });
      this.observeAssistant({ kind: 'format' });
      this.syncPreview();
      this.error = '';
      this.notify();
      this.flush();
      return true;
    } catch (error) {
      this.error = error instanceof Error ? error.message : 'Не удалось изменить оформление.';
      this.notify();
      return false;
    }
  }
  setAxisSize(axis: 'row' | 'column', index: number, size: number | undefined) {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const axisId = axis === 'row' ? sheet?.rows[index] : sheet?.columns[index];
    const minimum = axis === 'row' ? 12 : 24,
      maximum = axis === 'row' ? 1000 : 2000;
    if (
      !sheet ||
      !projection ||
      !actor ||
      !axisId ||
      !this.sender ||
      this.syncing ||
      this.pendingCount >= 128
    )
      return false;
    if (size !== undefined && (!Number.isInteger(size) || size < minimum || size > maximum)) {
      this.error = `Размер должен быть от ${minimum} до ${maximum} пикселей.`;
      this.notify();
      return false;
    }
    const id = uuidV7();
    const writes = [
      { axisKind: axis === 'row' ? 1 : 2, axisId: uuidBytes(axisId), sizeCssPx: size },
    ];
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            setAxisSizes: { sheetId: uuidBytes(sheet.id), writes },
          },
        },
      }),
    );
    if (bytes.length > 1_048_576) return false;
    this.pending.push({ kind: 'axisSize', id, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  get canUndo() {
    if (!this.canEdit) return false;
    return Boolean(
      this.undoHistory.length &&
      this.pendingCount < 128 &&
      !this.historyDirty &&
      !this.historyLoading &&
      this.sender &&
      this.pending.length === 0 &&
      !this.editing,
    );
  }
  get canRedo() {
    if (!this.canEdit) return false;
    return Boolean(
      this.redoHistory.length &&
      this.pendingCount < 128 &&
      !this.historyDirty &&
      !this.historyLoading &&
      this.sender &&
      this.pending.length === 0 &&
      !this.editing,
    );
  }
  undo() {
    return this.inverse('undo');
  }
  redo() {
    return this.inverse('redo');
  }
  private inverse(direction: 'undo' | 'redo') {
    const projection = this.projection,
      actor = this.actor,
      target = (direction === 'undo' ? this.undoHistory : this.redoHistory).at(-1);
    if (
      !projection ||
      !actor ||
      !target ||
      this.pendingCount >= 128 ||
      !(direction === 'undo' ? this.canUndo : this.canRedo)
    )
      return false;
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            undoCell: { targetOperationId: uuidBytes(target) },
          },
        },
      }),
    );
    this.pending.push({ kind: 'undo', direction, id, target, bytes, attempts: 0 });
    this.observeAssistant({ kind: 'undo' });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  copyFormulaPayload() {
    if (!this.canCopy) return undefined;
    if (this.pending.length || !this.projection) return undefined;
    if (
      this.selectionAnchor.row !== this.selected.row ||
      this.selectionAnchor.column !== this.selected.column
    )
      return undefined;
    const value = this.value(this.selected);
    if (value?.type !== 'formula') return undefined;
    const source = value.value.current_text;
    if (!source) return undefined;
    return JSON.stringify({
      schema_version: 1,
      document_id: this.projection.document_id,
      version: this.projection.version,
      sheet_id: this.sheet?.id,
      source,
      from_row: this.selected.row,
      from_column: this.selected.column,
    });
  }
  get cutStatus(): 'ready' | 'pending' | 'stale' | undefined {
    const cut = this.cutIntent;
    if (!cut) return undefined;
    if (
      this.pending.some((pending) => pending.kind === 'moveRange' && pending.cutPayload === cut.raw)
    )
      return 'pending';
    return cut.stale || cut.version !== this.projection?.version ? 'stale' : 'ready';
  }
  reconcileCutClipboard(raw: string) {
    if (this.cutIntent && this.cutIntent.raw !== raw) {
      this.cutIntent = undefined;
      this.notify();
    }
  }
  private moveError(message: string) {
    this.error = message;
    this.notify();
    return false;
  }
  private canPrepareMove() {
    if (!this.canEdit) return false;
    if (!this.projection || !this.sheet || !this.actor || !this.sender)
      return this.moveError('Для перемещения подключитесь к таблице. Исходные ячейки не изменены.');
    if (
      this.editing ||
      this.pending.length ||
      this.pendingCount >= 128 ||
      this.syncing ||
      this.loading ||
      this.formulaCopyInFlight
    )
      return this.moveError('Завершите ввод и дождитесь сохранения перед перемещением.');
    return true;
  }
  cutRangePayload(activate = true) {
    if (this.privacyMask) return undefined;
    if (!this.canCopy) return undefined;
    if (!this.canPrepareMove()) return undefined;
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    const rows = endRow - startRow + 1,
      columns = endColumn - startColumn + 1;
    if ([...this.filterState.hidden].some((row) => row >= startRow && row <= endRow)) {
      this.moveError('Сначала снимите фильтр перед вырезанием диапазона со скрытыми строками.');
      return undefined;
    }
    if (
      ![startRow, endRow, startColumn, endColumn].every(Number.isSafeInteger) ||
      startRow < 0 ||
      startColumn < 0 ||
      rows * columns > 1_000 ||
      endRow >= sheet.rows.length ||
      endColumn >= sheet.columns.length
    ) {
      this.moveError('Можно переместить от 1 до 1000 ячеек в пределах одного листа.');
      return undefined;
    }
    const raw = JSON.stringify({
      schema_version: 1,
      intent: 'cut',
      cut_id: uuidV7(),
      document_id: projection.document_id,
      version: projection.version,
      sheet_id: sheet.id,
      row_id: sheet.rows[startRow],
      column_id: sheet.columns[startColumn],
      rows,
      columns,
    });
    if (activate) this.cutIntent = { raw, version: projection.version, stale: false };
    this.error = '';
    this.notify();
    return raw;
  }
  activateCutClipboard(raw: string) {
    if (!this.projection || !this.canCopy || !this.canEdit) return;
    this.cutIntent = { raw, version: this.projection.version, stale: false };
    this.notify();
  }
  pasteCutRange(raw: string, position: Position = this.pasteOrigin) {
    if (!this.canPrepareMove()) return false;
    const projection = this.projection,
      sheet = this.sheet,
      actor = this.actor;
    if (!projection || !sheet || !actor) return false;
    const invalid = () => this.moveError('Некорректный вырезанный диапазон. Вырежьте его ещё раз.');
    if (byteLength(raw) > 16 * 1024) return invalid();
    let payload: unknown;
    try {
      payload = JSON.parse(raw);
    } catch {
      return invalid();
    }
    if (!payload || typeof payload !== 'object') return invalid();
    const cut = payload as Record<string, unknown>;
    if (
      cut.schema_version !== 1 ||
      cut.intent !== 'cut' ||
      !isUuid(cut.cut_id) ||
      !isUuid(cut.document_id) ||
      !isUuid(cut.sheet_id) ||
      !isUuid(cut.row_id) ||
      !isUuid(cut.column_id) ||
      typeof cut.version !== 'string' ||
      !/^\d{1,20}$/.test(cut.version) ||
      !Number.isSafeInteger(cut.rows) ||
      !Number.isSafeInteger(cut.columns) ||
      (cut.rows as number) < 1 ||
      (cut.columns as number) < 1 ||
      (cut.rows as number) * (cut.columns as number) > 1_000
    )
      return invalid();
    if (cut.document_id !== projection.document_id || cut.sheet_id !== sheet.id)
      return this.moveError('Перемещение возможно только в пределах исходного листа и таблицы.');
    if (cut.version !== projection.version || (this.cutIntent?.raw === raw && this.cutIntent.stale))
      return this.moveError('Источник диапазона изменился. Вырежьте его ещё раз.');
    const sourceRow = sheet.rows.indexOf(cut.row_id),
      sourceColumn = sheet.columns.indexOf(cut.column_id);
    const rows = cut.rows as number,
      columns = cut.columns as number;
    if (
      !Number.isSafeInteger(position.row) ||
      !Number.isSafeInteger(position.column) ||
      sourceRow < 0 ||
      sourceColumn < 0 ||
      position.row < 0 ||
      position.column < 0 ||
      sourceRow + rows > sheet.rows.length ||
      sourceColumn + columns > sheet.columns.length ||
      position.row + rows > sheet.rows.length ||
      position.column + columns > sheet.columns.length
    )
      return this.moveError('Диапазон перемещения выходит за границы листа.');
    if (sourceRow === position.row && sourceColumn === position.column)
      return this.moveError('Для перемещения выберите другую ячейку назначения.');
    const destinationRow = sheet.rows[position.row],
      destinationColumn = sheet.columns[position.column];
    if (!destinationRow || !destinationColumn) return invalid();
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(cut.version),
            clientSequence: ++this.sequence,
            moveRange: {
              source: {
                sheetId: uuidBytes(sheet.id),
                rowId: uuidBytes(cut.row_id),
                columnId: uuidBytes(cut.column_id),
              },
              destination: {
                sheetId: uuidBytes(sheet.id),
                rowId: uuidBytes(destinationRow),
                columnId: uuidBytes(destinationColumn),
              },
              rows,
              columns,
            },
          },
        },
      }),
    );
    this.cutIntent = { raw, version: cut.version, stale: false };
    this.pending.push({ kind: 'moveRange', id, cutPayload: raw, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    this.startMovePreview(id, {
      source: { sheet_id: sheet.id, row_id: cut.row_id, column_id: cut.column_id },
      destination: { sheet_id: sheet.id, row_id: destinationRow, column_id: destinationColumn },
      rows,
      columns,
    });
    return true;
  }
  copyRangePayload() {
    if (this.privacyMask || this.projection?.representation === 'anonymized') return undefined;
    if (!this.canCopy) return undefined;
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection || this.pending.length || this.editing) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    if ([...this.filterState.hidden].some((row) => row >= startRow && row <= endRow))
      return undefined;
    const rows = endRow - startRow + 1,
      columns = endColumn - startColumn + 1;
    if (rows * columns <= 1 || rows * columns > 1_000) return undefined;
    let hasFormula = false;
    for (let row = startRow; row <= endRow && !hasFormula; row++)
      for (let column = startColumn; column <= endColumn; column++)
        if (this.value({ row, column })?.type === 'formula') {
          hasFormula = true;
          break;
        }
    if (!hasFormula) return undefined;
    const row = sheet.rows[startRow],
      column = sheet.columns[startColumn];
    if (!row || !column) return undefined;
    return JSON.stringify({
      schema_version: 1,
      document_id: projection.document_id,
      version: projection.version,
      sheet_id: sheet.id,
      row_id: row,
      column_id: column,
      rows,
      columns,
    });
  }
  pasteRangeCopy(
    raw: string,
    position: Position = this.pasteOrigin,
    destinationExtent?: { rows: number; columns: number },
  ) {
    if (!this.canEdit) return false;
    const projection = this.projection,
      sheet = this.sheet,
      actor = this.actor;
    if (
      !projection ||
      !sheet ||
      !actor ||
      byteLength(raw) > 16 * 1024 ||
      this.pending.length ||
      this.pendingCount >= 128
    )
      return false;
    let payload: unknown;
    try {
      payload = JSON.parse(raw);
    } catch {
      return false;
    }
    if (!payload || typeof payload !== 'object') return false;
    const copy = payload as Record<string, unknown>;
    if (
      copy.schema_version !== 1 ||
      copy.document_id !== projection.document_id ||
      copy.version !== projection.version ||
      copy.sheet_id !== sheet.id ||
      typeof copy.row_id !== 'string' ||
      typeof copy.column_id !== 'string' ||
      !Number.isInteger(copy.rows) ||
      !Number.isInteger(copy.columns) ||
      (copy.rows as number) < 1 ||
      (copy.columns as number) < 1 ||
      (copy.rows as number) * (copy.columns as number) > 1_000
    ) {
      this.error = 'Источник диапазона изменился. Скопируйте его ещё раз.';
      this.notify();
      return false;
    }
    const sourceRow = sheet.rows.indexOf(copy.row_id),
      sourceColumn = sheet.columns.indexOf(copy.column_id);
    const rows = copy.rows as number,
      columns = copy.columns as number;
    const extent = destinationExtent ?? this.pasteExtent(rows, columns, position);
    if (
      !extent ||
      !Number.isInteger(extent.rows) ||
      !Number.isInteger(extent.columns) ||
      extent.rows < 1 ||
      extent.columns < 1 ||
      extent.rows % rows ||
      extent.columns % columns ||
      extent.rows * extent.columns > 1000
    ) {
      if (extent) {
        this.error = 'Размер диапазона заполнения недопустим или превышает лимит 1000 ячеек.';
        this.notify();
      }
      return false;
    }
    if (
      sourceRow < 0 ||
      sourceColumn < 0 ||
      sourceRow + rows > sheet.rows.length ||
      sourceColumn + columns > sheet.columns.length ||
      !Number.isInteger(position.row) ||
      !Number.isInteger(position.column) ||
      position.row < 0 ||
      position.column < 0 ||
      position.row + extent.rows > sheet.rows.length ||
      position.column + extent.columns > sheet.columns.length
    ) {
      this.error = 'Диапазон выходит за границы листа.';
      this.notify();
      return false;
    }
    const destinationRow = sheet.rows[position.row],
      destinationColumn = sheet.columns[position.column];
    if (!destinationRow || !destinationColumn) return false;
    const copyRange = {
      source: {
        sheetId: uuidBytes(sheet.id),
        rowId: uuidBytes(copy.row_id),
        columnId: uuidBytes(copy.column_id),
      },
      destination: {
        sheetId: uuidBytes(sheet.id),
        rowId: uuidBytes(destinationRow),
        columnId: uuidBytes(destinationColumn),
      },
      rows,
      columns,
    };
    const id = uuidV7();
    const bytes = toBinary(
      ClientEnvelopeSchema,
      create(ClientEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'command',
          value: {
            operationId: uuidBytes(id),
            documentId: uuidBytes(projection.document_id),
            actorId: actor,
            baseVersion: BigInt(projection.version),
            clientSequence: ++this.sequence,
            ...(destinationExtent === undefined &&
            extent.rows === rows &&
            extent.columns === columns
              ? { copyRange }
              : {
                  repeatRange: {
                    sourceBlock: copyRange,
                    destinationRows: extent.rows,
                    destinationColumns: extent.columns,
                  },
                }),
          },
        },
      }),
    );
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'copyRange', id, bytes, attempts: 0 });
    this.observeAssistant({ kind: 'paste' });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  async pasteFormulaCopy(raw: string, position: Position = this.pasteOrigin) {
    if (!this.canEdit) return false;
    const projection = this.projection;
    if (!projection || byteLength(raw) > 64 * 1024 || this.formulaCopyInFlight) return false;
    let payload: unknown;
    try {
      payload = JSON.parse(raw);
    } catch {
      return false;
    }
    if (!payload || typeof payload !== 'object') return false;
    const copy = payload as Record<string, unknown>;
    if (
      copy.schema_version !== 1 ||
      copy.document_id !== projection.document_id ||
      copy.version !== projection.version ||
      copy.sheet_id !== this.sheet?.id ||
      typeof copy.source !== 'string' ||
      !copy.source.startsWith('=') ||
      byteLength(copy.source) > 32 * 1024 ||
      !Number.isInteger(copy.from_row) ||
      !Number.isInteger(copy.from_column) ||
      (copy.from_row as number) < 0 ||
      (copy.from_column as number) < 0 ||
      (copy.from_row as number) >= 1_048_576 ||
      (copy.from_column as number) >= 16_384 ||
      !this.sheet?.rows[position.row] ||
      !this.sheet.columns[position.column]
    ) {
      this.error = 'Источник формулы изменился. Скопируйте ячейку ещё раз.';
      this.notify();
      return false;
    }
    const row = this.sheet.rows[position.row],
      column = this.sheet.columns[position.column];
    const extent = this.pasteExtent(1, 1, position);
    if (!extent) return false;
    if (extent.rows * extent.columns > 1) {
      return this.pasteRangeCopy(
        JSON.stringify({
          schema_version: 1,
          document_id: projection.document_id,
          version: projection.version,
          sheet_id: this.sheet.id,
          row_id: this.sheet.rows[copy.from_row as number],
          column_id: this.sheet.columns[copy.from_column as number],
          rows: 1,
          columns: 1,
        }),
        position,
      );
    }
    this.formulaCopyInFlight = true;
    try {
      const source = await copyFormulaInWorker({
        source: copy.source,
        fromRow: copy.from_row as number,
        fromColumn: copy.from_column as number,
        toRow: position.row,
        toColumn: position.column,
      });
      if (
        this.projection?.version !== projection.version ||
        this.sheet?.rows[position.row] !== row ||
        this.sheet.columns[position.column] !== column
      ) {
        this.error = 'Структура таблицы изменилась. Скопируйте ячейку ещё раз.';
        this.notify();
        return false;
      }
      return this.set(position, parseInput(source));
    } catch {
      this.error = 'Не удалось скопировать формулу.';
      this.notify();
      return false;
    } finally {
      this.formulaCopyInFlight = false;
    }
  }
  get canAutofill() {
    return Boolean(
      this.canEdit &&
      this.canCopy &&
      this.sheet &&
      !this.editing &&
      !this.pending.length &&
      !this.privacyMask &&
      this.filterState.hidden.size === 0 &&
      !this.sheet.merges?.length,
    );
  }
  planAutofill(source: FillBounds, target: Position) {
    const sheet = this.sheet;
    if (!sheet || !this.canAutofill) return undefined;
    const vertical = source.startColumn === source.endColumn;
    const horizontal = source.startRow === source.endRow;
    const length = vertical
      ? source.endRow - source.startRow + 1
      : horizontal
        ? source.endColumn - source.startColumn + 1
        : 0;
    const seed: number[] = [];
    if (length >= 2 && length <= 1000)
      for (let i = 0; i < length; i++) {
        const value = this.value({
          row: source.startRow + (vertical ? i : 0),
          column: source.startColumn + (vertical ? 0 : i),
        });
        if (value?.type !== 'number') break;
        seed.push(value.value);
      }
    const series = seed.length === length && length >= 2 && !!numericSeries(seed, length, 1);
    const plan = fillPlan(source, target, sheet.rows.length, sheet.columns.length, series);
    if (!plan) return undefined;
    // A numeric seed only extrapolates along its own axis.
    if (series && ((vertical && plan.axis === 'row') || (horizontal && plan.axis === 'column'))) {
      const offset = vertical
        ? plan.destination.startRow - source.startRow
        : plan.destination.startColumn - source.startColumn;
      const count = vertical
        ? plan.destination.endRow - plan.destination.startRow + 1
        : plan.destination.endColumn - plan.destination.startColumn + 1;
      const numbers = numericSeries(seed, offset, count);
      return numbers ? { ...plan, numbers } : undefined;
    }
    return { ...plan, numbers: undefined };
  }
  autofill(source: FillBounds, target: Position, version: string) {
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection || projection.version !== version || !this.canAutofill) return false;
    const plan = this.planAutofill(source, target);
    if (!plan) {
      this.error = 'Заполнение требует целых блоков и не более 1000 новых ячеек.';
      this.notify();
      return false;
    }
    const destination = plan.destination;
    const position = { row: destination.startRow, column: destination.startColumn };
    const rows = destination.endRow - destination.startRow + 1,
      columns = destination.endColumn - destination.startColumn + 1;
    const ok = plan.numbers
      ? this.pasteMatrix(
          plan.axis === 'row' ? plan.numbers.map((n) => [String(n)]) : [plan.numbers.map(String)],
          position,
          false,
          [],
          { rows, columns },
        )
      : this.pasteRangeCopy(
          JSON.stringify({
            schema_version: 1,
            document_id: projection.document_id,
            version,
            sheet_id: sheet.id,
            row_id: sheet.rows[source.startRow],
            column_id: sheet.columns[source.startColumn],
            rows: source.endRow - source.startRow + 1,
            columns: source.endColumn - source.startColumn + 1,
          }),
          position,
          { rows, columns },
        );
    if (ok) {
      this.select({ row: plan.range.startRow, column: plan.range.startColumn });
      this.select({ row: plan.range.endRow, column: plan.range.endColumn }, true);
    }
    return ok;
  }
  fillDown() {
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection || this.editing || this.pending.length) return false;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    const singleRow = endRow === startRow;
    if (singleRow && startRow === 0) return false;
    const sourceRow = singleRow ? startRow - 1 : startRow;
    const destinationRow = singleRow ? startRow : startRow + 1;
    const columns = endColumn - startColumn + 1;
    return this.pasteRangeCopy(
      JSON.stringify({
        schema_version: 1,
        document_id: projection.document_id,
        version: projection.version,
        sheet_id: sheet.id,
        row_id: sheet.rows[sourceRow],
        column_id: sheet.columns[startColumn],
        rows: 1,
        columns,
      }),
      { row: destinationRow, column: startColumn },
      { rows: endRow - destinationRow + 1, columns },
    );
  }
  pasteTsv(text: string, position: Position = this.pasteOrigin, preserveTrailingBlank = false) {
    if (!this.canEdit) return false;
    try {
      return this.pasteMatrix(parseDelimited(text, '\t', preserveTrailingBlank), position);
    } catch (error) {
      this.error = error instanceof Error ? error.message : 'Не удалось разобрать вставку.';
      this.notify();
      return false;
    }
  }
  pasteMatrix(
    rows: CellMatrix,
    position: Position = this.pasteOrigin,
    literal = false,
    alignment: readonly (string | null)[] = [],
    explicitExtent?: { rows: number; columns: number },
  ) {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (!sheet || !projection || !actor) return false;
    if (
      !Number.isInteger(position.row) ||
      !Number.isInteger(position.column) ||
      position.row < 0 ||
      position.column < 0 ||
      rows.length === 0
    )
      return false;
    const sourceCount = rows.reduce((count, row) => count + row.length, 0);
    if (sourceCount > 1000) {
      this.error = 'Диапазон вставки превышает лимит 1000 ячеек.';
      this.notify();
      return false;
    }
    const width = Math.max(...rows.map((row) => row.length));
    if (width === 0) return false;
    const extent = explicitExtent ?? this.pasteExtent(rows.length, width, position);
    if (!extent) return false;
    if (extent.rows !== rows.length || extent.columns !== width) {
      const source = rows;
      rows = Array.from({ length: extent.rows }, (_, row) =>
        Array.from(
          { length: extent.columns },
          (_, column) => source[row % source.length]?.[column % width] ?? '',
        ),
      );
    }
    const count = rows.reduce((total, row) => total + row.length, 0);
    if (
      count < 1 ||
      count > 1000 ||
      this.pendingCount >= 128 ||
      rows.some(
        (row, rowIndex) =>
          position.row + rowIndex >= sheet.rows.length ||
          position.column + row.length > sheet.columns.length,
      )
    ) {
      this.error = 'Диапазон вставки превышает границы листа или лимит 1000 ячеек.';
      this.notify();
      return false;
    }
    const writes: PendingBatch['writes'] = [];
    const wireWrites = [];
    const optimisticStyles: NonNullable<PendingBatch['styles']> = [];
    const wireStyles = [];
    let textBytes = 0;
    for (let r = 0; r < rows.length; r++) {
      const fields = rows[r] ?? [];
      for (let c = 0; c < fields.length; c++) {
        const field = fields[c] ?? '';
        const value: Value =
          literal && field !== '' ? { type: 'text', value: field } : parseInput(field);
        if (value.type === 'formula') {
          this.error = 'Массовая вставка формул пока не поддерживается. Введите формулу в ячейку.';
          this.notify();
          return false;
        }
        if (value.type === 'text') textBytes += byteLength(value.value);
        if (
          textBytes > 512 * 1024 ||
          (value.type === 'text' && byteLength(value.value) > 32 * 1024)
        ) {
          this.error = 'Текст вставки превышает лимит 512 КБ или 32 КБ на ячейку.';
          this.notify();
          return false;
        }
        const row = sheet.rows[position.row + r]!,
          column = sheet.columns[position.column + c]!;
        writes.push({ row, column, value });
        wireWrites.push(
          create(SetCellValueSchema, {
            address: {
              sheetId: uuidBytes(sheet.id),
              rowId: uuidBytes(row),
              columnId: uuidBytes(column),
            },
            value: create(CellValueSchema, {
              kind:
                value.type === 'blank'
                  ? { case: 'blank', value: true }
                  : value.type === 'number'
                    ? { case: 'number', value: value.value }
                    : value.type === 'boolean'
                      ? { case: 'boolean', value: value.value }
                      : { case: 'text', value: value.value },
            }),
          }),
        );
        const horizontal = alignment[c % width];
        if (horizontal === 'left' || horizontal === 'center' || horizontal === 'right') {
          const changes: StyleChange[] = [{ property: 'horizontal', value: horizontal }];
          optimisticStyles.push({ row, column, changes });
          wireStyles.push({
            address: {
              sheetId: uuidBytes(sheet.id),
              rowId: uuidBytes(row),
              columnId: uuidBytes(column),
            },
            patch: encodeStylePatch(changes),
          });
        }
      }
    }
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          ...(wireStyles.length > 0
            ? { setCellBatch: { writes: wireWrites, styles: wireStyles } }
            : {
                operation: create(CanonicalOperationSchema, {
                  kind: { case: 'setCellValues', value: { writes: wireWrites } },
                }),
              }),
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) {
      this.error = 'Команда вставки превышает лимит 1 МБ.';
      this.notify();
      return false;
    }
    this.pending.push({
      kind: 'batch',
      id,
      writes,
      ...(optimisticStyles.length > 0 ? { styles: optimisticStyles } : {}),
      bytes,
      attempts: 0,
    });
    this.observeAssistant({ kind: 'paste' });
    this.syncPreview();
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  private pasteExtent(rows: number, columns: number, position: Position) {
    const bounds = this.selectionBounds;
    const selectedRows = bounds.endRow - bounds.startRow + 1;
    const selectedColumns = bounds.endColumn - bounds.startColumn + 1;
    if (
      position.row !== bounds.startRow ||
      position.column !== bounds.startColumn ||
      selectedRows * selectedColumns === 1
    )
      return { rows, columns };
    if (selectedRows % rows !== 0 || selectedColumns % columns !== 0) {
      this.error = 'Размер выделения должен быть кратен размеру вставляемого блока.';
      this.notify();
      return undefined;
    }
    if (selectedRows * selectedColumns > 1000) {
      this.error = 'Диапазон вставки превышает лимит 1000 ячеек.';
      this.notify();
      return undefined;
    }
    return { rows: selectedRows, columns: selectedColumns };
  }
  addSheet() {
    if (!this.canEdit) return false;
    const projection = this.projection,
      actor = this.actor;
    if (!projection || !actor || projection.sheets.length >= 64 || this.pendingCount >= 128)
      return false;
    const names = new Set(projection.sheets.map((sheet) => sheet.name.toLowerCase()));
    let number = 1;
    while (names.has(`лист ${number}`)) number++;
    const sheetId = uuidV7();
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'addSheet',
              value: {
                sheetId: uuidBytes(sheetId),
                name: `Лист ${number}`,
                rows: Array.from({ length: 1_000 }, () => uuidBytes(uuidV7())),
                columns: Array.from({ length: 26 }, () => uuidBytes(uuidV7())),
              },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'addSheet', id, sheetId, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  extendRows() {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (
      !sheet ||
      !projection ||
      !actor ||
      sheet.rows.length >= 100_000 ||
      this.personalView.settings.filters.some((filter) => filter.sheet_id === sheet.id) ||
      this.pending.some((pending) => pending.kind === 'extendRows') ||
      this.pendingCount >= 128
    )
      return false;
    const rows = Array.from({ length: Math.min(1_000, 100_000 - sheet.rows.length) }, uuidV7);
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'extendRows',
              value: { sheetId: uuidBytes(sheet.id), rows: rows.map(uuidBytes) },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'extendRows', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  insertRowBeforeSelection() {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const before = sheet?.rows[this.selected.row];
    if (
      !sheet ||
      !projection ||
      !actor ||
      !before ||
      sheet.rows.length >= 100_000 ||
      this.pending.some((pending) => pending.kind === 'insertRows') ||
      this.pendingCount >= 128
    )
      return false;
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'insertRows',
              value: {
                sheetId: uuidBytes(sheet.id),
                beforeRowId: uuidBytes(before),
                rows: [uuidBytes(uuidV7())],
              },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'insertRows', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  insertColumnBeforeSelection() {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const before = sheet?.columns[this.selected.column];
    if (
      !sheet ||
      !projection ||
      !actor ||
      !before ||
      sheet.columns.length >= 1_000 ||
      this.pending.some((pending) => pending.kind === 'insertColumns') ||
      this.pendingCount >= 128
    )
      return false;
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'insertColumns',
              value: {
                sheetId: uuidBytes(sheet.id),
                beforeColumnId: uuidBytes(before),
                columns: [uuidBytes(uuidV7())],
              },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'insertColumns', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  deleteSelectedColumn() {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const column = sheet?.columns[this.selected.column];
    if (
      !sheet ||
      !projection ||
      !actor ||
      !column ||
      sheet.columns.length <= 1 ||
      this.pending.some((pending) => pending.kind === 'deleteColumns') ||
      this.pendingCount >= 128
    )
      return false;
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'deleteColumns',
              value: { sheetId: uuidBytes(sheet.id), columns: [uuidBytes(column)] },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'deleteColumns', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  deleteSelectedRow() {
    if (!this.canEdit) return false;
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    const row = sheet?.rows[this.selected.row];
    if (
      !sheet ||
      !projection ||
      !actor ||
      !row ||
      sheet.rows.length <= 1 ||
      this.pending.some((pending) => pending.kind === 'deleteRows') ||
      this.pendingCount >= 128
    )
      return false;
    const id = uuidV7();
    const message = create(ClientEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'command',
        value: {
          operationId: uuidBytes(id),
          documentId: uuidBytes(projection.document_id),
          actorId: actor,
          baseVersion: BigInt(projection.version),
          clientSequence: ++this.sequence,
          operation: {
            kind: {
              case: 'deleteRows',
              value: { sheetId: uuidBytes(sheet.id), rows: [uuidBytes(row)] },
            },
          },
        },
      },
    });
    const bytes = toBinary(ClientEnvelopeSchema, message);
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'deleteRows', id, bytes, attempts: 0 });
    this.notify();
    this.flush();
    return true;
  }
  private flush() {
    if (!this.canEdit) return;
    if (this.inflight || !this.sender || this.syncing) return;
    const op = this.pending[0];
    if (!op) return;
    if (op.attempts >= 6) {
      this.error = 'Изменение не подтверждено. Переподключитесь для повторной попытки.';
      this.notify();
      return;
    }
    if (this.sender(op.bytes)) {
      op.attempts++;
      this.inflight = op.id;
      this.submittedAt.set(op.id, performance.now());
      this.timer = setTimeout(
        () => {
          this.inflight = undefined;
          this.flush();
        },
        Math.min(30000, 2000 * 2 ** (op.attempts - 1)),
      );
    }
  }
  private apply(operation: CanonicalOperation) {
    if (operation.kind.case === 'mergeWrite') {
      const update = operation.kind.value,
        sheet = this.projection?.sheets.find((item) => item.id === bytesUuid(update.sheetId));
      const range = update.range;
      if (
        !sheet ||
        update.sheetId.length !== 16 ||
        !range ||
        [range.firstRowId, range.lastRowId, range.firstColumnId, range.lastColumnId].some(
          (id) => id.length !== 16,
        )
      )
        throw new Error('Invalid canonical merged range');
      const next: MergeRange = {
        first_row_id: bytesUuid(range.firstRowId),
        last_row_id: bytesUuid(range.lastRowId),
        first_column_id: bytesUuid(range.firstColumnId),
        last_column_id: bytesUuid(range.lastColumnId),
      };
      const existing = sheet.merges ?? [],
        index = existing.findIndex(
          (item) =>
            item.first_row_id === next.first_row_id &&
            item.last_row_id === next.last_row_id &&
            item.first_column_id === next.first_column_id &&
            item.last_column_id === next.last_column_id,
        );
      if ((update.merged && index >= 0) || (!update.merged && index < 0))
        throw new Error('Canonical merge membership preimage mismatch');
      if (update.merged) sheet.merges = [...existing, next];
      else sheet.merges = existing.filter((_, i) => i !== index);
      return;
    }
    if (operation.kind.case === 'frozenRowsWrite') {
      const update = operation.kind.value;
      const sheet = this.projection?.sheets.find((item) => item.id === bytesUuid(update.sheetId));
      if (!sheet || update.sheetId.length !== 16 || update.rows > Math.min(1000, sheet.rows.length))
        throw new Error('Invalid canonical frozen rows');
      sheet.frozen_rows = update.rows;
      return;
    }
    if (operation.kind.case === 'axisSizeWrite') {
      const update = operation.kind.value;
      const sheet = this.projection?.sheets.find((item) => item.id === bytesUuid(update.sheetId));
      if (
        !sheet ||
        update.sheetId.length !== 16 ||
        update.writes.length < 1 ||
        update.writes.length > 1000
      )
        throw new Error('Invalid canonical axis-size change');
      const rows = new Map(
        (sheet.row_heights ?? []).map((item) => [item.axis_id, item.size_css_px]),
      );
      const columns = new Map(
        (sheet.column_widths ?? []).map((item) => [item.axis_id, item.size_css_px]),
      );
      const seen = new Set<string>();
      for (const write of update.writes) {
        if (write.axisId.length !== 16 || (write.axisKind !== 1 && write.axisKind !== 2))
          throw new Error('Invalid canonical axis-size target');
        const id = bytesUuid(write.axisId);
        const target = write.axisKind === 1 ? rows : columns;
        const ids = write.axisKind === 1 ? sheet.rows : sheet.columns;
        const minimum = write.axisKind === 1 ? 12 : 24,
          maximum = write.axisKind === 1 ? 1000 : 2000;
        if (
          !ids.includes(id) ||
          seen.has(`${write.axisKind}/${id}`) ||
          (write.sizeCssPx !== undefined &&
            (!Number.isInteger(write.sizeCssPx) ||
              write.sizeCssPx < minimum ||
              write.sizeCssPx > maximum))
        )
          throw new Error('Invalid canonical axis-size value');
        seen.add(`${write.axisKind}/${id}`);
        if (write.sizeCssPx === undefined || write.sizeCssPx === (write.axisKind === 1 ? 28 : 112))
          target.delete(id);
        else target.set(id, write.sizeCssPx);
      }
      sheet.row_heights = [...rows].map(([axis_id, size_css_px]) => ({ axis_id, size_css_px }));
      sheet.column_widths = [...columns].map(([axis_id, size_css_px]) => ({
        axis_id,
        size_css_px,
      }));
      return;
    }
    if (operation.kind.case === 'setCellValues') {
      const next = new Map(this.confirmed);
      const touched = new Set<string>();
      if (!operation.kind.value.writes.length || operation.kind.value.writes.length > 1000)
        throw new Error('Invalid canonical batch');
      for (const write of operation.kind.value.writes) {
        const address = write.address,
          value = write.value?.kind;
        const sheet =
          address &&
          this.projection?.sheets.find((sheet) => sheet.id === bytesUuid(address.sheetId));
        if (
          !address ||
          address.sheetId.length !== 16 ||
          address.rowId.length !== 16 ||
          address.columnId.length !== 16 ||
          !sheet?.rows.includes(bytesUuid(address.rowId)) ||
          !sheet.columns.includes(bytesUuid(address.columnId)) ||
          !value?.case ||
          (value.case === 'number' && !Number.isFinite(value.value))
        )
          throw new Error('Invalid canonical batch');
        const cellKey = key(bytesUuid(address.rowId), bytesUuid(address.columnId));
        if (touched.has(cellKey)) throw new Error('Duplicate canonical batch cell');
        touched.add(cellKey);
        if (value.case === 'blank') next.delete(cellKey);
        else next.set(cellKey, { type: value.case, value: value.value } as Value);
      }
      this.confirmed = next;
      for (const address of touched) this.previewDirty.add(address);
      return;
    }
    if (
      operation.kind.case === 'extendRows' ||
      operation.kind.case === 'insertRows' ||
      operation.kind.case === 'deleteRows' ||
      operation.kind.case === 'insertColumns' ||
      operation.kind.case === 'deleteColumns' ||
      operation.kind.case === 'addSheet'
    )
      throw new Error('Structural operation requires a canonical projection refresh');
    const entry = operation.kind.value;
    const address = entry?.address;
    if (
      !address ||
      address.sheetId.length !== 16 ||
      address.rowId.length !== 16 ||
      address.columnId.length !== 16
    )
      throw new Error('Invalid canonical address');
    const sheet = this.projection?.sheets.find((sheet) => sheet.id === bytesUuid(address.sheetId));
    if (
      !sheet ||
      !sheet.rows.includes(bytesUuid(address.rowId)) ||
      !sheet.columns.includes(bytesUuid(address.columnId))
    )
      throw new Error('Unknown canonical axis');
    const k = key(bytesUuid(address.rowId), bytesUuid(address.columnId));
    this.previewDirty.add(k);
    if (operation.kind.case === 'clearCell') {
      this.confirmed.delete(k);
      return;
    }
    if (operation.kind.case === 'setCellValue') {
      const value = operation.kind.value.value?.kind;
      if (!value || !value.case || (value.case === 'number' && !Number.isFinite(value.value)))
        throw new Error('Invalid canonical value');
      if (value.case === 'blank') this.confirmed.delete(k);
      else this.confirmed.set(k, { type: value.case, value: value.value } as Value);
      return;
    }
    if (operation.kind.case === 'setCellFormula') {
      const formula = operation.kind.value.formula;
      if (
        !formula ||
        !formula.ast ||
        formula.compatibilityProfile !== 1 ||
        !formula.origin ||
        bytesUuid(formula.origin.sheetId) !== sheet.id ||
        bytesUuid(formula.origin.rowId) !== bytesUuid(address.rowId) ||
        bytesUuid(formula.origin.columnId) !== bytesUuid(address.columnId)
      )
        throw new Error('Invalid canonical formula');
      this.confirmed.set(k, {
        type: 'formula',
        value: decodeFormula({
          source: formula.source,
          current_text: formula.canonical,
          canonical: formula.canonical,
          ast_schema: formula.ast.schemaVersion,
          compatibility_profile: formulaProfile,
          bound_at_version: String(formula.boundAtVersion),
          bound_formula_wire_hex: [...toBinary(BoundFormulaSchema, formula)]
            .map((byte) => byte.toString(16).padStart(2, '0'))
            .join(''),
        }),
      });
      return;
    }
    throw new Error('Unknown canonical operation');
  }
  async receive(envelope: ServerEnvelope) {
    if (this.guestAccessClosed) return;
    const payload = envelope.payload;
    if (
      this.isGuest &&
      (payload.case === 'committed' ||
        (payload.case === 'documentHeadChanged' && payload.value.latestOperation))
    )
      throw new Error('Гостевое соединение получило историю вместо текущего состояния');
    if (payload.case === 'presenceSnapshot') {
      const snapshot = payload.value;
      if (snapshot.documentId.length !== 16 || snapshot.peers.length > 64)
        throw new Error('Invalid presence snapshot');
      if (bytesUuid(snapshot.documentId) !== this.projection?.document_id) return;
      const sheet = this.sheet;
      if (!sheet) return;
      const rows = new Map(sheet.rows.map((id, index) => [id, index]));
      const columns = new Map(sheet.columns.map((id, index) => [id, index]));
      const peers: RemotePresence[] = [];
      const seen = new Set<string>();
      for (const peer of snapshot.peers) {
        if (peer.sessionId.length !== 16 || peer.actorId.length !== 16)
          throw new Error('Invalid presence peer');
        const sessionId = bytesUuid(peer.sessionId);
        if (seen.has(sessionId)) throw new Error('Duplicate presence peer');
        seen.add(sessionId);
        if (!peer.anchor || !peer.focus) continue;
        if (
          peer.anchor.sheetId.length !== 16 ||
          peer.focus.sheetId.length !== 16 ||
          peer.anchor.rowId.length !== 16 ||
          peer.focus.rowId.length !== 16 ||
          peer.anchor.columnId.length !== 16 ||
          peer.focus.columnId.length !== 16
        )
          throw new Error('Invalid presence address');
        if (
          bytesUuid(peer.anchor.sheetId) !== sheet.id ||
          bytesUuid(peer.focus.sheetId) !== sheet.id
        )
          continue;
        const anchorRow = rows.get(bytesUuid(peer.anchor.rowId));
        const focusRow = rows.get(bytesUuid(peer.focus.rowId));
        const anchorColumn = columns.get(bytesUuid(peer.anchor.columnId));
        const focusColumn = columns.get(bytesUuid(peer.focus.columnId));
        if (
          anchorRow === undefined ||
          focusRow === undefined ||
          anchorColumn === undefined ||
          focusColumn === undefined
        )
          continue;
        const actorId = bytesUuid(peer.actorId);
        if (actorId === (this.actor && bytesUuid(this.actor))) continue;
        peers.push({
          sessionId,
          actorId,
          anchor: { row: anchorRow, column: anchorColumn },
          focus: { row: focusRow, column: focusColumn },
        });
      }
      this.remotePresence = peers;
      this.notify();
      return;
    }
    if (payload.case === 'replayComplete') {
      const marker = payload.value;
      if (marker.documentId.length !== 16 || marker.barrierVersion > 9223372036854775807n)
        throw new Error('Invalid replay barrier');
      if (bytesUuid(marker.documentId) !== this.projection?.document_id) return;
      if (marker.barrierVersion > this.historyThrough) this.dirtyHistory(marker.barrierVersion);
      this.observeHistoryVersion(marker.barrierVersion);
      if (marker.barrierVersion > BigInt(this.projection.version)) {
        if (marker.barrierVersion > this.headRefreshVersion)
          this.headRefreshVersion = marker.barrierVersion;
        await this.refreshHead();
      }
      return;
    }
    if (payload.case === 'documentHeadChanged') {
      const hint = payload.value;
      if (
        hint.documentId.length !== 16 ||
        hint.headVersion === 0n ||
        hint.headVersion > 9223372036854775807n ||
        ![0, 1, 2].includes(hint.headEventKind) ||
        (hint.headEventKind === 1 ? hint.headActorId.length !== 16 : hint.headActorId.length !== 0)
      )
        throw new Error('Invalid document head hint');
      if (bytesUuid(hint.documentId) !== this.projection?.document_id) return;
      this.observeHistoryVersion(hint.headVersion);
      const localEvent =
        hint.latestOperation &&
        (this.pending.some(
          (pending) => pending.id === bytesUuid(hint.latestOperation!.operationId),
        ) ||
          this.historyVersions.has(bytesUuid(hint.latestOperation.operationId)));
      if (
        hint.headVersion > this.historyThrough &&
        (hint.headEventKind === 0 ||
          (hint.headEventKind === 1 &&
            !localEvent &&
            bytesUuid(hint.headActorId) === (this.actor && bytesUuid(this.actor))))
      )
        this.dirtyHistory(hint.headVersion);
      if (hint.headVersion > BigInt(this.projection.version)) {
        const event = hint.latestOperation;
        if (
          event?.canonicalOperation &&
          hint.headVersion === BigInt(this.projection.version) + 1n &&
          event.canonicalOperation?.kind.case !== 'extendRows' &&
          event.canonicalOperation?.kind.case !== 'insertRows' &&
          event.canonicalOperation?.kind.case !== 'deleteRows' &&
          event.canonicalOperation?.kind.case !== 'insertColumns' &&
          event.canonicalOperation?.kind.case !== 'deleteColumns' &&
          event.canonicalOperation?.kind.case !== 'addSheet'
        ) {
          if (
            event.documentId.length !== 16 ||
            bytesUuid(event.documentId) !== this.projection.document_id ||
            event.operationId.length !== 16 ||
            event.actorId.length !== 16 ||
            event.committedVersion !== hint.headVersion ||
            event.confirmedStateHash.length !== 32
          )
            throw new Error('Invalid canonical head event');
          this.apply(event.canonicalOperation);
          this.projection = {
            ...this.projection,
            version: String(hint.headVersion),
            state_hash: [...event.confirmedStateHash]
              .map((byte) => byte.toString(16).padStart(2, '0'))
              .join(''),
          };
          this.pruneHistory();
          this.restartCalculation();
          this.syncPreview();
          this.notify();
          if (BigInt(this.projection.version) >= this.headRefreshVersion) {
            clearTimeout(this.headRefreshTimer);
            this.headRefreshAttempts = 0;
          }
          return;
        }
        if (hint.headVersion > this.headRefreshVersion) this.headRefreshVersion = hint.headVersion;
        await this.refreshHead();
      }
      return;
    }
    if (payload.case === 'calculationCommitted') {
      this.receiveCalculation(payload.value);
      return;
    }
    if (payload.case !== 'committed' && payload.case !== 'receipt' && payload.case !== 'rejected')
      return;
    const message = payload.value;
    if (message.documentId.length !== 16 || message.operationId.length !== 16)
      throw new Error('Invalid operation identity');
    if (bytesUuid(message.documentId) !== this.projection?.document_id) return;
    const id = bytesUuid(message.operationId);
    const pending = this.pending.find((op) => op.id === id);
    clearTimeout(this.timer);
    this.inflight = undefined;
    if (payload.case === 'rejected') {
      this.assistant.reset(false);
      const rejection = payload.value;
      const sourceChanged =
        rejection.code === 17 &&
        (pending?.kind === 'copyRange' ||
          pending?.kind === 'moveRange' ||
          pending?.kind === 'sortRange');
      const unsupportedMove = pending?.kind === 'moveRange' && rejection.code === 18;
      if (rejection.retryable && !sourceChanged && !unsupportedMove) {
        // An admission failure can follow a committed move whose ACK was lost.
        // Its original bytes and ID must survive until a receipt resolves the outcome.
        this.error =
          pending?.kind === 'moveRange'
            ? 'Не удалось подтвердить перемещение. Повторяем запрос…'
            : 'Сервер занят. Повторяем изменение…';
        this.timer = setTimeout(() => this.flush(), 1500);
        this.notify();
        return;
      }
      if (pending?.kind === 'moveRange') {
        this.pending = this.pending.filter((p) => p.id !== id);
        this.syncPreview();
        const stale = rejection.code === 17 || rejection.code === 4;
        if (stale && this.cutIntent?.raw === pending.cutPayload) this.cutIntent.stale = true;
        if (stale || rejection.resyncRequired) await this.refresh();
        this.error = stale
          ? 'Источник диапазона изменился. Вырежьте его ещё раз.'
          : `Перемещение отклонено. ${rejectionText[rejection.code] ?? 'Обновите таблицу и вырежьте диапазон ещё раз.'}`;
        this.notify();
        this.flush();
        return;
      }
      if (pending?.kind === 'copyRange' && rejection.code === 17) {
        this.pending = this.pending.filter((p) => p.id !== id);
        this.error = 'Источник диапазона изменился. Скопируйте его ещё раз.';
        await this.refresh();
        this.notify();
        return;
      }
      this.pending = this.pending.filter((p) => p.id !== id);
      if (pending?.kind === 'undo') {
        const history = pending.direction === 'undo' ? this.undoHistory : this.redoHistory;
        if (history.at(-1) === pending.target) history.pop();
      }
      this.error = `Изменение отклонено. ${rejectionText[rejection.code] ?? 'Неизвестная причина; обновите страницу.'}`;
      if (rejection.resyncRequired) await this.refresh();
      if (pending?.kind === 'sortRange')
        this.error =
          rejection.code === 19
            ? 'Подтверждённый расчёт ещё не готов. Дождитесь результатов и повторите сортировку.'
            : rejection.code === 17
              ? 'Книга изменилась. Проверьте границы и повторите сортировку.'
              : rejection.code === 16
                ? 'Сортировка превышает предел события 4 МиБ; книга не изменена.'
                : 'Сортировка отклонена. Проверьте границы, предел 100 000 ячеек и объединения в теле диапазона.';
    } else {
      if (payload.case === 'committed') {
        const submittedAt = this.submittedAt.get(id);
        if (submittedAt !== undefined)
          this.performanceMetrics.serverConfirmationMs = performance.now() - submittedAt;
        this.submittedAt.delete(id);
      }
      this.observeHistoryVersion(payload.value.committedVersion);
      if (
        payload.case === 'receipt' &&
        payload.value.currentHeadVersion > this.historyObservedThrough
      ) {
        // A receipt identifies its target, not the authors of the following tail.
        this.dirtyHistory(payload.value.currentHeadVersion);
      }
      if (
        payload.case === 'committed' &&
        payload.value.committedVersion === BigInt(this.projection.version) + 1n &&
        payload.value.canonicalOperation &&
        payload.value.canonicalOperation.kind.case !== 'extendRows' &&
        payload.value.canonicalOperation.kind.case !== 'insertRows' &&
        payload.value.canonicalOperation.kind.case !== 'deleteRows' &&
        payload.value.canonicalOperation.kind.case !== 'insertColumns' &&
        payload.value.canonicalOperation.kind.case !== 'deleteColumns' &&
        payload.value.canonicalOperation.kind.case !== 'addSheet'
      ) {
        if (payload.value.confirmedStateHash.length !== 32) throw new Error('Invalid state hash');
        this.apply(payload.value.canonicalOperation);
        this.projection = {
          ...this.projection,
          version: String(payload.value.committedVersion),
          state_hash: [...payload.value.confirmedStateHash]
            .map((v) => v.toString(16).padStart(2, '0'))
            .join(''),
        };
        this.restartCalculation();
      } else {
        const requiredVersion =
          payload.case === 'receipt'
            ? payload.value.currentHeadVersion
            : payload.value.committedVersion;
        if (!(await this.refresh(requiredVersion))) {
          this.notify();
          this.timer = setTimeout(() => this.flush(), 2000);
          return;
        }
      }
      // A receipt refresh can overlap a canonical ACK for the same command.
      // Consume history only if this intent is still pending after the await.
      const completed = this.pending.find((p) => p.id === id);
      this.pending = this.pending.filter((p) => p.id !== id);
      if (completed?.kind === 'moveRange' && this.cutIntent?.raw === completed.cutPayload)
        this.cutIntent = undefined;
      if (
        completed?.kind === 'addSheet' &&
        this.projection?.sheets.some((sheet) => sheet.id === completed.sheetId)
      )
        this.selectSheet(completed.sheetId);
      const committedVersion = payload.value.committedVersion;
      if (completed) {
        this.historyGeneration++;
        this.historyAbort?.abort();
        this.historyRequiredThrough =
          this.historyRequiredThrough > committedVersion
            ? this.historyRequiredThrough
            : committedVersion;
      }
      if (completed && committedVersion > this.historyThrough) {
        this.historyVersions.set(completed.id, committedVersion);
        if (
          completed?.kind === 'cell' ||
          completed?.kind === 'batch' ||
          completed?.kind === 'copyRange' ||
          completed?.kind === 'sortRange' ||
          completed?.kind === 'moveRange' ||
          completed?.kind === 'insertRows' ||
          completed?.kind === 'deleteRows' ||
          completed?.kind === 'insertColumns' ||
          completed?.kind === 'deleteColumns' ||
          completed?.kind === 'style'
        ) {
          this.undoHistory.push(completed.id);
          if (this.undoHistory.length > 100) this.undoHistory.shift();
          this.redoHistory = [];
        } else if (completed?.kind === 'undo') {
          const from = completed.direction === 'undo' ? this.undoHistory : this.redoHistory;
          const to = completed.direction === 'undo' ? this.redoHistory : this.undoHistory;
          if (from.at(-1) === completed.target) {
            from.pop();
            to.push(completed.id);
            if (to.length > 100) to.shift();
          }
        } else if (completed) {
          this.undoHistory = [];
          this.redoHistory = [];
        }
      }
      this.pruneHistory();
      this.error = '';
    }
    this.syncPreview();
    this.notify();
    this.flush();
    this.scheduleHistory();
  }
}
