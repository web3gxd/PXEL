import type { CalculatedValue } from '../document/store';
import {
  addressKey,
  toPreviewValue,
  type PreviewCell,
  type PreviewInit,
  type PreviewMoveGeometry,
  type PreviewValue,
  type PreviewResult,
  type Reply,
  type Request,
  type Stamp,
} from './protocol';

export type WorkerPort = Pick<Worker, 'postMessage' | 'terminate' | 'onmessage' | 'onerror'>;
export type PreviewStatus = 'idle' | 'pending' | 'ready' | 'invalid' | 'failed' | 'unavailable';
export class PreviewClient {
  private worker: WorkerPort | undefined;
  private desired: Stamp | undefined;
  private acknowledged: Stamp | undefined;
  private active: Request | undefined;
  private id = 0;
  private queued = new Map<string, PreviewCell>();
  private queuedMove: { stamp: Stamp; geometry: PreviewMoveGeometry } | undefined;
  private hasMove = false;
  private values = new Map<string, CalculatedValue>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  constructor(
    private readonly update: (
      status: PreviewStatus,
      values?: ReadonlyMap<string, CalculatedValue>,
      stamp?: Stamp,
      moveWrites?: ReadonlyMap<string, PreviewValue>,
    ) => void,
    private readonly factory: () => WorkerPort | undefined = () =>
      typeof Worker === 'undefined'
        ? undefined
        : new Worker(new URL('./workbook.worker.ts', import.meta.url), { type: 'module' }),
  ) {}
  reset(data: PreviewInit) {
    this.dispose();
    this.desired = data;
    try {
      this.worker = this.factory();
    } catch {
      this.fail();
      return;
    }
    if (!this.worker) {
      this.update('unavailable');
      return;
    }
    const worker = this.worker;
    worker.onmessage = (event: MessageEvent<Reply>) => {
      if (this.worker !== worker) return;
      this.receive(event.data);
    };
    worker.onerror = () => {
      if (this.worker === worker) this.fail();
    };
    this.send({
      id: ++this.id,
      kind: 'init',
      data: {
        ...data,
        sheets: data.sheets.map((sheet) => ({
          id: sheet.id,
          name: sheet.name,
          rows: sheet.rows,
          columns: sheet.columns,
          cells: sheet.cells.map((cell) => ({ ...cell, value: toPreviewValue(cell.value) })),
        })),
      },
    });
  }
  patch(stamp: Stamp, cells: PreviewCell[]) {
    if (!this.worker || !this.desired) return;
    // A private moved projection cannot receive ordinary patches based on confirmed content.
    if (this.hasMove) {
      this.fail();
      return;
    }
    if (
      stamp.document_id !== this.desired.document_id ||
      BigInt(stamp.input_version) < BigInt(this.desired.input_version) ||
      BigInt(stamp.pending_revision) <= BigInt(this.desired.pending_revision)
    ) {
      this.fail();
      return;
    }
    this.desired = stamp;
    for (const cell of cells)
      this.queued.set(addressKey(cell), { ...cell, value: toPreviewValue(cell.value) });
    if (
      this.queued.size > 256 ||
      new TextEncoder().encode(JSON.stringify([...this.queued.values()])).length > 1_048_576
    ) {
      this.fail();
      return;
    }
    this.update('pending');
    this.flush();
  }
  move(stamp: Stamp, geometry: PreviewMoveGeometry): boolean {
    if (!this.worker || !this.desired || this.hasMove || this.queued.size > 0) return false;
    if (
      stamp.document_id !== this.desired.document_id ||
      stamp.input_version !== this.desired.input_version ||
      !/^\d+$/.test(stamp.pending_revision) ||
      BigInt(stamp.pending_revision) <= BigInt(this.desired.pending_revision) ||
      !validAddress(geometry.source) ||
      !validAddress(geometry.destination) ||
      geometry.source.sheet_id !== geometry.destination.sheet_id ||
      !Number.isInteger(geometry.rows) ||
      !Number.isInteger(geometry.columns) ||
      geometry.rows < 1 ||
      geometry.columns < 1 ||
      geometry.rows * geometry.columns > 1000
    )
      return false;
    this.desired = stamp;
    this.hasMove = true;
    this.queuedMove = {
      stamp: {
        document_id: stamp.document_id,
        input_version: stamp.input_version,
        pending_revision: stamp.pending_revision,
      },
      geometry: {
        source: { ...geometry.source },
        destination: { ...geometry.destination },
        rows: geometry.rows,
        columns: geometry.columns,
      },
    };
    this.update('pending');
    this.flush();
    return true;
  }
  private flush() {
    if (
      this.active ||
      !this.acknowledged ||
      !this.desired ||
      this.acknowledged.pending_revision === this.desired.pending_revision
    )
      return;
    if (this.queuedMove) {
      const move = this.queuedMove;
      if (this.acknowledged.input_version !== move.stamp.input_version) {
        this.fail();
        return;
      }
      this.queuedMove = undefined;
      this.send({
        id: ++this.id,
        kind: 'move',
        data: {
          ...move.stamp,
          ...move.geometry,
          schema_version: 1,
          expected_pending_revision: this.acknowledged.pending_revision,
        },
      });
      return;
    }
    const data = {
      document_id: this.desired.document_id,
      input_version: this.desired.input_version,
      pending_revision: this.desired.pending_revision,
      schema_version: 1 as const,
      expected_input_version: this.acknowledged.input_version,
      expected_pending_revision: this.acknowledged.pending_revision,
      cells: [...this.queued.values()],
    };
    this.queued.clear();
    this.send({ id: ++this.id, kind: 'apply', data });
  }
  private send(request: Request) {
    this.active = request;
    this.update('pending');
    this.timer = setTimeout(() => this.fail(), 5000);
    try {
      this.worker?.postMessage(request);
    } catch {
      this.fail();
    }
  }
  private receive(reply: Reply) {
    if (!this.active || reply.id !== this.active.id) return;
    clearTimeout(this.timer);
    if ('error' in reply) {
      if (
        reply.error === 'InvalidFormula' &&
        reply.phase === 'apply' &&
        this.active.kind === 'apply'
      ) {
        const failedRevision = this.active.data.pending_revision;
        const cells = new Map(this.active.data.cells.map((cell) => [addressKey(cell), cell]));
        for (const [address, cell] of this.queued) cells.set(address, cell);
        this.queued = cells;
        this.active = undefined;
        this.update('invalid');
        if (this.desired?.pending_revision !== failedRevision) this.flush();
        return;
      }
      this.fail();
      return;
    }
    const result = reply.result;
    const isMove = this.active.kind === 'move';
    const sent = this.active.data;
    if (
      !validResult(result) ||
      (isMove ? !validMoveWrites(result.move_writes) : result.move_writes !== undefined) ||
      result.document_id !== sent.document_id ||
      result.input_version !== sent.input_version ||
      result.pending_revision !== sent.pending_revision
    ) {
      this.fail();
      return;
    }
    if (result.full) this.values.clear();
    for (const cell of result.cleared) this.values.delete(addressKey(cell));
    for (const cell of result.results) this.values.set(addressKey(cell), cell.value);
    this.acknowledged = result;
    this.active = undefined;
    if (
      result.input_version === this.desired?.input_version &&
      result.pending_revision === this.desired.pending_revision
    )
      this.update(
        'ready',
        this.values,
        result,
        isMove
          ? new Map(result.move_writes?.map((cell) => [addressKey(cell), cell.value]))
          : undefined,
      );
    this.flush();
  }
  private fail() {
    this.dispose();
    this.update('failed');
  }
  dispose() {
    clearTimeout(this.timer);
    this.worker?.terminate();
    this.worker = undefined;
    this.active = undefined;
    this.acknowledged = undefined;
    this.desired = undefined;
    this.queued.clear();
    this.queuedMove = undefined;
    this.hasMove = false;
    this.values.clear();
  }
}
function validAddress(cell: Omit<PreviewCell, 'value'>) {
  return (
    cell &&
    [cell.sheet_id, cell.row_id, cell.column_id].every(
      (id) => typeof id === 'string' && /^[0-9a-f-]{36}$/.test(id),
    )
  );
}
function validMoveWrites(writes: PreviewCell[] | undefined) {
  if (
    !Array.isArray(writes) ||
    writes.length === 0 ||
    writes.length > 10000 ||
    new TextEncoder().encode(JSON.stringify(writes)).length > 4_194_304
  )
    return false;
  const seen = new Set<string>();
  return writes.every((cell) => {
    if (!validAddress(cell) || seen.has(addressKey(cell)) || !cell.value) return false;
    seen.add(addressKey(cell));
    const value = cell.value;
    return (
      value.type === 'blank' ||
      (value.type === 'number' && Number.isFinite(value.value)) ||
      (value.type === 'boolean' && typeof value.value === 'boolean') ||
      (value.type === 'text' && typeof value.value === 'string' && value.value.length <= 32768) ||
      (value.type === 'formula' &&
        value.value &&
        typeof value.value.source === 'string' &&
        value.value.source.startsWith('=') &&
        value.value.source.length <= 32768)
    );
  });
}
function validResult(result: PreviewResult) {
  return (
    result &&
    result.schema_version === 1 &&
    result.preview === true &&
    result.complete === true &&
    typeof result.full === 'boolean' &&
    result.formula_engine_version === 'pxel-formula-0.14.0' &&
    result.compatibility_profile === 'pxel-excel-subset-v1' &&
    /^[0-9a-f]{64}$/.test(result.normalized_result_hash) &&
    Array.isArray(result.results) &&
    result.results.length <= 100000 &&
    Array.isArray(result.cleared) &&
    result.cleared.length <= 100000 &&
    result.cleared.every(validAddress) &&
    result.results.every((cell) => {
      if (!validAddress(cell) || !cell.value) return false;
      const value = cell.value;
      return (
        value.type === 'blank' ||
        (value.type === 'number' && Number.isFinite(value.value)) ||
        (value.type === 'text' && typeof value.value === 'string' && value.value.length <= 32768) ||
        (value.type === 'boolean' && typeof value.value === 'boolean') ||
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
      );
    })
  );
}
