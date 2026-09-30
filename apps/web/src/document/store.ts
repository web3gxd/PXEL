import { create, toBinary } from '@bufbuild/protobuf';
import { PreviewClient, type PreviewStatus } from '../calculation/client';
import { copyFormulaInWorker } from '../calculation/formula-copy';
import { readHistory } from './history';
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
export type Sheet = {
  id: string;
  name: string;
  rows: string[];
  columns: string[];
  cells: { row_id: string; column_id: string; value: Value }[];
  styles?: StyledCell[];
};
export type Projection = AuthoredMetadata & {
  document_id: string;
  version: string;
  state_hash: string;
  calculation_context?: PreviewContext | null;
  sheets: Sheet[];
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
  | PendingStyle;
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
  const p = input as Partial<Projection>;
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
    if (p.authored_projection_schema_version !== 4 && totalCells > 100000) throw invalid();
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
        v.value = decodeFormula(v.value, p.authored_projection_schema_version === 4);
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
    if (p.authored_projection_schema_version === 4) {
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
  }
  decodeAuthoredMetadata(p, rowIds, columnIds);
  return p as Projection;
}

export class DocumentStore {
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
  remotePresence: RemotePresence[] = [];
  editing: { position: Position; text: string; source: 'cell' | 'bar' } | undefined;
  error = '';
  loading = false;
  exporting = false;
  recalculating = false;
  calculated = new Map<string, CalculatedValue>();
  calculatedVersion: string | undefined;
  calculationStatus: 'idle' | 'pending' | 'complete' | 'failed' = 'idle';
  calculationError = '';
  private calculationTimer: ReturnType<typeof setTimeout> | undefined;
  private calculationRequestedVersion: string | undefined;
  private calculationAssembly:
    { nextSequence: number; bytes: number; values: Map<string, CalculatedValue> } | undefined;
  private revision = 0;
  private listeners = new Set<() => void>();
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
  private recalculationRequest: { document_id: string; operation_id: string } | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  snapshot = () => this.revision;
  notify = () => {
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
      const response = await fetch(`/api/documents/${encodeURIComponent(document)}/undo-history`, {
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
    this.selected = { row: 0, column: 0 };
    this.selectionAnchor = { row: 0, column: 0 };
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
    return result;
  }
  display(position: Position) {
    const value = this.displayContent(position);
    return value?.type === 'number'
      ? formatNumber(value.value, this.style(position).number_format)
      : displayValue(value);
  }
  displayContent(position: Position): Value | CalculatedValue | undefined {
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
      return value;
    if (
      this.calculatedVersion !== this.projection?.version ||
      this.calculationStatus !== 'complete'
    )
      return value;
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
      projection.authored_projection_schema_version === 4
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
        ...(projection.authored_projection_schema_version === 4 ? [] : overlay),
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
            projection.authored_projection_schema_version === 4
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
        for (const address of projection.authored_projection_schema_version === 4
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
          ...(projection.authored_projection_schema_version === 4
            ? { styles: [...styles.values()] }
            : {}),
        };
      });
      if (projection.authored_projection_schema_version === 4) {
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
          schema_version: 4,
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
    this.calculated.clear();
    this.calculatedVersion = undefined;
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
          },
        },
      }),
    );
    if (!this.sender(bytes)) {
      this.failCalculation();
      return;
    }
    this.calculationRequestedVersion = this.projection.version;
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
    try {
      const bytes = toBinary(CalculationCommittedSchema, message).byteLength;
      if (
        message.documentId.length !== 16 ||
        message.contextId.length !== 16 ||
        bytesUuid(message.contextId) !==
          (this.projection.calculation_context?.context_id ?? this.projection.document_id) ||
        message.calculationEpoch !== BigInt(this.projection.calculation_context?.epoch ?? '0') ||
        message.formulaEngineVersion !== 'pxel-formula-0.14.0' ||
        message.compatibilityProfile !== formulaProfile ||
        !message.fullSnapshot ||
        bytes > 256 * 1024 ||
        message.patchSequence > 4096
      )
        throw new Error('Invalid calculation metadata');
      if (message.errorCode !== 0) {
        if (
          message.errorCode < 1 ||
          message.errorCode > 5 ||
          !message.isFinal ||
          message.patchSequence !== 0 ||
          message.cells.length !== 0 ||
          message.calculatedStateHash.length !== 0
        )
          throw new Error('Invalid calculation failure');
        this.failCalculation();
        return;
      }
      if (message.calculatedStateHash.length !== (message.isFinal ? 32 : 0))
        throw new Error('Invalid calculation hash');
      const assembly = this.calculationAssembly ?? {
        nextSequence: 0,
        bytes: 0,
        values: new Map<string, CalculatedValue>(),
      };
      if (message.patchSequence !== assembly.nextSequence)
        throw new Error('Invalid calculation sequence');
      assembly.bytes += bytes;
      if (assembly.bytes > 32 * 1024 * 1024 || assembly.values.size + message.cells.length > 100000)
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
          !targetAxes.columns.has(bytesUuid(address.columnId)) ||
          !value?.case
        )
          throw new Error('Invalid calculation cell');
        const cellKey = key(bytesUuid(address.rowId), bytesUuid(address.columnId));
        if (assembly.values.has(cellKey) || this.confirmed.get(cellKey)?.type !== 'formula')
          throw new Error('Invalid calculation set');
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
          assembly.values.size !==
          [...this.confirmed.values()].filter((value) => value.type === 'formula').length
        )
          throw new Error('Incomplete calculation');
        clearTimeout(this.calculationTimer);
        this.calculated = assembly.values;
        this.calculatedVersion = this.projection.version;
        this.calculationStatus = 'complete';
        this.calculationError = '';
        this.calculationAssembly = undefined;
      }
    } catch {
      this.failCalculation();
    } finally {
      this.notify();
    }
  }
  load(projection: Projection) {
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
      if (this.editing) {
        this.editing = undefined;
        this.error = 'Структура таблицы изменилась. Повторите ввод в нужной ячейке.';
      }
      this.remotePresence = [];
    }
    if (documentChanged) {
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
    const id = this.projection?.document_id ?? new URL(location.href).searchParams.get('document');
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
        response = await fetch(`/api/documents/${encodeURIComponent(id)}`, {
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
    if (this.loading) return;
    this.loading = true;
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
      history.replaceState(null, '', url);
      this.creation = undefined;
      await this.refresh();
    } catch (error) {
      this.error = error instanceof Error ? error.message : 'Не удалось создать таблицу';
    } finally {
      this.loading = false;
      this.notify();
    }
  }
  async recalculate() {
    const id = this.projection?.document_id;
    if (!id || !this.sender || this.recalculating || this.pending.length) return;
    if (this.recalculationRequest?.document_id !== id)
      this.recalculationRequest = { document_id: id, operation_id: uuidV7() };
    const request = this.recalculationRequest;
    this.recalculating = true;
    this.error = '';
    this.notify();
    try {
      const response = await fetch(`/api/documents/${encodeURIComponent(id)}/recalculate`, {
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
  async exportXlsx() {
    const id = this.projection?.document_id;
    if (!id || this.exporting || this.pending.length) return;
    this.exporting = true;
    this.error = '';
    this.notify();
    try {
      const response = await fetch(`/api/documents/${encodeURIComponent(id)}/export.xlsx`, {
        signal: AbortSignal.timeout(30_000),
      });
      if (!response.ok) {
        throw new Error(
          response.status === 403
            ? 'Экспорт доступен только владельцу таблицы.'
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
    this.disconnected();
    this.preview.dispose();
    this.previewValues = new Map();
    this.previewStamp = undefined;
    this.previewStatus = 'idle';
    this.movePreview = undefined;
  }
  select(position: Position, extend = false) {
    if (!extend) this.selectionAnchor = { ...position };
    this.selected = position;
    this.schedulePresence();
    this.notify();
  }
  get selectionBounds() {
    return {
      startRow: Math.min(this.selectionAnchor.row, this.selected.row),
      endRow: Math.max(this.selectionAnchor.row, this.selected.row),
      startColumn: Math.min(this.selectionAnchor.column, this.selected.column),
      endColumn: Math.max(this.selectionAnchor.column, this.selected.column),
    };
  }
  get pasteOrigin(): Position {
    const { startRow, startColumn } = this.selectionBounds;
    return { row: startRow, column: startColumn };
  }
  copySelectionTsv() {
    const sheet = this.sheet;
    if (!sheet) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    const count = (endRow - startRow + 1) * (endColumn - startColumn + 1);
    if (count > 1_000) {
      this.error = 'Можно скопировать не более 1000 ячеек за один раз.';
      this.notify();
      return undefined;
    }
    const lines: string[] = [];
    let bytes = 0;
    for (let row = startRow; row <= endRow; row++) {
      const fields: string[] = [];
      for (let column = startColumn; column <= endColumn; column++) {
        const value = this.value({ row, column });
        const text = value?.type === 'formula' ? value.value.current_text : displayValue(value);
        if (text === undefined) {
          this.error = 'Формула ещё не готова для копирования.';
          this.notify();
          return undefined;
        }
        if (/\r|\n|\t/.test(text) || (value?.type === 'text' && text.startsWith('='))) {
          this.error = 'Эту ячейку нельзя без потерь скопировать как TSV.';
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
      lines.push(fields.join('\t'));
    }
    this.error = '';
    this.notify();
    return lines.join('\n');
  }
  begin(text = displayValue(this.value(this.selected)), source: 'cell' | 'bar' = 'cell') {
    if (!this.sheet) return;
    this.editing = { position: { ...this.selected }, text, source };
    this.syncPreview();
    this.notify();
  }
  draft(text: string) {
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
    const edit = this.editing;
    if (!edit) return;
    this.editing = undefined;
    if (!this.set(edit.position, parseInput(edit.text))) this.editing = edit;
    this.notify();
  }
  set(position: Position, value: Value) {
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
    this.syncPreview();
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  get canFormat() {
    return Boolean(
      this.sheet &&
      this.actor &&
      this.sender &&
      !this.syncing &&
      !this.editing &&
      this.pendingCount < 128,
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
  get canUndo() {
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
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  copyFormulaPayload() {
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
  cutRangePayload() {
    if (!this.canPrepareMove()) return undefined;
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
    const rows = endRow - startRow + 1,
      columns = endColumn - startColumn + 1;
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
    this.cutIntent = { raw, version: projection.version, stale: false };
    this.error = '';
    this.notify();
    return raw;
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
    const sheet = this.sheet,
      projection = this.projection;
    if (!sheet || !projection || this.pending.length || this.editing) return undefined;
    const { startRow, endRow, startColumn, endColumn } = this.selectionBounds;
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
  pasteRangeCopy(raw: string, position: Position = this.pasteOrigin) {
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
    if (
      sourceRow < 0 ||
      sourceColumn < 0 ||
      sourceRow + rows > sheet.rows.length ||
      sourceColumn + columns > sheet.columns.length ||
      position.row + rows > sheet.rows.length ||
      position.column + columns > sheet.columns.length
    ) {
      this.error = 'Диапазон выходит за границы листа.';
      this.notify();
      return false;
    }
    const destinationRow = sheet.rows[position.row],
      destinationColumn = sheet.columns[position.column];
    if (!destinationRow || !destinationColumn) return false;
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
            copyRange: {
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
            },
          },
        },
      }),
    );
    if (bytes.byteLength > 1_048_576) return false;
    this.pending.push({ kind: 'copyRange', id, bytes, attempts: 0 });
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  async pasteFormulaCopy(raw: string, position: Position = this.selected) {
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
  pasteTsv(text: string, position: Position = this.selected, preserveTrailingBlank = false) {
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (!sheet || !projection || !actor) return false;
    const normalizedText = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    const rows = (preserveTrailingBlank ? normalizedText : normalizedText.replace(/\n$/, ''))
      .split('\n')
      .map((line) => line.split('\t'));
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
    let textBytes = 0;
    for (let r = 0; r < rows.length; r++) {
      const fields = rows[r] ?? [];
      for (let c = 0; c < fields.length; c++) {
        const value = parseInput(fields[c] ?? '');
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
          operation: create(CanonicalOperationSchema, {
            kind: {
              case: 'setCellValues',
              value: { writes: wireWrites },
            },
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
    this.pending.push({ kind: 'batch', id, writes, bytes, attempts: 0 });
    this.syncPreview();
    this.error = '';
    this.notify();
    this.flush();
    return true;
  }
  addSheet() {
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
    const sheet = this.sheet,
      projection = this.projection,
      actor = this.actor;
    if (
      !sheet ||
      !projection ||
      !actor ||
      sheet.rows.length >= 100_000 ||
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
    if (
      operation.kind.case === 'extendRows' ||
      operation.kind.case === 'insertRows' ||
      operation.kind.case === 'deleteRows' ||
      operation.kind.case === 'insertColumns' ||
      operation.kind.case === 'deleteColumns' ||
      operation.kind.case === 'addSheet' ||
      operation.kind.case === 'setCellValues'
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
    const payload = envelope.payload;
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
          event.canonicalOperation?.kind.case !== 'addSheet' &&
          event.canonicalOperation?.kind.case !== 'setCellValues'
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
      const rejection = payload.value;
      const sourceChanged =
        rejection.code === 17 && (pending?.kind === 'copyRange' || pending?.kind === 'moveRange');
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
    } else {
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
        payload.value.canonicalOperation.kind.case !== 'addSheet' &&
        payload.value.canonicalOperation.kind.case !== 'setCellValues'
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
