import type { CalculatedValue } from '../document/store';
import {
  addressKey,
  type PreviewCell,
  type PreviewInit,
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
  private values = new Map<string, CalculatedValue>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  constructor(
    private readonly update: (
      status: PreviewStatus,
      values?: ReadonlyMap<string, CalculatedValue>,
      stamp?: Stamp,
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
    this.send({ id: ++this.id, kind: 'init', data });
  }
  patch(stamp: Stamp, cells: PreviewCell[]) {
    if (!this.worker || !this.desired) return;
    if (
      stamp.document_id !== this.desired.document_id ||
      BigInt(stamp.input_version) < BigInt(this.desired.input_version) ||
      BigInt(stamp.pending_revision) <= BigInt(this.desired.pending_revision)
    ) {
      this.fail();
      return;
    }
    this.desired = stamp;
    for (const cell of cells) this.queued.set(addressKey(cell), cell);
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
  private flush() {
    if (
      this.active ||
      !this.acknowledged ||
      !this.desired ||
      this.acknowledged.pending_revision === this.desired.pending_revision
    )
      return;
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
    const sent = this.active.data;
    if (
      !validResult(result) ||
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
      this.update('ready', this.values, result);
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
    this.values.clear();
  }
}
function validResult(result: PreviewResult) {
  const validAddress = (cell: Omit<PreviewCell, 'value'>) =>
    cell &&
    [cell.sheet_id, cell.row_id, cell.column_id].every(
      (id) => typeof id === 'string' && /^[0-9a-f-]{36}$/.test(id),
    );
  return (
    result &&
    result.schema_version === 1 &&
    result.preview === true &&
    result.complete === true &&
    typeof result.full === 'boolean' &&
    result.formula_engine_version === 'pxel-formula-0.3.0' &&
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
