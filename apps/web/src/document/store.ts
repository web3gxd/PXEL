import { create, toBinary } from '@bufbuild/protobuf';
import { PreviewClient, type PreviewStatus } from '../calculation/client';
import { copyFormulaInWorker } from '../calculation/formula-copy';
import type { Stamp, PreviewCell } from '../calculation/protocol';
import {
  ClientEnvelopeSchema,
  CanonicalOperationSchema,
  CellValueSchema,
  SetCellValueSchema,
  CalculationCommittedSchema,
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
};
export type Projection = {
  document_id: string;
  version: string;
  state_hash: string;
  calculation_context?: { context_id: string; epoch: string } | null;
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
  | PendingRangeCopy;
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
  13: 'Отмена невозможна: ячейка была изменена после вашей правки.',
  14: 'Эту операцию пока нельзя отменить.',
  15: 'Срок отмены этой операции истек. Последние 10 000 изменений доступны для отмены.',
  16: 'Изменение слишком большое для надежной записи в журнал.',
  17: 'Источник диапазона изменился. Скопируйте его ещё раз.',
};
const isUuid = (value: unknown): value is string =>
  typeof value === 'string' &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
const formulaProfile = 'pxel-excel-subset-v1';
const byteLength = (text: string) => new TextEncoder().encode(text).length;
function decodeFormula(input: unknown): FormulaDescriptor {
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
    !/^\d+$/.test(value.bound_at_version)
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
  if (
    p.calculation_context != null &&
    (!isUuid(p.calculation_context.context_id) ||
      typeof p.calculation_context.epoch !== 'string' ||
      !/^[1-9]\d*$/.test(p.calculation_context.epoch))
  )
    throw invalid();
  const sheetIds = new Set<string>();
  const sheetNames = new Set<string>();
  const rowIds = new Set<string>();
  const columnIds = new Set<string>();
  let totalCells = 0;
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
    if (totalCells > 100000) throw invalid();
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
        v.value = decodeFormula(v.value);
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
  }
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
  private preview = new PreviewClient((status, values, stamp) => {
    this.previewStatus = status;
    this.previewValues = values ?? new Map();
    this.previewStamp = stamp;
    this.notify();
  });
  projection: Projection | undefined;
  activeSheetId: string | undefined;
  confirmed = new Map<string, Value>();
  pending: Pending[] = [];
  private lastUndoableOperation: string | undefined;
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
    return this.confirmed.get(key(row, column));
  }
  display(position: Position) {
    const row = this.sheet?.rows[position.row],
      column = this.sheet?.columns[position.column];
    const value = row && column ? this.previewValue(row, column) : this.value(position);
    if (value?.type !== 'formula') return displayValue(value);
    if (
      row &&
      column &&
      this.previewStatus === 'ready' &&
      this.previewStamp?.input_version === this.projection?.version &&
      this.previewStamp?.pending_revision === String(this.previewRevision) &&
      (this.editing || this.pending.length || this.calculationStatus !== 'complete')
    ) {
      const preview = this.previewValues.get(`${this.sheet?.id}/${row}/${column}`);
      if (preview) return displayValue(preview);
    }
    if (
      !row ||
      !column ||
      this.pending.some(
        (pending) =>
          (pending.kind === 'cell' && pending.row === row && pending.column === column) ||
          (pending.kind === 'batch' &&
            pending.writes.some((item) => item.row === row && item.column === column)),
      )
    )
      return displayValue(value);
    if (
      this.calculatedVersion !== this.projection?.version ||
      this.calculationStatus !== 'complete'
    )
      return displayValue(value);
    return displayValue(this.calculated.get(key(row, column)));
  }
  get previewVisible() {
    return (
      this.previewStatus === 'ready' &&
      this.previewStamp?.input_version === this.projection?.version &&
      this.previewStamp?.pending_revision === String(this.previewRevision) &&
      this.previewValues.size > 0 &&
      Boolean(this.editing || this.pending.length || this.calculationStatus !== 'complete')
    );
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
    return this.confirmed.get(key(row, column)) ?? { type: 'blank' };
  }
  private syncPreview(reset = false) {
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
    this.previewOverlay = overlay;
    this.previewDirty.clear();
    if (reset) this.previewRevision = -1n;
    const stamp = {
      document_id: projection.document_id,
      input_version: projection.version,
      pending_revision: String(++this.previewRevision),
    };
    if (reset) {
      const keys = new Set([...this.confirmed.keys(), ...overlay]);
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
          value: this.previewValue(row_id, column_id),
        });
      }
      this.preview.reset({
        ...stamp,
        schema_version: projection.sheets.length === 1 ? 1 : 2,
        sheets: projection.sheets.map((item) => ({
          ...item,
          cells: cellsBySheet.get(item.id) ?? [],
        })),
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
      this.preview.patch(stamp, cells);
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
        message.formulaEngineVersion !== 'pxel-formula-0.3.0' ||
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
      this.lastUndoableOperation = undefined;
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
    this.activeSheetId = nextSheet?.id;
    this.selected = nextSelected;
    this.selectionAnchor = nextAnchor;
    this.confirmed = new Map(
      projection.sheets.flatMap((sheet) =>
        sheet.cells.map((c) => [key(c.row_id, c.column_id), c.value] as const),
      ),
    );
    this.restartCalculation();
    this.syncPreview(true);
    this.subscribeToHead();
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
      if (!(await this.refresh(BigInt(receipt.committed_version)))) return;
      this.recalculationRequest = undefined;
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
    this.subscribeToHead();
    if (this.previewStatus === 'idle' && this.projection) this.syncPreview(true);
    this.restartCalculation();
    this.flush();
    return true;
  }
  disconnected() {
    const hadPresence = this.remotePresence.length > 0;
    this.calculationRequestedVersion = undefined;
    this.calculationAssembly = undefined;
    clearTimeout(this.calculationTimer);
    this.connectionGeneration++;
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
    if (this.pending.length >= 128) {
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
  get canUndo() {
    return Boolean(
      this.lastUndoableOperation && this.sender && this.pending.length === 0 && !this.editing,
    );
  }
  undo() {
    const projection = this.projection,
      actor = this.actor,
      target = this.lastUndoableOperation;
    if (!projection || !actor || !target || !this.canUndo) return false;
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
    this.pending.push({ kind: 'undo', id, target, bytes, attempts: 0 });
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
    if (!projection || !sheet || !actor || byteLength(raw) > 16 * 1024 || this.pending.length)
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
      this.pending.length >= 128 ||
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
    if (!projection || !actor || projection.sheets.length >= 64 || this.pending.length >= 128)
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
      this.pending.length >= 128
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
      this.pending.length >= 128
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
      this.pending.length >= 128
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
      this.pending.length >= 128
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
      this.pending.length >= 128
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
        hint.headVersion > 9223372036854775807n
      )
        throw new Error('Invalid document head hint');
      if (bytesUuid(hint.documentId) !== this.projection?.document_id) return;
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
      if (pending?.kind === 'copyRange' && rejection.code === 17) {
        this.pending = this.pending.filter((p) => p.id !== id);
        this.error = 'Источник диапазона изменился. Скопируйте его ещё раз.';
        await this.refresh();
        this.notify();
        return;
      }
      if (rejection.retryable) {
        this.error = 'Сервер занят. Повторяем изменение…';
        this.timer = setTimeout(() => this.flush(), 1500);
        this.notify();
        return;
      }
      this.pending = this.pending.filter((p) => p.id !== id);
      if (pending?.kind === 'undo' && !rejection.retryable) this.lastUndoableOperation = undefined;
      this.error = `Изменение отклонено. ${rejectionText[rejection.code] ?? 'Неизвестная причина; обновите страницу.'}`;
      if (rejection.resyncRequired) await this.refresh();
    } else {
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
      this.pending = this.pending.filter((p) => p.id !== id);
      if (
        pending?.kind === 'addSheet' &&
        this.projection?.sheets.some((sheet) => sheet.id === pending.sheetId)
      )
        this.selectSheet(pending.sheetId);
      if (pending?.kind === 'cell') {
        this.lastUndoableOperation = pending.value.type === 'formula' ? undefined : pending.id;
      } else if (pending) {
        this.lastUndoableOperation = undefined;
      }
      this.error = '';
    }
    this.syncPreview();
    this.notify();
    this.flush();
  }
}
