import { afterEach, describe, expect, it, vi } from 'vitest';
import { create, fromBinary } from '@bufbuild/protobuf';
import type { Reply, Request } from '../calculation/protocol';
import { ClientEnvelopeSchema, ServerEnvelopeSchema } from '../generated/pxel/wire/v1/session_pb';
import type { FormulaError } from '../generated/pxel/wire/v1/session_pb';
import {
  DocumentStore,
  decodeProjection,
  displayValue,
  parseInput,
  uuidBytes,
  uuidV7,
  type Projection,
} from './store';

const documentId = '01994100-0000-7000-8000-000000000001';
const rowId = '01994100-0000-7000-8000-000000000002';
const columnId = '01994100-0000-7000-8000-000000000003';
const sheetId = '01994100-0000-7000-8000-000000000004';
const projection = (): Projection => ({
  document_id: documentId,
  version: '1',
  state_hash: '0'.repeat(64),
  sheets: [{ id: sheetId, name: 'Лист 1', rows: [rowId], columns: [columnId], cells: [] }],
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
it('keeps draft WASM preview separate from authored and canonical state, then removes it on cancel', () => {
  vi.useFakeTimers();
  const requests: Request[] = [];
  let deliver: ((reply: Reply) => void) | undefined;
  class PreviewWorker {
    onmessage: ((event: MessageEvent<Reply>) => void) | null = null;
    onerror = null;
    constructor() {
      deliver = (reply) => this.onmessage?.({ data: reply } as MessageEvent<Reply>);
    }
    postMessage(request: Request) {
      requests.push(request);
    }
    terminate() {}
  }
  vi.stubGlobal('Worker', PreviewWorker);
  const store = new DocumentStore();
  store.load(projection());
  const finish = (request: Request, hasFormula: boolean) =>
    deliver?.({
      id: request.id,
      result: {
        schema_version: 1,
        preview: true,
        complete: true,
        full: request.kind === 'init',
        document_id: documentId,
        input_version: request.data.input_version,
        pending_revision: request.data.pending_revision,
        formula_engine_version: 'pxel-formula-0.1.0',
        compatibility_profile: 'pxel-excel-subset-v1',
        normalized_result_hash: 'a'.repeat(64),
        evaluated_cells: 1,
        dirty_cells: 1,
        results: hasFormula
          ? [
              {
                sheet_id: sheetId,
                row_id: rowId,
                column_id: columnId,
                value: { type: 'number', value: 5 },
              },
            ]
          : [],
        cleared: hasFormula ? [] : [{ sheet_id: sheetId, row_id: rowId, column_id: columnId }],
      },
    });
  finish(requests[0]!, false);
  store.begin('=2+3', 'bar');
  finish(requests[1]!, true);
  expect(store.display({ row: 0, column: 0 })).toBe('5');
  expect(store.previewVisible).toBe(true);
  expect(store.editing?.text).toBe('=2+3');
  expect(store.value({ row: 0, column: 0 })).toBeUndefined();
  expect(store.confirmed.size).toBe(0);
  expect(store.calculated.size).toBe(0);
  expect(store.pending).toHaveLength(0);
  expect(store.projection?.state_hash).toBe('0'.repeat(64));
  store.cancel();
  expect(store.previewVisible).toBe(false);
  finish(requests[2]!, false);
  expect(store.display({ row: 0, column: 0 })).toBe('');
  expect(requests.filter((request) => request.kind === 'init')).toHaveLength(1);
});
async function connectedStore() {
  vi.useFakeTimers();
  vi.stubGlobal('location', { href: 'http://localhost/' });
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({ ok: true, json: async () => projection() })),
  );
  const store = new DocumentStore();
  store.load(projection());
  const sent: Uint8Array[] = [];
  await store.connected(uuidBytes(documentId), (bytes) => {
    sent.push(bytes);
    return true;
  });
  sent.length = 0;
  return { store, sent };
}
describe('document projection and pending intents', () => {
  it('refreshes canonical content when a remote head hint arrives', async () => {
    const { store } = await connectedStore();
    const next = projection();
    next.version = '2';
    next.sheets[0]!.cells = [
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 42 } },
    ];
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => next })),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: { documentId: uuidBytes(documentId), headVersion: 2n },
        },
      }),
    );
    expect(store.projection?.version).toBe('2');
    expect(store.display({ row: 0, column: 0 })).toBe('42');
    store.disconnected();
  });
  it('retries a temporarily failed remote head refresh without resending a user command', async () => {
    const { store, sent } = await connectedStore();
    const next = projection();
    next.version = '2';
    let calls = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        calls++;
        if (calls === 1) throw new Error('Temporary network loss');
        return { ok: true, json: async () => next };
      }),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: { documentId: uuidBytes(documentId), headVersion: 2n },
        },
      }),
    );
    expect(store.projection?.version).toBe('1');
    expect(sent).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1000);
    expect(store.projection?.version).toBe('2');
    expect(calls).toBe(2);
    store.disconnected();
  });
  it('retains pending receipt until recovery reaches its committed version', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 42 });
    const command = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing command');
    const receipt = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'receipt',
        value: {
          documentId: uuidBytes(documentId),
          operationId: command.value.operationId,
          committedVersion: 2n,
          currentHeadVersion: 2n,
        },
      },
    });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new Error('Network unavailable');
      }),
    );
    await store.receive(receipt);
    expect(store.pending).toHaveLength(1);
    expect(store.projection?.version).toBe('1');
    expect(store.error).not.toBe('');
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => projection() })),
    );
    await store.receive(receipt);
    expect(store.pending).toHaveLength(1);
    expect(store.error).not.toBe('');
    const recovered = projection();
    recovered.version = '2';
    recovered.sheets[0]?.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'number', value: 42 },
    });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => recovered })),
    );
    await store.receive(receipt);
    expect(store.pending).toHaveLength(0);
    expect(store.projection?.version).toBe('2');
    expect(store.confirmed.size).toBe(1);
    store.disconnected();
  });
  it('gates reconnect sends until snapshot loaded and ignores older HTTP responses', async () => {
    const { store, sent } = await connectedStore();
    store.disconnected();
    let finish: ((response: unknown) => void) | undefined;
    vi.stubGlobal(
      'fetch',
      vi.fn(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      ),
    );
    const reconnect = store.connected(uuidBytes(documentId), (bytes) => {
      sent.push(bytes);
      return true;
    });
    store.set({ row: 0, column: 0 }, { type: 'number', value: 9 });
    expect(sent).toHaveLength(0);
    finish?.({ ok: true, json: async () => projection() });
    await reconnect;
    expect(sent).toHaveLength(2);
    const subscription = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    expect(subscription.case).toBe('subscribeDocument');
    const refresh = store.refresh();
    const command = fromBinary(ClientEnvelopeSchema, sent[1] ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing command');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            committedVersion: 2n,
            canonicalOperation: command.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    finish?.({ ok: true, json: async () => projection() });
    await refresh;
    expect(store.projection?.version).toBe('2');
    expect(displayValue(store.value({ row: 0, column: 0 }))).toBe('9');
    store.disconnected();
  });
  it('ignores a superseded snapshot request completing after the latest one', async () => {
    const { store } = await connectedStore();
    const responses: ((value: unknown) => void)[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(() => new Promise((resolve) => responses.push(resolve))),
    );
    const old = store.refresh(),
      latest = store.refresh();
    const newer = projection();
    newer.version = '3';
    responses[1]?.({ ok: true, json: async () => newer });
    await latest;
    responses[0]?.({ ok: true, json: async () => projection() });
    await old;
    expect(store.projection?.version).toBe('3');
    store.disconnected();
  });
  it('retains an oversized draft without queuing an invalid command', async () => {
    const { store, sent } = await connectedStore();
    store.begin('я'.repeat(17000));
    store.commit();
    expect(store.editing?.text).toHaveLength(17000);
    expect(store.pending).toHaveLength(0);
    expect(sent).toHaveLength(0);
    expect(store.error).toContain('32 КБ');
    store.disconnected();
  });
  it('does not mutate confirmed cells before canonical commit', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 42 });
    expect(store.confirmed.size).toBe(0);
    expect(displayValue(store.value({ row: 0, column: 0 }))).toBe('42');
    expect(store.projection?.version).toBe('1');
    const command = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing command');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            committedVersion: 2n,
            canonicalOperation: command.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.pending).toHaveLength(0);
    expect(store.confirmed.size).toBe(1);
    expect(store.projection?.version).toBe('2');
    store.disconnected();
  });
  it('retries identical bytes and keeps a newer pending edit on the same cell', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'text', value: 'first' });
    store.set({ row: 0, column: 0 }, { type: 'text', value: 'second' });
    expect(sent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(2000);
    expect(sent).toHaveLength(2);
    expect(sent[1]).toEqual(sent[0]);
    expect(displayValue(store.value({ row: 0, column: 0 }))).toBe('second');
    expect(store.confirmed.size).toBe(0);
    store.disconnected();
  });
  it('reject removes optimistic content, no false saved value', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 7 });
    const command = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing command');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 1,
          },
        },
      }),
    );
    expect(store.pending).toHaveLength(0);
    expect(store.confirmed.size).toBe(0);
    expect(store.error).toContain('отклонено');
    store.disconnected();
  });
  it('editor cancel and formula bar share a draft without writing', async () => {
    const { store, sent } = await connectedStore();
    store.begin('draft', 'bar');
    store.draft('changed');
    expect(store.editing?.source).toBe('bar');
    store.cancel();
    expect(sent).toHaveLength(0);
    expect(store.value({ row: 0, column: 0 })).toBeUndefined();
    store.begin('42');
    store.commit();
    expect(sent).toHaveLength(1);
    store.disconnected();
  });
  it('caps pending queue independently of wire retries', async () => {
    const { store } = await connectedStore();
    for (let i = 0; i < 129; i++) store.set({ row: 0, column: 0 }, { type: 'number', value: i });
    expect(store.pending).toHaveLength(128);
    expect(store.error).toContain('Слишком много');
    store.disconnected();
  });
});
it('primitive input distinguishes formula intent from literal values', () => {
  expect(parseInput('=A1+1')).toEqual({ type: 'formula', value: { source: '=A1+1' } });
  expect(parseInput('1e999')).toEqual({ type: 'text', value: '1e999' });
  expect(parseInput('-0')).toEqual({ type: 'number', value: 0 });
  expect(parseInput('false')).toEqual({ type: 'boolean', value: false });
  expect(parseInput('')).toEqual({ type: 'blank' });
});
it('snapshot boundary rejects malformed axes and nonfinite values', () => {
  expect(decodeProjection(projection()).version).toBe('1');
  const invalid = projection();
  invalid.sheets[0]?.rows.push(rowId);
  expect(() => decodeProjection(invalid)).toThrow();
  expect(() => decodeProjection({})).toThrow();
});
it('operation identities are valid UUIDv7', () => {
  expect(uuidV7()).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
});

const cellPosition = { row: 0, column: 0 };
function formulaProjection(version = '2'): Projection {
  const p = projection();
  p.version = version;
  p.sheets[0]?.cells.push({
    row_id: rowId,
    column_id: columnId,
    value: {
      type: 'formula',
      value: {
        source: '=1+2',
        canonical: '=(1+2)',
        ast_schema: 1,
        compatibility_profile: 'pxel-excel-subset-v1',
        bound_at_version: '1',
      },
    },
  });
  return p;
}
function calculation(
  version: string,
  value = 3,
  options: { sequence?: number; final?: boolean; cells?: boolean; error?: number } = {},
) {
  const final = options.final ?? true;
  return create(ServerEnvelopeSchema, {
    protocolVersion: 1,
    payload: {
      case: 'calculationCommitted',
      value: {
        documentId: uuidBytes(documentId),
        inputVersion: BigInt(version),
        calculationEpoch: 0n,
        formulaEngineVersion: 'pxel-formula-0.1.0',
        compatibilityProfile: 'pxel-excel-subset-v1',
        contextId: uuidBytes(documentId),
        patchSequence: options.sequence ?? 0,
        isFinal: final,
        calculatedStateHash:
          final && !options.error ? new Uint8Array(32).fill(1) : new Uint8Array(),
        cells:
          options.cells === false
            ? []
            : [
                {
                  address: {
                    sheetId: uuidBytes(sheetId),
                    rowId: uuidBytes(rowId),
                    columnId: uuidBytes(columnId),
                  },
                  value: { kind: { case: 'number', value } },
                },
              ],
        fullSnapshot: true,
        errorCode: options.error ?? 0,
      },
    },
  });
}
describe('formula intents and versioned server calculation', () => {
  it('sends formula source separately and edits source while displaying server result', async () => {
    const { store, sent } = await connectedStore();
    store.begin('=1+2');
    store.commit();
    const command = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing formula command');
    expect(command.value.operation).toBeUndefined();
    expect(command.value.formulaCommand?.source).toBe('=1+2');
    expect(store.confirmed.size).toBe(0);
    expect(store.display(cellPosition)).toBe('=1+2');

    const address = {
      sheetId: uuidBytes(sheetId),
      rowId: uuidBytes(rowId),
      columnId: uuidBytes(columnId),
    };
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            committedVersion: 2n,
            confirmedStateHash: new Uint8Array(32),
            canonicalOperation: {
              kind: {
                case: 'setCellFormula',
                value: {
                  address,
                  formula: {
                    source: '=1+2',
                    canonical: '=(1+2)',
                    boundAtVersion: 1n,
                    compatibilityProfile: 1,
                    origin: address,
                    ast: {
                      schemaVersion: 1,
                      root: 2,
                      nodes: [
                        { kind: { case: 'number', value: 1 } },
                        { kind: { case: 'number', value: 2 } },
                        { kind: { case: 'binary', value: { operator: 1, left: 0, right: 1 } } },
                      ],
                    },
                  },
                },
              },
            },
          },
        },
      }),
    );
    await store.receive(calculation('2'));
    expect(store.calculationStatus).toBe('complete');
    expect(store.display(cellPosition)).toBe('3');
    expect(displayValue(store.value(cellPosition))).toBe('=1+2');
    store.begin();
    expect(store.editing?.text).toBe('=1+2');
    expect(store.pending).toHaveLength(0);
    store.disconnected();
  });
  it('ignores superseded calculations and requests the newest version without HTTP polling', async () => {
    const { store, sent } = await connectedStore();
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    store.load(formulaProjection('2'));
    store.load(formulaProjection('3'));
    const requests = sent.map((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload);
    expect(
      requests.map((payload) =>
        payload.case === 'requestCalculation' ? payload.value.inputVersion : undefined,
      ),
    ).toEqual([2n, 3n]);
    await store.receive(calculation('2', 22));
    expect(store.calculatedVersion).toBeUndefined();
    expect(store.display(cellPosition)).toBe('=1+2');
    await store.receive(calculation('3', 33));
    expect(store.calculatedVersion).toBe('3');
    expect(store.display(cellPosition)).toBe('33');
    expect(fetcher).not.toHaveBeenCalled();
    store.disconnected();
  });
  it('does not promote values attached to a failed calculation', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2', 999, { error: 1 }));
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    expect(store.calculatedVersion).toBeUndefined();
    expect(store.display(cellPosition)).toBe('=1+2');
    expect(store.value(cellPosition)?.type).toBe('formula');
    expect(store.calculationError).not.toBe('');
    store.disconnected();
  });
  it('never treats a result for a different input version as current', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection('2'));
    await store.receive(calculation('1', 99));
    expect(store.calculated.size).toBe(0);
    expect(store.display(cellPosition)).toBe('=1+2');
    store.disconnected();
  });
  it('bounds waiting for calculation and clears staged chunks on disconnect', async () => {
    const { store, sent } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2', 3, { final: false }));
    expect(store.calculated.size).toBe(0);
    await vi.advanceTimersByTimeAsync(35000);
    expect(store.calculationStatus).toBe('failed');
    expect(sent).toHaveLength(1);
    store.disconnected();
    await store.receive(calculation('2', 3, { sequence: 1, cells: false }));
    expect(store.calculatedVersion).toBeUndefined();
    await vi.advanceTimersByTimeAsync(60000);
    expect(sent).toHaveLength(1);
  });
  it('publishes a complete chunk stream atomically and ignores late duplicates', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2', 3, { final: false }));
    expect(store.calculationStatus).toBe('pending');
    expect(store.calculated.size).toBe(0);
    await store.receive(calculation('2', 3, { sequence: 1, cells: false }));
    expect(store.calculationStatus).toBe('complete');
    expect(store.display(cellPosition)).toBe('3');
    await store.receive(calculation('2', 999));
    expect(store.display(cellPosition)).toBe('3');
    store.disconnected();
  });
  it('rejects out-of-order or duplicate chunks without publishing partial results', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2', 3, { sequence: 1 }));
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    store.load(formulaProjection());
    await store.receive(calculation('2', 3, { final: false }));
    await store.receive(calculation('2', 3, { final: false }));
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    store.disconnected();
  });
  it('does not overlay an old confirmed result on a new pending formula', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2'));
    expect(store.calculationStatus).toBe('complete');
    store.set(cellPosition, parseInput('=4+5'));
    expect(store.display(cellPosition)).toBe('=4+5');
    expect(store.calculatedVersion).toBe('2');
    expect(displayValue(store.confirmed.values().next().value)).toBe('=1+2');
    store.disconnected();
  });
  it.each([
    'oversized',
    'unknown-error',
    'wrong-context',
    'wrong-epoch',
    'wrong-profile',
    'missing-final-hash',
  ])('rejects %s calculation metadata or payload without partial publication', async (invalid) => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    const envelope = calculation('2');
    if (envelope.payload.case !== 'calculationCommitted') throw new Error('Missing calculation');
    const message = envelope.payload.value;
    if (invalid === 'wrong-context') message.contextId = uuidBytes(rowId);
    if (invalid === 'wrong-epoch') message.calculationEpoch = 1n;
    if (invalid === 'wrong-profile') message.compatibilityProfile = 'unsupported';
    if (invalid === 'missing-final-hash') message.calculatedStateHash = new Uint8Array();
    const value = message.cells[0]?.value;
    if (value && invalid === 'oversized')
      value.kind = { case: 'text', value: 'x'.repeat(256 * 1024) };
    if (value && invalid === 'unknown-error')
      value.kind = { case: 'error', value: 1000 as FormulaError };
    await store.receive(envelope);
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    expect(store.calculatedVersion).toBeUndefined();
    store.disconnected();
  });
  it('accepts only results for the persisted context after an epoch change', async () => {
    const { store } = await connectedStore();
    const projection = formulaProjection();
    projection.calculation_context = { context_id: rowId, epoch: '2' };
    store.load(projection);
    await store.receive(calculation('2'));
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    store.load(projection);
    const current = calculation('2');
    if (current.payload.case !== 'calculationCommitted') throw new Error('Missing calculation');
    current.payload.value.contextId = uuidBytes(rowId);
    current.payload.value.calculationEpoch = 2n;
    await store.receive(current);
    expect(store.calculationStatus).toBe('complete');
    expect(store.display(cellPosition)).toBe('3');
    store.disconnected();
  });
  it('rejects unsupported formula profile before loading projection', () => {
    const p = formulaProjection();
    const cell = p.sheets[0]?.cells[0];
    if (cell?.value.type !== 'formula') throw new Error('Missing fixture');
    cell.value.value.ast_schema = 99;
    expect(() => decodeProjection(p)).toThrow();
  });
});
