import { afterEach, describe, expect, it, vi } from 'vitest';
import { create, fromBinary } from '@bufbuild/protobuf';
import type { Reply, Request } from '../calculation/protocol';
import {
  CellAddressSchema,
  ClientEnvelopeSchema,
  ServerEnvelopeSchema,
} from '../generated/pxel/wire/v1/session_pb';
import type { FormulaError } from '../generated/pxel/wire/v1/session_pb';
import {
  DocumentStore,
  bytesUuid,
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
it('renders only valid remote presence without changing confirmed document state', async () => {
  const store = new DocumentStore();
  store.projection = projection();
  const before = store.projection.state_hash;
  const envelope = create(ServerEnvelopeSchema, {
    protocolVersion: 1,
    payload: {
      case: 'presenceSnapshot',
      value: {
        documentId: uuidBytes(documentId),
        peers: [
          {
            sessionId: uuidBytes(uuidV7()),
            actorId: uuidBytes(uuidV7()),
            anchor: {
              sheetId: uuidBytes(sheetId),
              rowId: uuidBytes(rowId),
              columnId: uuidBytes(columnId),
            },
            focus: {
              sheetId: uuidBytes(sheetId),
              rowId: uuidBytes(rowId),
              columnId: uuidBytes(columnId),
            },
          },
        ],
      },
    },
  });
  await store.receive(envelope);
  expect(store.remotePresence).toHaveLength(1);
  expect(store.remotePresence[0]?.focus).toEqual({ row: 0, column: 0 });
  expect(store.projection.state_hash).toBe(before);
  expect(store.projection.version).toBe('1');
  store.disconnected();
  expect(store.remotePresence).toHaveLength(0);
  store.dispose();
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
        formula_engine_version: 'pxel-formula-0.7.0',
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
it('initializes every sheet once and addresses preview patches by the owning sheet', () => {
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
  const secondSheet = '01994100-0000-7000-8000-000000000005';
  const secondRow = '01994100-0000-7000-8000-000000000006';
  const secondColumn = '01994100-0000-7000-8000-000000000007';
  const book = projection();
  book.sheets[0]!.cells.push({
    row_id: rowId,
    column_id: columnId,
    value: { type: 'formula', value: { source: "='Data'!A1+1" } },
  });
  book.sheets.push({
    id: secondSheet,
    name: 'Data',
    rows: [secondRow],
    columns: [secondColumn],
    cells: [{ row_id: secondRow, column_id: secondColumn, value: { type: 'number', value: 4 } }],
  });
  const store = new DocumentStore();
  store.load(book);
  const init = requests[0];
  expect(init?.kind).toBe('init');
  if (init?.kind !== 'init') throw new Error('Expected preview init');
  expect(init.data.schema_version).toBe(2);
  expect(init.data.sheets).toHaveLength(2);
  expect(init.data.sheets[1]?.cells[0]?.value).toEqual({ type: 'number', value: 4 });
  expect(store.selectSheet(secondSheet)).toBe(true);
  const selectedInit = requests[1];
  if (selectedInit?.kind !== 'init') throw new Error('Expected preview reinit');
  deliver?.({
    id: selectedInit.id,
    result: {
      schema_version: 1,
      preview: true,
      complete: true,
      full: true,
      document_id: documentId,
      input_version: '1',
      pending_revision: '0',
      formula_engine_version: 'pxel-formula-0.7.0',
      compatibility_profile: 'pxel-excel-subset-v1',
      normalized_result_hash: 'a'.repeat(64),
      evaluated_cells: 1,
      dirty_cells: 1,
      results: [
        {
          sheet_id: sheetId,
          row_id: rowId,
          column_id: columnId,
          value: { type: 'number', value: 5 },
        },
      ],
      cleared: [],
    },
  });
  store.begin('9');
  const patch = requests[2];
  expect(patch?.kind).toBe('apply');
  if (patch?.kind !== 'apply') throw new Error('Expected preview patch');
  expect(patch.data.cells).toEqual([
    {
      sheet_id: secondSheet,
      row_id: secondRow,
      column_id: secondColumn,
      value: { type: 'number', value: 9 },
    },
  ]);
  store.dispose();
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
  it('retries uncertain epoch advancement with the same operation ID and waits for confirmed projection', async () => {
    const { store } = await connectedStore();
    const operationIds: string[] = [];
    let failed = false;
    const fetchMock = vi.fn(async (_url: string, options?: RequestInit) => {
      if (options?.method === 'POST') {
        const body = JSON.parse(String(options.body)) as { operation_id: string };
        operationIds.push(body.operation_id);
        if (!failed) {
          failed = true;
          throw new Error('Network response lost');
        }
        return {
          ok: true,
          json: async () => ({ operation_id: body.operation_id, committed_version: '2' }),
        };
      }
      return {
        ok: true,
        json: async () => ({
          ...projection(),
          version: '2',
          calculation_context: { context_id: uuidV7(), epoch: '1' },
        }),
      };
    });
    vi.stubGlobal('fetch', fetchMock);
    await store.recalculate();
    expect(store.projection?.version).toBe('1');
    expect(store.recalculating).toBe(false);
    expect(store.error).not.toBe('');
    await store.recalculate();
    expect(operationIds).toHaveLength(2);
    expect(operationIds[1]).toBe(operationIds[0]);
    expect(store.projection?.version).toBe('2');
    expect(store.projection?.calculation_context?.epoch).toBe('1');
    expect(store.error).toBe('');
    await store.recalculate();
    expect(operationIds[2]).not.toBe(operationIds[1]);
    store.dispose();
  });

  it('does not advance an epoch with pending cell commands or after a permission refusal', async () => {
    const { store } = await connectedStore();
    const fetchMock = vi.fn(async () => ({ ok: false, status: 403 }));
    vi.stubGlobal('fetch', fetchMock);
    await store.recalculate();
    expect(store.error).toContain('Нет права');
    expect(store.projection?.version).toBe('1');
    store.begin('7');
    store.commit();
    await store.recalculate();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    store.dispose();
  });

  it('sends a bounded stable-ID AddSheet and selects it only after canonical commit', async () => {
    const { store, sent } = await connectedStore();
    expect(store.addSheet()).toBe(true);
    expect(store.projection?.sheets).toHaveLength(1);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'addSheet')
      throw new Error('Missing AddSheet command');
    const proposed = payload.value.operation.kind.value;
    expect(proposed.name).toBe('Лист 2');
    expect(proposed.rows).toHaveLength(1_000);
    expect(proposed.columns).toHaveLength(26);
    const added = {
      id: bytesUuid(proposed.sheetId),
      name: proposed.name,
      rows: proposed.rows.map(bytesUuid),
      columns: proposed.columns.map(bytesUuid),
      cells: [],
    };
    const updated = { ...projection(), version: '2', sheets: [...projection().sheets, added] };
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, status: 200, json: async () => updated })),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            committedVersion: 2n,
            canonicalOperation: payload.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.projection?.sheets).toHaveLength(2);
    expect(store.activeSheetId).toBe(added.id);
    expect(store.pending).toHaveLength(0);
    store.disconnected();
  });
  it('undoes an acknowledged own plain write as a new non-optimistic command', async () => {
    const { store, sent } = await connectedStore();
    expect(store.canUndo).toBe(false);
    expect(store.set({ row: 0, column: 0 }, { type: 'number', value: 42 })).toBe(true);
    const original = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (original.case !== 'command' || original.value.operation?.kind.case !== 'setCellValue')
      throw new Error('Missing original write');
    const originalId = original.value.operationId;
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: originalId,
            committedVersion: 2n,
            canonicalOperation: original.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.canUndo).toBe(true);
    expect(store.undo()).toBe(true);
    expect(store.canUndo).toBe(false);
    expect(store.display({ row: 0, column: 0 })).toBe('42');
    expect(store.pending[0]?.kind).toBe('undo');
    const command = fromBinary(ClientEnvelopeSchema, sent[1] ?? new Uint8Array()).payload;
    if (command.case !== 'command' || !command.value.undoCell)
      throw new Error('Missing undo command');
    expect(command.value.operation).toBeUndefined();
    expect(command.value.undoCell.targetOperationId).toEqual(originalId);
    expect(command.value.operationId).not.toEqual(originalId);
    expect(store.undo()).toBe(false);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            committedVersion: 3n,
            canonicalOperation: {
              kind: {
                case: 'clearCell',
                value: { address: original.value.operation.kind.value.address },
              },
            },
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.projection?.version).toBe('3');
    expect(store.display({ row: 0, column: 0 })).toBe('');
    expect(store.pending).toHaveLength(0);
    expect(store.canUndo).toBe(false);
    store.disconnected();
  });
  it('shows typed undo conflict without applying an inverse locally', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'text', value: 'mine' });
    const original = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (original.case !== 'command' || !original.value.operation)
      throw new Error('Missing original write');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: original.value.operationId,
            committedVersion: 2n,
            canonicalOperation: original.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.undo()).toBe(true);
    const undo = fromBinary(ClientEnvelopeSchema, sent[1] ?? new Uint8Array()).payload;
    if (undo.case !== 'command') throw new Error('Missing undo');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: undo.value.operationId,
            code: 13,
            retryable: false,
          },
        },
      }),
    );
    expect(store.display({ row: 0, column: 0 })).toBe('mine');
    expect(store.error).toContain('ячейка была изменена');
    expect(store.canUndo).toBe(false);
    store.disconnected();
  });
  it('applies a remote restoration using the existing wire value operation', async () => {
    const { store } = await connectedStore();
    const fetchMock = vi.fn(async () => {
      throw new Error('Unexpected full projection read');
    });
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
            latestOperation: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(uuidV7()),
              actorId: uuidBytes(uuidV7()),
              committedVersion: 2n,
              confirmedStateHash: new Uint8Array(32),
              canonicalOperation: {
                kind: {
                  case: 'setCellValue',
                  value: {
                    address: {
                      sheetId: uuidBytes(sheetId),
                      rowId: uuidBytes(rowId),
                      columnId: uuidBytes(columnId),
                    },
                    value: { kind: { case: 'text', value: 'previous' } },
                  },
                },
              },
            },
          },
        },
      }),
    );
    expect(store.projection?.version).toBe('2');
    expect(store.display({ row: 0, column: 0 })).toBe('previous');
    expect(store.canUndo).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
    store.disconnected();
  });
  it('selects a rectangle and copies sparse values including its blank trailing cell', () => {
    const state = projection();
    const secondRow = uuidV7(),
      secondColumn = uuidV7();
    state.sheets[0]!.rows.push(secondRow);
    state.sheets[0]!.columns.push(secondColumn);
    state.sheets[0]!.cells.push(
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 1 } },
      { row_id: rowId, column_id: secondColumn, value: { type: 'boolean', value: true } },
      { row_id: secondRow, column_id: columnId, value: { type: 'text', value: 'x' } },
    );
    const store = new DocumentStore();
    store.load(state);
    store.select({ row: 1, column: 1 }, true);
    expect(store.selectionBounds).toEqual({ startRow: 0, endRow: 1, startColumn: 0, endColumn: 1 });
    expect(store.copySelectionTsv()).toBe('1\tTRUE\nx\t');
    expect(store.pasteOrigin).toEqual({ row: 0, column: 0 });
    store.select({ row: 1, column: 0 });
    expect(store.selectionBounds).toEqual({ startRow: 1, endRow: 1, startColumn: 0, endColumn: 0 });
    store.dispose();
  });
  it('keeps both selection endpoints on stable axes after insertion', () => {
    const store = new DocumentStore();
    const before = projection();
    before.sheets[0]!.rows.push(uuidV7());
    before.sheets[0]!.columns.push(uuidV7());
    store.load(before);
    store.select({ row: 1, column: 1 }, true);
    const after = projection();
    after.version = '2';
    after.sheets[0]!.rows = [uuidV7(), ...before.sheets[0]!.rows];
    after.sheets[0]!.columns = [uuidV7(), ...before.sheets[0]!.columns];
    store.load(after);
    expect(store.selectionAnchor).toEqual({ row: 1, column: 1 });
    expect(store.selected).toEqual({ row: 2, column: 2 });
    store.dispose();
  });
  it('captures a versioned stable-ID range when selection contains a formula', () => {
    const state = projection();
    state.sheets[0]!.columns.push(uuidV7());
    state.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'formula', value: { source: '=1+1', current_text: '=1+1' } },
    });
    const store = new DocumentStore();
    store.load(state);
    store.select({ row: 0, column: 1 }, true);
    expect(store.copySelectionTsv()).toBe('=1+1\t');
    const raw = store.copyRangePayload();
    expect(raw).toContain(rowId);
    expect(raw).toContain(columnId);
    state.version = '2';
    store.load(state);
    expect(store.pasteRangeCopy(raw ?? '')).toBe(false);
    store.dispose();
  });
  it('sends one server-bound range copy and refreshes its canonical projection', async () => {
    const { store, sent } = await connectedStore();
    const expanded = projection();
    expanded.sheets[0]!.rows.push(uuidV7());
    expanded.sheets[0]!.columns.push(uuidV7(), uuidV7(), uuidV7());
    expanded.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: expanded.sheets[0]!.columns[1]!,
      value: {
        type: 'formula',
        value: {
          source: '=A1+1',
          current_text: '=A1+1',
          canonical: '=(A1+1)',
          ast_schema: 1,
          compatibility_profile: 'pxel-excel-subset-v1',
          bound_at_version: '1',
        },
      },
    });
    store.load(expanded);
    store.select({ row: 0, column: 1 }, true);
    const raw = store.copyRangePayload();
    expect(raw).toBeDefined();
    store.select({ row: 1, column: 2 });
    expect(store.pasteRangeCopy(raw ?? '')).toBe(true);
    expect(store.pending).toHaveLength(1);
    const sentCommand = sent
      .map((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload)
      .find((payload) => payload.case === 'command');
    if (!sentCommand || sentCommand.case !== 'command' || !sentCommand.value.copyRange)
      throw new Error('Missing range copy command');
    expect(sentCommand.value.copyRange.rows).toBe(1);
    expect(sentCommand.value.copyRange.columns).toBe(2);
    expect(bytesUuid(sentCommand.value.copyRange.source?.rowId ?? new Uint8Array())).toBe(rowId);
    const next = structuredClone(expanded);
    next.version = '2';
    next.sheets[0]!.cells.push({
      row_id: expanded.sheets[0]!.rows[1]!,
      column_id: expanded.sheets[0]!.columns[2]!,
      value: { type: 'number', value: 7 },
    });
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: sentCommand.value.operationId,
            committedVersion: 2n,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.pending).toHaveLength(0);
    expect(store.projection?.version).toBe('2');
    store.disconnected();
  });
  it('uses current formula text for copy and refuses a stale clipboard version', async () => {
    const store = new DocumentStore();
    const state = projection();
    state.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'formula', value: { source: '=A1', current_text: '=B1' } },
    });
    store.load(state);
    const raw = store.copyFormulaPayload();
    expect(raw).toContain('=B1');
    state.sheets[0]!.cells[0]!.value = { type: 'formula', value: { source: '=A1' } };
    store.load(state);
    expect(store.copyFormulaPayload()).toBeUndefined();
    state.version = '2';
    store.load(state);
    expect(await store.pasteFormulaCopy(raw ?? '')).toBe(false);
    expect(store.error).toContain('Скопируйте');
    store.dispose();
  });
  it('pastes a TSV rectangle as one optimistic durable command', async () => {
    const { store, sent } = await connectedStore();
    const secondRow = uuidV7(),
      secondColumn = uuidV7();
    const expanded = projection();
    expanded.sheets[0]!.rows.push(secondRow);
    expanded.sheets[0]!.columns.push(secondColumn);
    store.load(expanded);
    expect(store.pasteTsv('1\tTRUE\nhello\t')).toBe(true);
    expect(store.pending).toHaveLength(1);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'number', value: 1 });
    expect(store.value({ row: 1, column: 0 })).toEqual({ type: 'text', value: 'hello' });
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'setCellValues')
      throw new Error('Missing batch command');
    expect(payload.value.operation.kind.value.writes).toHaveLength(4);
    expect(store.pasteTsv('=A1\t2')).toBe(false);
    expect(store.pending).toHaveLength(1);
    expect(store.error).toContain('формул');
    store.disconnected();
  });
  it('deletes by stable row ID and cancels a draft when axes change', async () => {
    const { store, sent } = await connectedStore();
    expect(store.deleteSelectedRow()).toBe(false);
    const secondRow = uuidV7();
    const twoRows = projection();
    twoRows.sheets[0]!.rows.push(secondRow);
    store.load(twoRows);
    store.selected = { row: 1, column: 0 };
    store.begin('draft');
    expect(store.deleteSelectedRow()).toBe(true);
    expect(store.deleteSelectedRow()).toBe(false);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'deleteRows')
      throw new Error('Missing row deletion command');
    expect(payload.value.operation.kind.value.rows.map(bytesUuid)).toEqual([secondRow]);
    const after = projection();
    after.version = '2';
    store.load(after);
    expect(store.selected.row).toBe(0);
    expect(store.editing).toBeUndefined();
    expect(store.error).toContain('Структура таблицы изменилась');
    store.disconnected();
  });
  it('queues a stable-anchor insertion and waits for canonical axes', async () => {
    const { store, sent } = await connectedStore();
    expect(store.insertRowBeforeSelection()).toBe(true);
    expect(store.insertRowBeforeSelection()).toBe(false);
    expect(store.sheet?.rows).toEqual([rowId]);
    expect(store.pending).toHaveLength(1);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'insertRows')
      throw new Error('Missing row insertion command');
    expect(bytesUuid(payload.value.operation.kind.value.beforeRowId)).toBe(rowId);
    const inserted = bytesUuid(payload.value.operation.kind.value.rows[0]!);
    const next = projection();
    next.version = '2';
    next.sheets[0]!.rows.unshift(inserted);
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            committedVersion: 2n,
            canonicalOperation: payload.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.sheet?.rows).toEqual([inserted, rowId]);
    expect(store.pending).toHaveLength(0);
    store.disconnected();
  });
  it('inserts a column before the selected stable ID and refreshes canonical axes', async () => {
    const { store, sent } = await connectedStore();
    expect(store.insertColumnBeforeSelection()).toBe(true);
    expect(store.insertColumnBeforeSelection()).toBe(false);
    expect(store.pending).toHaveLength(1);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'insertColumns')
      throw new Error('Missing column insertion command');
    expect(bytesUuid(payload.value.operation.kind.value.beforeColumnId)).toBe(columnId);
    const inserted = bytesUuid(payload.value.operation.kind.value.columns[0]!);
    const next = projection();
    next.version = '2';
    next.sheets[0]!.columns.unshift(inserted);
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            committedVersion: 2n,
            canonicalOperation: payload.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.sheet?.columns).toEqual([inserted, columnId]);
    expect(store.pending).toHaveLength(0);
    store.disconnected();
  });
  it('deletes the selected stable column and waits for canonical axes', async () => {
    const { store, sent } = await connectedStore();
    expect(store.deleteSelectedColumn()).toBe(false);
    const secondColumn = uuidV7();
    const twoColumns = projection();
    twoColumns.sheets[0]!.columns.push(secondColumn);
    store.load(twoColumns);
    store.selected = { row: 0, column: 1 };
    expect(store.deleteSelectedColumn()).toBe(true);
    expect(store.deleteSelectedColumn()).toBe(false);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'deleteColumns')
      throw new Error('Missing column deletion command');
    expect(payload.value.operation.kind.value.columns.map(bytesUuid)).toEqual([secondColumn]);
    const next = projection();
    next.version = '2';
    store.load(next);
    expect(store.selected.column).toBe(0);
    store.disconnected();
  });
  it('queues one bounded row extension and refreshes axes after commit', async () => {
    const { store, sent } = await connectedStore();
    expect(store.extendRows()).toBe(true);
    expect(store.extendRows()).toBe(false);
    expect(store.sheet?.rows).toHaveLength(1);
    expect(store.pending).toHaveLength(1);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0] ?? new Uint8Array()).payload;
    if (payload.case !== 'command' || payload.value.operation?.kind.case !== 'extendRows')
      throw new Error('Missing row extension command');
    const appended = payload.value.operation.kind.value.rows.map(bytesUuid);
    expect(appended).toHaveLength(1000);
    expect(new Set(appended).size).toBe(1000);
    const next = projection();
    next.version = '2';
    next.sheets[0]!.rows.push(...appended);
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            committedVersion: 2n,
            canonicalOperation: payload.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.sheet?.rows).toHaveLength(1001);
    expect(store.pending).toHaveLength(0);
    store.disconnected();
  });
  it('applies an immediately next canonical head event without downloading the workbook', async () => {
    const { store } = await connectedStore();
    const fetchMock = vi.fn(async () => {
      throw new Error('Unexpected full projection read');
    });
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
            latestOperation: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(uuidV7()),
              actorId: uuidBytes(documentId),
              committedVersion: 2n,
              confirmedStateHash: new Uint8Array(32),
              canonicalOperation: {
                kind: {
                  case: 'setCellValue',
                  value: {
                    address: {
                      sheetId: uuidBytes(sheetId),
                      rowId: uuidBytes(rowId),
                      columnId: uuidBytes(columnId),
                    },
                    value: { kind: { case: 'number', value: 42 } },
                  },
                },
              },
            },
          },
        },
      }),
    );
    expect(store.projection?.version).toBe('2');
    expect(store.display({ row: 0, column: 0 })).toBe('42');
    expect(fetchMock).not.toHaveBeenCalled();
    store.disconnected();
  });
  it('refreshes instead of applying a head event with no canonical patch', async () => {
    const { store } = await connectedStore();
    const next = projection();
    next.version = '2';
    next.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'number', value: 42 },
    });
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
            latestOperation: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(uuidV7()),
              actorId: uuidBytes(uuidV7()),
              committedVersion: 2n,
              confirmedStateHash: new Uint8Array(32),
            },
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.projection?.version).toBe('2');
    expect(store.display({ row: 0, column: 0 })).toBe('42');
    store.disconnected();
  });
  it('uses the replay barrier to repair a missing event', async () => {
    const { store } = await connectedStore();
    const next = projection();
    next.version = '3';
    next.sheets[0]!.cells = [
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 73 } },
    ];
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'replayComplete',
          value: {
            documentId: uuidBytes(documentId),
            barrierVersion: 3n,
            resyncRequired: true,
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.projection?.version).toBe('3');
    expect(store.display({ row: 0, column: 0 })).toBe('73');
    store.disconnected();
  });
  it('applies ordered replay events without a full projection read', async () => {
    const { store } = await connectedStore();
    const fetchMock = vi.fn(async () => {
      throw new Error('Unexpected full projection read');
    });
    vi.stubGlobal('fetch', fetchMock);
    for (let version = 2; version <= 4; version++) {
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'documentHeadChanged',
            value: {
              documentId: uuidBytes(documentId),
              headVersion: BigInt(version),
              latestOperation: {
                documentId: uuidBytes(documentId),
                operationId: uuidBytes(uuidV7()),
                actorId: uuidBytes(documentId),
                committedVersion: BigInt(version),
                confirmedStateHash: new Uint8Array(32),
                canonicalOperation: {
                  kind: {
                    case: 'setCellValue',
                    value: {
                      address: {
                        sheetId: uuidBytes(sheetId),
                        rowId: uuidBytes(rowId),
                        columnId: uuidBytes(columnId),
                      },
                      value: { kind: { case: 'number', value: version * 10 } },
                    },
                  },
                },
              },
            },
          },
        }),
      );
    }
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'replayComplete',
          value: { documentId: uuidBytes(documentId), barrierVersion: 4n },
        },
      }),
    );
    expect(store.projection?.version).toBe('4');
    expect(store.display({ row: 0, column: 0 })).toBe('40');
    expect(fetchMock).not.toHaveBeenCalled();
    store.disconnected();
  });
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
  it('refreshes when a head hint contains an event without a wire canonical operation', async () => {
    const { store } = await connectedStore();
    const next = projection();
    next.version = '2';
    next.sheets[0]!.cells = [
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 43 } },
    ];
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
            latestOperation: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(uuidV7()),
              actorId: uuidBytes(uuidV7()),
              committedVersion: 2n,
              confirmedStateHash: new Uint8Array(32),
            },
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.projection?.version).toBe('2');
    expect(store.display({ row: 0, column: 0 })).toBe('43');
    store.disconnected();
  });
  it('uses a full authorized refresh for a structural axis event', async () => {
    const { store } = await connectedStore();
    const appended = uuidV7();
    const next = projection();
    next.version = '2';
    next.sheets[0]!.rows.push(appended);
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
            latestOperation: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(uuidV7()),
              actorId: uuidBytes(uuidV7()),
              committedVersion: 2n,
              confirmedStateHash: new Uint8Array(32),
              canonicalOperation: {
                kind: {
                  case: 'extendRows',
                  value: { sheetId: uuidBytes(sheetId), rows: [uuidBytes(appended)] },
                },
              },
            },
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.projection?.version).toBe('2');
    expect(store.sheet?.rows.at(-1)).toBe(appended);
    store.disconnected();
  });
  it('resynchronizes from the server when a coalesced hint skips a version', async () => {
    const { store } = await connectedStore();
    const next = projection();
    next.version = '3';
    next.sheets[0]!.cells = [
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 77 } },
    ];
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => next }));
    vi.stubGlobal('fetch', fetchMock);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 3n,
            latestOperation: { committedVersion: 3n },
          },
        },
      }),
    );
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(store.projection?.version).toBe('3');
    expect(store.display({ row: 0, column: 0 })).toBe('77');
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
  it('retries consecutive projection rate limits without losing the current document', async () => {
    vi.useFakeTimers();
    const store = new DocumentStore();
    store.projection = projection();
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce({ ok: false, status: 429 })
      .mockResolvedValueOnce({ ok: false, status: 429 })
      .mockResolvedValueOnce({ ok: false, status: 429 })
      .mockResolvedValueOnce({ ok: false, status: 429 })
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => projection() });
    vi.stubGlobal('fetch', fetcher);
    const refresh = store.refresh();
    await vi.runAllTimersAsync();
    expect(await refresh).toBe(true);
    expect(fetcher).toHaveBeenCalledTimes(5);
    expect(store.projection?.document_id).toBe(documentId);
    expect(store.error).toBe('');
    store.dispose();
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
    const commands = sent.filter(
      (bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'command',
    );
    expect(commands).toHaveLength(2);
    expect(commands[1]).toEqual(commands[0]);
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
it('selects sheets by stable ID and applies remote patches to hidden sheets', async () => {
  const second = {
    id: uuidV7(),
    name: 'Plan',
    rows: [uuidV7()],
    columns: [uuidV7()],
    cells: [] as Projection['sheets'][number]['cells'],
  };
  second.cells.push({
    row_id: second.rows[0]!,
    column_id: second.columns[0]!,
    value: { type: 'text', value: 'second' },
  });
  const state = projection();
  state.sheets[0]!.cells.push({
    row_id: rowId,
    column_id: columnId,
    value: { type: 'text', value: 'first' },
  });
  state.sheets.push(second);
  expect(decodeProjection(structuredClone(state)).sheets).toHaveLength(2);
  const store = new DocumentStore();
  store.load(structuredClone(state));
  expect(store.display({ row: 0, column: 0 })).toBe('first');
  await store.receive(
    create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'documentHeadChanged',
        value: {
          documentId: uuidBytes(documentId),
          headVersion: 2n,
          latestOperation: {
            documentId: uuidBytes(documentId),
            operationId: uuidBytes(uuidV7()),
            actorId: uuidBytes(uuidV7()),
            committedVersion: 2n,
            confirmedStateHash: new Uint8Array(32),
            canonicalOperation: {
              kind: {
                case: 'setCellValue',
                value: {
                  address: {
                    sheetId: uuidBytes(second.id),
                    rowId: uuidBytes(second.rows[0]!),
                    columnId: uuidBytes(second.columns[0]!),
                  },
                  value: { kind: { case: 'text', value: 'remote' } },
                },
              },
            },
          },
        },
      },
    }),
  );
  expect(store.projection?.version).toBe('2');
  expect(store.selectSheet(second.id)).toBe(true);
  expect(store.sheet?.name).toBe('Plan');
  expect(store.display({ row: 0, column: 0 })).toBe('remote');
  const refreshed = structuredClone(state);
  refreshed.version = '3';
  refreshed.sheets[1]!.cells[0]!.value = { type: 'text', value: 'updated' };
  store.load(refreshed);
  expect(store.activeSheetId).toBe(second.id);
  expect(store.display({ row: 0, column: 0 })).toBe('updated');
  expect(store.selectSheet(sheetId)).toBe(true);
  expect(store.display({ row: 0, column: 0 })).toBe('first');
  const collision = structuredClone(state);
  collision.sheets[1]!.rows[0] = rowId;
  expect(() => decodeProjection(collision)).toThrow();
  store.dispose();
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
        formulaEngineVersion: 'pxel-formula-0.7.0',
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
it('accepts authoritative calculation for a formula on a hidden sheet', async () => {
  const state = formulaProjection();
  const formula = state.sheets[0]!.cells.pop()!;
  const second = {
    id: uuidV7(),
    name: 'Plan',
    rows: [uuidV7()],
    columns: [uuidV7()],
    cells: [formula],
  };
  formula.row_id = second.rows[0]!;
  formula.column_id = second.columns[0]!;
  state.sheets.push(second);
  vi.stubGlobal('location', { href: 'http://localhost/' });
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({ ok: true, json: async () => structuredClone(state) })),
  );
  const store = new DocumentStore();
  store.load(structuredClone(state));
  expect(await store.connected(uuidBytes(documentId), () => true)).toBe(true);
  const patch = calculation('2', 42);
  if (patch.payload.case !== 'calculationCommitted') throw new Error('Missing calculation fixture');
  patch.payload.value.cells[0]!.address = create(CellAddressSchema, {
    sheetId: uuidBytes(second.id),
    rowId: uuidBytes(second.rows[0]!),
    columnId: uuidBytes(second.columns[0]!),
  });
  await store.receive(patch);
  expect(store.calculationStatus).toBe('complete');
  expect(store.selectSheet(second.id)).toBe(true);
  expect(store.display({ row: 0, column: 0 })).toBe('42');
  store.dispose();
});
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
    expect(
      sent.filter(
        (bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'requestCalculation',
      ),
    ).toHaveLength(1);
    store.disconnected();
    await store.receive(calculation('2', 3, { sequence: 1, cells: false }));
    expect(store.calculatedVersion).toBeUndefined();
    await vi.advanceTimersByTimeAsync(60000);
    expect(
      sent.filter(
        (bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'requestCalculation',
      ),
    ).toHaveLength(1);
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
  it('uses shifted A1 text for editing but preserves authored input otherwise', () => {
    const unchanged = formulaProjection();
    const first = unchanged.sheets[0]?.cells[0];
    if (first?.value.type !== 'formula') throw new Error('Missing fixture');
    first.value.value.current_text = '=(1+2)';
    const original = decodeProjection(unchanged).sheets[0]?.cells[0]?.value;
    expect(original?.type === 'formula' && original.value.source).toBe('=1+2');

    const shifted = formulaProjection();
    const second = shifted.sheets[0]?.cells[0];
    if (second?.value.type !== 'formula') throw new Error('Missing fixture');
    second.value.value.source = '=A2';
    second.value.value.canonical = '=A2';
    second.value.value.current_text = '=A3';
    const resolved = decodeProjection(shifted).sheets[0]?.cells[0]?.value;
    expect(resolved?.type === 'formula' && resolved.value.source).toBe('=A3');
    expect(resolved?.type === 'formula' && resolved.value.authored_source).toBe('=A2');
  });
});
