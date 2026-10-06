import { cellsMenu } from '../ui/cells-menu';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { create, fromBinary } from '@bufbuild/protobuf';
import type { PreviewCell, PreviewResult, Reply, Request } from '../calculation/protocol';
import {
  CellAddressSchema,
  CalculatedCellSchema,
  CalculatedValueSchema,
  ClientEnvelopeSchema,
  ServerEnvelopeSchema,
} from '../generated/pxel/wire/v1/session_pb';
import type { FormulaError } from '../generated/pxel/wire/v1/session_pb';
import { decodeHistory, readHistory } from './history';
import { DEFAULT_STYLE } from './styles';
import { sheetMetrics } from '../grid/geometry';
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

describe('independent optimistic formatting', () => {
  const authored = (state = projection()): Projection => ({
    ...state,
    authored_projection_schema_version: 4,
    style_schema_version: 2,
    style_profile_version: 2,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
    sheets: state.sheets.map((sheet) => ({ ...sheet, styles: sheet.styles ?? [] })),
  });
  it('keeps pending styles separate from confirmed contents and emits one typed intent', async () => {
    const { store, sent } = await connectedStore();
    expect(store.setStyle([{ property: 'font_bold', value: true }])).toBe(true);
    expect(store.style({ row: 0, column: 0 }).font_bold).toBe(true);
    expect(store.confirmedStyles.size).toBe(0);
    expect(store.confirmed.size).toBe(0);
    expect(store.projection?.version).toBe('1');
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1) ?? new Uint8Array()).payload;
    if (payload.case !== 'command') throw new Error('Missing formatting command');
    expect(payload.value.setCellStyle?.patch).toMatchObject({
      schemaVersion: 1,
      profileVersion: 2,
      changes: [{ kind: { case: 'fontBold', value: { action: { case: 'set', value: true } } } }],
    });
    expect(payload.value.operation).toBeUndefined();
    expect(payload.value.setRangeStyle).toBeUndefined();
    expect(store.pending).toHaveLength(1);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            code: 3,
          },
        },
      }),
    );
    expect(store.style({ row: 0, column: 0 }).font_bold).toBe(false);
    expect(store.pending).toHaveLength(0);
    store.dispose();
  });
  it('freezes a rectangular stable target list and enforces both range and patch limits', async () => {
    const { store, sent } = await connectedStore();
    const state = authored();
    const sheet = state.sheets[0]!;
    sheet.rows.push(uuidV7());
    sheet.columns.push(uuidV7());
    store.load(state);
    store.select({ row: 1, column: 1 }, true);
    expect(
      store.setStyle([
        { property: 'fill', value: [200, 220, 240] },
        { property: 'font_italic', value: true },
      ]),
    ).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing range style');
    expect(payload.value.setCellStyle).toBeUndefined();
    expect(payload.value.setRangeStyle?.addresses).toHaveLength(4);
    expect(
      payload.value.setRangeStyle?.addresses.map((a) => [
        bytesUuid(a.rowId),
        bytesUuid(a.columnId),
      ]),
    ).toEqual(sheet.rows.flatMap((row) => sheet.columns.map((column) => [row, column])));
    expect(store.style({ row: 1, column: 1 }).fill).toEqual([200, 220, 240]);
    expect(store.confirmedStyles.size).toBe(0);
    expect(store.setStyle([])).toBe(false);
    expect(
      store.setStyle([
        { property: 'font_bold', value: true },
        { property: 'font_bold', reset: true },
      ]),
    ).toBe(false);
    expect(store.pending).toHaveLength(1);
    sheet.rows = Array.from({ length: 1001 }, () => uuidV7());
    store.load(state);
    store.select({ row: 0, column: 0 });
    store.select({ row: 1000, column: 0 }, true);
    expect(store.setStyle([{ property: 'font_bold', value: true }])).toBe(false);
    expect(store.error).toContain('1000');
    store.dispose();
  });
  it('merges pending leaves over remote styles and clears only rejected local leaves', async () => {
    const { store, sent } = await connectedStore();
    store.setStyle([{ property: 'font_bold', value: true }]);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing style');
    const remote = authored();
    remote.version = '2';
    remote.sheets[0]!.styles = [
      { row_id: rowId, column_id: columnId, style: { ...DEFAULT_STYLE, font_italic: true } },
    ];
    store.load(remote);
    expect(store.style({ row: 0, column: 0 })).toMatchObject({
      font_bold: true,
      font_italic: true,
    });
    expect(store.confirmedStyles.values().next().value?.font_bold).toBe(false);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            code: 9,
          },
        },
      }),
    );
    expect(store.style({ row: 0, column: 0 })).toMatchObject({
      font_bold: false,
      font_italic: true,
    });
    expect(store.projection?.version).toBe('2');
    store.dispose();
  });
  it('retains original bytes on transient rejection then consumes style ACK/history only once', async () => {
    const { store, sent } = await connectedStore();
    store.setStyle([{ property: 'font_bold', value: true }]);
    const original = sent.at(-1)!;
    const payload = fromBinary(ClientEnvelopeSchema, original).payload;
    if (payload.case !== 'command') throw new Error('Missing style');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            code: 7,
            retryable: true,
          },
        },
      }),
    );
    expect(store.pending).toHaveLength(1);
    expect(store.style({ row: 0, column: 0 }).font_bold).toBe(true);
    await vi.advanceTimersByTimeAsync(1500);
    const retry = sent
      .filter((b) => fromBinary(ClientEnvelopeSchema, b).payload.case === 'command')
      .at(-1);
    expect(retry).toEqual(original);
    const confirmed = authored();
    confirmed.version = '2';
    confirmed.sheets[0]!.styles = [
      { row_id: rowId, column_id: columnId, style: { ...DEFAULT_STYLE, font_bold: true } },
    ];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) =>
        url.endsWith('/undo-history')
          ? Response.json(
              historyResponse({
                through_version: '2',
                undo: [
                  { operation_id: bytesUuid(payload.value.operationId), committed_version: '2' },
                ],
              }),
            )
          : { ok: true, json: async () => structuredClone(confirmed) },
      ),
    );
    const ack = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'committed',
        value: {
          documentId: uuidBytes(documentId),
          operationId: payload.value.operationId,
          actorId: uuidBytes(documentId),
          committedVersion: 2n,
          confirmedStateHash: new Uint8Array(32),
        },
      },
    });
    await store.receive(ack);
    await store.receive(ack);
    expect(store.pending).toHaveLength(0);
    expect(store.confirmedStyles.values().next().value?.font_bold).toBe(true);
    expect(store.canUndo).toBe(true);
    expect(store.undo()).toBe(true);
    const undo = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (undo.case !== 'command') throw new Error('Missing style undo');
    expect(undo.value.undoCell?.targetOperationId).toEqual(payload.value.operationId);
    store.dispose();
  });
  it('formats numbers and confirmed formula results without changing editing/clipboard content', async () => {
    const { store } = await connectedStore();
    const state = authored();
    state.sheets[0]!.cells = [
      { row_id: rowId, column_id: columnId, value: { type: 'number', value: 0.125 } },
    ];
    state.sheets[0]!.styles = [
      {
        row_id: rowId,
        column_id: columnId,
        style: { ...DEFAULT_STYLE, number_format: 'percent2' },
      },
    ];
    store.load(state);
    expect(store.display({ row: 0, column: 0 })).toBe('12.50%');
    expect(displayValue(store.value({ row: 0, column: 0 }))).toBe('0.125');
    expect(store.copySelectionTsv()).toBe('0.125');
    store.setStyle([{ property: 'number_format', value: 'decimal2' }]);
    expect(store.display({ row: 0, column: 0 })).toBe('0.13');
    expect(store.confirmed.get(`${rowId}/${columnId}`)).toEqual({ type: 'number', value: 0.125 });
    store.dispose();
  });
  it('hydrates exact authored metadata and patches only changed style leaves, including rollback', async () => {
    const requests: { worker: StyleWorker; request: Request }[] = [];
    class StyleWorker {
      onmessage: ((event: MessageEvent<Reply>) => void) | null = null;
      onerror = null;
      postMessage(request: Request) {
        requests.push({ worker: this, request });
      }
      terminate() {}
    }
    vi.stubGlobal('Worker', StyleWorker);
    const { store, sent } = await connectedStore();
    const state = authored();
    store.load(state);
    const last = () => {
      const entry = requests.at(-1);
      if (!entry) throw new Error('Missing Worker request');
      return entry;
    };
    const finish = (entry: ReturnType<typeof last>, hash: string) =>
      entry.worker.onmessage?.({
        data: {
          id: entry.request.id,
          result: {
            schema_version: 2,
            preview: true,
            complete: true,
            full: entry.request.kind === 'init',
            document_id: documentId,
            input_version: entry.request.data.input_version,
            pending_revision: entry.request.data.pending_revision,
            formula_engine_version: 'pxel-formula-0.16.0',
            compatibility_profile: 'pxel-excel-subset-v1',
            normalized_result_hash: 'a'.repeat(64),
            normalized_authored_state_hash: hash,
            evaluated_cells: 0,
            dirty_cells: 0,
            results: [],
            cleared: [],
          },
        },
      } as unknown as MessageEvent<Reply>);
    const init = last();
    expect(init.request.kind).toBe('init');
    expect(init.request.data).toMatchObject({
      schema_version: 4,
      last_axis_change_version: '1',
      retired_row_ids: [],
      retired_column_ids: [],
      row_activation_floors: [],
      column_activation_floors: [],
      sheets: [{ styles: [] }],
    });
    finish(init, state.state_hash);
    expect(store.previewAuthoredHash).toBe(state.state_hash);
    store.setStyle([{ property: 'font_bold', value: true }]);
    const changed = last();
    expect(changed.request.kind).toBe('patch');
    if (changed.request.kind !== 'patch') throw new Error('Expected authored patch');
    expect(changed.request.data.cells).toEqual([]);
    expect(changed.request.data.styles).toEqual([
      {
        sheet_id: sheetId,
        row_id: rowId,
        column_id: columnId,
        changes: [{ property: 'font_bold', value: true }],
      },
    ]);
    expect(store.previewAuthoredHash).toBeUndefined();
    finish(changed, 'b'.repeat(64));
    expect(store.previewAuthoredHash).toBe('b'.repeat(64));
    expect(store.projection?.state_hash).toBe(state.state_hash);
    const command = sent
      .map((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload)
      .filter((p) => p.case === 'command')
      .at(-1);
    if (command?.case !== 'command') throw new Error('Missing durable style');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 3,
          },
        },
      }),
    );
    const rollback = last();
    if (rollback.request.kind !== 'patch') throw new Error('Missing style rollback');
    expect(rollback.request.data.styles[0]?.changes).toEqual([
      { property: 'font_bold', value: null },
    ]);
    finish(rollback, state.state_hash);
    finish(changed, 'b'.repeat(64));
    expect(store.previewAuthoredHash).toBe(state.state_hash);
    expect(store.style({ row: 0, column: 0 }).font_bold).toBe(false);
    store.setStyle([{ property: 'font_italic', value: true }]);
    store.set({ row: 0, column: 0 }, { type: 'formula', value: { source: '=1+2' } });
    store.load(structuredClone(state));
    const reset = last();
    if (reset.request.kind !== 'init') throw new Error('Missing exact hydration');
    expect(reset.request.data.sheets[0]?.cells).toEqual([]);
    expect(reset.request.data.sheets[0]?.styles).toEqual([]);
    finish(reset, state.state_hash);
    const pending = last();
    if (pending.request.kind !== 'patch') throw new Error('Missing post-hydration overlays');
    expect(pending.request.data.cells[0]?.value).toEqual({
      type: 'formula',
      value: { source: '=1+2' },
    });
    expect(pending.request.data.styles[0]?.changes).toEqual([
      { property: 'font_italic', value: true },
    ]);
    expect(store.previewAuthoredHash).toBeUndefined();
    store.dispose();
    expect(store.previewAuthoredHash).toBeUndefined();
  });
  it('refuses an idle Worker authored hash mismatch without changing confirmed formatting', async () => {
    let deliver: ((request: Request) => void) | undefined;
    class WrongHashWorker {
      onmessage: ((event: MessageEvent<Reply>) => void) | null = null;
      onerror = null;
      postMessage(request: Request) {
        deliver = () =>
          this.onmessage?.({
            data: {
              id: request.id,
              result: {
                schema_version: 2,
                preview: true,
                complete: true,
                full: true,
                document_id: documentId,
                input_version: request.data.input_version,
                pending_revision: request.data.pending_revision,
                formula_engine_version: 'pxel-formula-0.16.0',
                compatibility_profile: 'pxel-excel-subset-v1',
                normalized_result_hash: 'a'.repeat(64),
                normalized_authored_state_hash: 'f'.repeat(64),
                evaluated_cells: 0,
                dirty_cells: 0,
                results: [],
                cleared: [],
              },
            },
          } as unknown as MessageEvent<Reply>);
      }
      terminate() {}
    }
    vi.stubGlobal('Worker', WrongHashWorker);
    const state = authored();
    state.sheets[0]!.styles = [
      { row_id: rowId, column_id: columnId, style: { ...DEFAULT_STYLE, font_bold: true } },
    ];
    const store = new DocumentStore();
    store.load(state);
    deliver?.({} as Request);
    expect(store.previewStatus).toBe('failed');
    expect(store.previewAuthoredHash).toBeUndefined();
    expect(store.style({ row: 0, column: 0 }).font_bold).toBe(true);
    expect(store.projection?.state_hash).toBe(state.state_hash);
    store.dispose();
  });
});

describe('server-authoritative cut and move', () => {
  const cutProjection = (): Projection => {
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    state.sheets[0]!.columns.push(uuidV7(), uuidV7(), uuidV7());
    state.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'text', value: 'source' },
    });
    return state;
  };
  const commands = (sent: Uint8Array[]) =>
    sent
      .map((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload)
      .filter((p) => p.case === 'command');

  it('captures stable geometry without clearing cells and submits exactly one move for overlapping paste', async () => {
    const { store, sent } = await connectedStore();
    const state = cutProjection();
    store.load(state);
    store.select({ row: 0, column: 1 }, true);
    const raw = store.cutRangePayload();
    expect(raw).toBeDefined();
    expect(JSON.parse(raw ?? '')).toMatchObject({
      schema_version: 1,
      intent: 'cut',
      document_id: documentId,
      version: '1',
      sheet_id: sheetId,
      row_id: rowId,
      column_id: columnId,
      rows: 1,
      columns: 2,
    });
    expect(store.cutStatus).toBe('ready');
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    expect(commands(sent)).toHaveLength(0);
    store.select({ row: 0, column: 1 });
    expect(store.pasteCutRange(raw ?? '')).toBe(true);
    const command = commands(sent)[0];
    if (command?.case !== 'command' || !command.value.moveRange) throw new Error('Missing move');
    expect(command.value.baseVersion).toBe(1n);
    expect(command.value.moveRange).toMatchObject({ rows: 1, columns: 2 });
    expect(bytesUuid(command.value.moveRange.source?.rowId ?? new Uint8Array())).toBe(rowId);
    expect(bytesUuid(command.value.moveRange.destination?.columnId ?? new Uint8Array())).toBe(
      state.sheets[0]!.columns[1],
    );
    expect(command.value.operation).toBeUndefined();
    expect(command.value.copyRange).toBeUndefined();
    expect(commands(sent)).toHaveLength(1);
    expect(store.pending[0]?.kind).toBe('moveRange');
    expect(store.cutStatus).toBe('pending');
    expect(store.projection?.version).toBe('1');
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    expect(store.value({ row: 0, column: 1 })).toBeUndefined();
    expect(store.pasteCutRange(raw ?? '')).toBe(false);
    expect(commands(sent)).toHaveLength(1);
    store.dispose();
  });

  it('accepts a same-document marker in another connected tab without local cut state', async () => {
    const first = await connectedStore();
    const second = await connectedStore();
    const state = cutProjection();
    first.store.load(structuredClone(state));
    second.store.load(structuredClone(state));
    const raw = first.store.cutRangePayload();
    expect(second.store.cutStatus).toBeUndefined();
    expect(second.store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
    expect(commands(first.sent)).toHaveLength(0);
    expect(commands(second.sent)).toHaveLength(1);
    expect(second.store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    first.store.dispose();
    second.store.dispose();
  });

  it('rejects foreign, stale and malformed cut markers without fallback or writes', async () => {
    const { store, sent } = await connectedStore();
    const state = cutProjection();
    store.load(state);
    const raw = store.cutRangePayload();
    const valid = JSON.parse(raw ?? '') as Record<string, unknown>;
    for (const patch of [
      { document_id: uuidV7() },
      { version: '0' },
      { sheet_id: uuidV7() },
      { rows: 0 },
      { rows: 1.5 },
      { rows: 1001 },
      { columns: -1 },
      { row_id: uuidV7() },
      { column_id: 'bad' },
      { intent: 'copy' },
      { cut_id: 'bad' },
    ]) {
      expect(
        store.pasteCutRange(JSON.stringify({ ...valid, ...patch }), { row: 1, column: 0 }),
      ).toBe(false);
      expect(store.error).not.toBe('');
    }
    expect(store.pasteCutRange(raw ?? '', { row: 0, column: -1 })).toBe(false);
    expect(store.pasteCutRange(raw ?? '', { row: Number.NaN, column: 0 })).toBe(false);
    expect(store.pasteCutRange(raw ?? '', { row: 2, column: 0 })).toBe(false);
    expect(store.pasteCutRange(raw ?? '', { row: 0, column: 0 })).toBe(false);
    expect(commands(sent)).toHaveLength(0);
    expect(store.pending).toHaveLength(0);
    expect(store.confirmed.get(`${rowId}/${columnId}`)).toEqual({ type: 'text', value: 'source' });
    store.dispose();
  });

  it('marks a cut stale after a remote change and requires a fresh cut', async () => {
    const { store, sent } = await connectedStore();
    const state = cutProjection();
    store.load(state);
    const raw = store.cutRangePayload();
    store.load({ ...state, version: '2' });
    expect(store.cutStatus).toBe('stale');
    expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(false);
    expect(store.error).toContain('Вырежьте');
    expect(commands(sent)).toHaveLength(0);
    const fresh = store.cutRangePayload();
    expect(store.pasteCutRange(fresh ?? '', { row: 1, column: 0 })).toBe(true);
    store.dispose();
  });

  it('does not capture a cut while disconnected, editing or awaiting a command', async () => {
    const { store, sent } = await connectedStore();
    store.load(cutProjection());
    store.begin('draft');
    expect(store.cutRangePayload()).toBeUndefined();
    store.cancel();
    store.set({ row: 0, column: 0 }, { type: 'text', value: 'pending' });
    expect(store.cutRangePayload()).toBeUndefined();
    expect(commands(sent)).toHaveLength(1);
    store.disconnected();
    expect(store.cutRangePayload()).toBeUndefined();
    store.dispose();
  });

  it('abandons local cut decoration when clipboard content changes but keeps TSV paste semantics', async () => {
    const { store, sent } = await connectedStore();
    store.load(cutProjection());
    const raw = store.cutRangePayload();
    store.reconcileCutClipboard(raw ?? '');
    expect(store.cutStatus).toBe('ready');
    store.reconcileCutClipboard('');
    expect(store.cutStatus).toBeUndefined();
    expect(store.pasteTsv('external', { row: 1, column: 0 })).toBe(true);
    expect(commands(sent)[0]?.value.operation?.kind.case).toBe('setCellValues');
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    store.dispose();
  });

  it('never retries a rejected move automatically and preserves a stale cut until recaptured', async () => {
    const { store, sent } = await connectedStore();
    const state = cutProjection();
    store.load(state);
    const raw = store.cutRangePayload();
    expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
    const command = commands(sent)[0];
    if (command?.case !== 'command') throw new Error('Missing command');
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => ({ ...state, version: '2' }) })),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 17,
            retryable: true,
            resyncRequired: true,
          },
        },
      }),
    );
    await vi.advanceTimersByTimeAsync(5_000);
    expect(commands(sent)).toHaveLength(1);
    expect(store.pending).toHaveLength(0);
    expect(store.cutStatus).toBe('stale');
    expect(store.error).toContain('Вырежьте');
    expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(false);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    store.dispose();
  });

  it('keeps a terminal dependency refusal non-destructive and allows an explicit fresh attempt', async () => {
    const { store, sent } = await connectedStore();
    store.load(cutProjection());
    const raw = store.cutRangePayload();
    store.pasteCutRange(raw ?? '', { row: 1, column: 0 });
    const command = commands(sent)[0];
    if (command?.case !== 'command') throw new Error('Missing command');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 18,
            retryable: false,
          },
        },
      }),
    );
    await vi.advanceTimersByTimeAsync(5_000);
    expect(commands(sent)).toHaveLength(1);
    expect(store.cutStatus).toBe('ready');
    expect(store.error).toContain('отклонено');
    expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
    expect(commands(sent)).toHaveLength(2);
    expect(commands(sent)[1]?.value.operationId).not.toEqual(command.value.operationId);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    store.dispose();
  });

  it.each([7, 8])(
    'retains an uncertain move ID after transient rejection %i until a receipt resolves it',
    async (code) => {
      const { store, sent } = await connectedStore();
      store.set({ row: 0, column: 0 }, { type: 'number', value: 7 });
      await acknowledgeCellResult(store, sent, 7);
      expect(store.canUndo).toBe(true);
      sent.length = 0;
      const state = { ...cutProjection(), version: '2' };
      store.load(state);
      const raw = store.cutRangePayload();
      expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
      const first = commands(sent)[0];
      if (first?.case !== 'command') throw new Error('Missing move');
      const originalBytes = store.pending[0]?.bytes;
      // The original move may already be committed, but its ACK never arrives.
      await vi.advanceTimersByTimeAsync(2000);
      expect(commands(sent)).toHaveLength(2);
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'rejected',
            value: {
              documentId: uuidBytes(documentId),
              operationId: first.value.operationId,
              code,
              retryable: true,
            },
          },
        }),
      );
      expect(store.pending).toHaveLength(1);
      expect(store.pending[0]?.bytes).toEqual(originalBytes);
      expect(store.cutStatus).toBe('pending');
      expect(store.cutRangePayload()).toBeUndefined();
      expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(false);
      await vi.advanceTimersByTimeAsync(1500);
      const retries = commands(sent);
      expect(retries).toHaveLength(3);
      for (const retry of retries) expect(retry.value).toEqual(first.value);
      for (const bytes of sent.filter(
        (data) => fromBinary(ClientEnvelopeSchema, data).payload.case === 'command',
      ))
        expect(bytes).toEqual(originalBytes);
      expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });

      const moved = structuredClone(state);
      moved.version = '3';
      moved.sheets[0]!.cells[0]!.row_id = moved.sheets[0]!.rows[1]!;
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => moved })),
      );
      const receipt = create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'receipt',
          value: {
            documentId: uuidBytes(documentId),
            operationId: first.value.operationId,
            committedVersion: 3n,
            currentHeadVersion: 3n,
            canonicalEventHash: new Uint8Array(32),
          },
        },
      });
      await store.receive(receipt);
      expect(store.pending).toHaveLength(0);
      expect(store.cutStatus).toBeUndefined();
      expect(store.canUndo).toBe(true);
      expect(store.canRedo).toBe(false);
      expect(store.value({ row: 0, column: 0 })).toBeUndefined();
      expect(store.value({ row: 1, column: 0 })).toEqual({ type: 'text', value: 'source' });
      await store.receive(receipt);
      await vi.advanceTimersByTimeAsync(5000);
      expect(commands(sent)).toHaveLength(3);
      expect(store.pending).toHaveLength(0);
      store.dispose();
    },
  );

  it('clears the cut and makes the acknowledged move undoable only after projection hydration', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 7 });
    await acknowledgeCellResult(store, sent, 7);
    expect(store.canUndo).toBe(true);
    const state = { ...cutProjection(), version: '2' };
    store.load(state);
    const raw = store.cutRangePayload();
    store.pasteCutRange(raw ?? '', { row: 1, column: 0 });
    const command = commands(sent).at(-1);
    if (command?.case !== 'command') throw new Error('Missing move');
    let resolveRefresh: ((value: unknown) => void) | undefined;
    vi.stubGlobal(
      'fetch',
      vi.fn(
        () =>
          new Promise((resolve) => {
            resolveRefresh = resolve;
          }),
      ),
    );
    const receive = store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            committedVersion: 3n,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.cutStatus).toBe('pending');
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    const moved = structuredClone(state);
    moved.version = '3';
    moved.sheets[0]!.cells[0]!.row_id = moved.sheets[0]!.rows[1]!;
    resolveRefresh?.({ ok: true, json: async () => moved });
    await receive;
    expect(store.cutStatus).toBeUndefined();
    expect(store.pending).toHaveLength(0);
    expect(store.value({ row: 0, column: 0 })).toBeUndefined();
    expect(store.value({ row: 1, column: 0 })).toEqual({ type: 'text', value: 'source' });
    expect(store.canUndo).toBe(true);
    expect(store.canRedo).toBe(false);
    expect(store.undo()).toBe(true);
    expect(commands(sent).at(-1)?.value.undoCell?.targetOperationId).toEqual(
      command.value.operationId,
    );
    store.dispose();
  });

  it('deduplicates overlapping move receipts and ACKs and traverses inverse projection refreshes', async () => {
    const { store, sent } = await connectedStore();
    const original = cutProjection();
    store.load(original);
    const raw = store.cutRangePayload();
    store.pasteCutRange(raw ?? '', { row: 1, column: 0 });
    const move = commands(sent).at(-1)?.value;
    if (!move) throw new Error('Missing move');
    const moved = structuredClone(original);
    moved.version = '2';
    moved.sheets[0]!.cells[0]!.row_id = moved.sheets[0]!.rows[1]!;
    const receipt = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'receipt',
        value: {
          documentId: uuidBytes(documentId),
          operationId: move.operationId,
          committedVersion: 2n,
          currentHeadVersion: 2n,
          canonicalEventHash: new Uint8Array(32),
        },
      },
    });
    const ack = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'committed',
        value: {
          documentId: uuidBytes(documentId),
          operationId: move.operationId,
          committedVersion: 2n,
          confirmedStateHash: new Uint8Array(32),
        },
      },
    });
    let release: (() => void) | undefined;
    let fetches = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        if (++fetches === 1)
          await new Promise<void>((resolve) => {
            release = resolve;
          });
        return { ok: true, json: async () => structuredClone(moved) };
      }),
    );
    const delayedReceipt = store.receive(receipt);
    await store.receive(ack);
    release?.();
    await delayedReceipt;
    await store.receive(receipt);
    await store.receive(ack);
    expect(store.pending).toHaveLength(0);
    expect(store.undo()).toBe(true);
    const undo = commands(sent).at(-1)?.value;
    if (!undo) throw new Error('Missing undo');
    expect(undo.undoCell?.targetOperationId).toEqual(move.operationId);
    expect(store.value({ row: 0, column: 0 })).toBeUndefined();
    expect(store.value({ row: 1, column: 0 })).toEqual({ type: 'text', value: 'source' });
    const restored = { ...original, version: '3' };
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => structuredClone(restored) })),
    );
    const undoAck = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'committed',
        value: {
          documentId: uuidBytes(documentId),
          operationId: undo.operationId,
          committedVersion: 3n,
          confirmedStateHash: new Uint8Array(32),
        },
      },
    });
    await store.receive(undoAck);
    await store.receive(undoAck);
    expect(store.canUndo).toBe(false);
    expect(store.canRedo).toBe(true);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    expect(store.redo()).toBe(true);
    const redo = commands(sent).at(-1)?.value;
    if (!redo) throw new Error('Missing redo');
    expect(redo.undoCell?.targetOperationId).toEqual(undo.operationId);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'text', value: 'source' });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => ({ ...moved, version: '4' }) })),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'receipt',
          value: {
            documentId: uuidBytes(documentId),
            operationId: redo.operationId,
            committedVersion: 4n,
            currentHeadVersion: 4n,
            canonicalEventHash: new Uint8Array(32),
          },
        },
      }),
    );
    await store.receive(receipt);
    expect(store.canRedo).toBe(false);
    expect(store.undo()).toBe(true);
    expect(commands(sent).at(-1)?.value.undoCell?.targetOperationId).toEqual(redo.operationId);
    store.dispose();
  });

  it('bounds mixed move history to 100 entries and branches redo only after a new committed move', async () => {
    const { store, sent } = await connectedStore();
    store.load(cutProjection());
    for (let value = 0; value < 100; value++) {
      store.set({ row: 0, column: 0 }, { type: 'number', value });
      await acknowledgeCellResult(store, sent, value);
    }
    const moveOnce = async () => {
      const before = structuredClone(store.projection!);
      const raw = store.cutRangePayload();
      expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
      const move = commands(sent).at(-1)?.value;
      if (!move) throw new Error('Missing move');
      const after = structuredClone(before);
      after.version = String(BigInt(before.version) + 1n);
      after.sheets[0]!.cells[0]!.row_id = after.sheets[0]!.rows[1]!;
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => after })),
      );
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'committed',
            value: {
              documentId: uuidBytes(documentId),
              operationId: move.operationId,
              committedVersion: BigInt(after.version),
              confirmedStateHash: new Uint8Array(32),
            },
          },
        }),
      );
      expect(store.canRedo).toBe(false);
      expect(store.undo()).toBe(true);
      const undo = commands(sent).at(-1)?.value;
      if (!undo) throw new Error('Missing inverse');
      before.version = String(BigInt(after.version) + 1n);
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => before })),
      );
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'committed',
            value: {
              documentId: uuidBytes(documentId),
              operationId: undo.operationId,
              committedVersion: BigInt(before.version),
              confirmedStateHash: new Uint8Array(32),
            },
          },
        }),
      );
      expect(store.canRedo).toBe(true);
    };
    await moveOnce();
    await moveOnce();
    for (let value = 98; value >= 0; value--) {
      expect(store.undo()).toBe(true);
      await acknowledgeCellResult(store, sent, value);
    }
    expect(store.canUndo).toBe(false);
    store.dispose();
  });
});

const documentId = '01994100-0000-7000-8000-000000000001';
const rowId = '01994100-0000-7000-8000-000000000002';
const columnId = '01994100-0000-7000-8000-000000000003';
const sheetId = '01994100-0000-7000-8000-000000000004';
const previewContext = (
  context_id: string,
  epoch: string,
): NonNullable<Projection['calculation_context']> => ({
  schema_version: 1,
  context_id,
  epoch,
  effective_timestamp_utc_ms: '86400000',
  utc_offset_minutes: 180,
  civil_day_since_unix_epoch: 1,
  random_seed: '01'.repeat(32),
  engine_version: 'pxel-formula-0.16.0',
  compatibility_profile: 'pxel-excel-subset-v1',
  reason: 'explicit',
});
const projection = (): Projection => ({
  document_id: documentId,
  version: '1',
  state_hash: '0'.repeat(64),
  sheets: [{ id: sheetId, name: 'Лист 1', rows: [rowId], columns: [columnId], cells: [] }],
});
describe('durable Cells menu commands', () => {
  const settingsProjection = (): Projection => ({
    ...projection(),
    authored_projection_schema_version: 8,
    style_schema_version: 2,
    style_profile_version: 2,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
    sheets: [
      {
        ...projection().sheets[0]!,
        rows: [rowId, uuidV7(), uuidV7()],
        columns: [columnId, uuidV7(), uuidV7()],
        styles: [],
        merges: [],
        row_heights: [],
        column_widths: [],
        frozen_rows: 0,
        hidden: false,
        hidden_rows: [],
        hidden_columns: [],
        default_column_width: 112,
        tab_color: null,
      },
    ],
  });
  it('validates stable hidden IDs, default width, colour and the last visible sheet', () => {
    const state = settingsProjection();
    expect(decodeProjection(state)).toEqual(state);
    for (const patch of [
      { hidden_rows: [uuidV7()] },
      { default_column_width: 23 },
      { tab_color: 0x1000000 },
      { hidden_rows: [rowId, rowId] },
      { hidden: true },
    ]) {
      const broken = structuredClone(state);
      Object.assign(broken.sheets[0]!, patch);
      expect(() => decodeProjection(broken)).toThrow();
    }
  });
  it('sends one settings intent and keeps confirmed metadata unchanged until ACK', async () => {
    const { store, sent } = await connectedStore();
    store.load(settingsProjection());
    expect(store.renameSheet('Новый лист')).toBe(true);
    expect(store.sheet?.name).toBe('Лист 1');
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing settings command');
    expect(payload.value.sheetSettings).toMatchObject({ schemaVersion: 1, name: 'Новый лист' });
    expect(store.renameSheet('Вторая правка')).toBe(false);
    expect(store.pending).toHaveLength(1);
    store.dispose();
  });
  it('does not enqueue unchanged settings and refuses invalid names or hiding all axes', async () => {
    const { store, sent } = await connectedStore();
    store.load(settingsProjection());
    expect(store.renameSheet('Лист 1')).toBe(true);
    expect(store.moveOrCopySheet(0, false)).toBe(true);
    expect(store.setTabColor(null)).toBe(true);
    expect(store.setDefaultColumnWidth(112)).toBe(true);
    expect(store.pending).toHaveLength(0);
    expect(store.renameSheet('bad/name')).toBe(false);
    expect(store.renameSheet('bad\0name')).toBe(false);
    expect(store.setSheetHidden(true)).toBe(false);
    store.select({ row: 2, column: 0 }, true);
    expect(store.setAxisVisibility('row', true)).toBe(false);
    expect(sent).toHaveLength(0);
    store.dispose();
  });
  it('preserves retired hidden identities without hiding live cells or sending stale targets', async () => {
    const { store } = await connectedStore();
    const state = settingsProjection(),
      retired = uuidV7();
    state.retired_row_ids = [retired];
    state.sheets[0]!.hidden_rows = [retired];
    expect(decodeProjection(state)).toEqual(state);
    store.load(state);
    expect(store.hiddenRows.size).toBe(0);
    expect(store.setAxisVisibility('row', false)).toBe(true);
    expect(store.pending).toHaveLength(0);
    store.dispose();
  });
  it('copies only visible cells and refuses an internal rectangular copy across hidden axes', async () => {
    const { store } = await connectedStore();
    const state = settingsProjection(),
      sheet = state.sheets[0]!;
    sheet.hidden_rows = [sheet.rows[1]!];
    sheet.hidden_columns = [sheet.columns[1]!];
    sheet.cells = sheet.rows.flatMap((row, r) =>
      sheet.columns.map((column, c) => ({
        row_id: row,
        column_id: column,
        value: { type: 'text' as const, value: `${r}${c}` },
      })),
    );
    store.load(state);
    store.select({ row: 2, column: 2 }, true);
    expect(store.copySelectionTsv()).toBe('00\t02\n20\t22');
    expect(store.copyRangePayload()).toBeUndefined();
    store.dispose();
  });
  it.each(['row', 'column'] as const)(
    'shows large hidden %s sets in repeatable bounded commands',
    async (axis) => {
      const { store, sent } = await connectedStore();
      const state = settingsProjection(),
        sheet = state.sheets[0]!;
      const ids = Array.from({ length: 1002 }, () => uuidV7());
      if (axis === 'row') {
        sheet.rows = ids;
        sheet.hidden_rows = ids.slice(0, 1001);
      } else {
        sheet.columns = ids;
        sheet.hidden_columns = ids.slice(0, 1001);
      }
      store.load(state);
      const menu = cellsMenu(
        store,
        true,
        () => {},
        () => {},
      );
      const visibility = menu.actions
        .find((action) => action.label === 'Форматировать')
        ?.children?.find((action) => action.label === 'Видимость');
      const show = visibility?.children?.find(
        (action) =>
          action.label ===
          (axis === 'row' ? 'Показать 1000 скрытых строк' : 'Показать 1000 скрытых столбцов'),
      );
      expect(show?.disabled).toBe(false);
      show?.run?.();
      const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
      if (payload.case !== 'command') throw new Error('Missing visibility command');
      expect(payload.value.sheetSettings?.visibility).toHaveLength(1000);
      expect(payload.value.sheetSettings?.visibility.every((entry) => !entry.hidden)).toBe(true);
      expect(axis === 'row' ? sheet.hidden_rows : sheet.hidden_columns).toHaveLength(1001);
      store.dispose();
    },
  );
  it('autofits selected columns in one command using displayed text and preserves confirmed widths', async () => {
    const { store, sent } = await connectedStore();
    const state = settingsProjection(),
      sheet = state.sheets[0]!;
    sheet.cells = [
      {
        row_id: sheet.rows[0]!,
        column_id: sheet.columns[0]!,
        value: { type: 'text', value: 'long text' },
      },
      { row_id: sheet.rows[0]!, column_id: sheet.columns[1]!, value: { type: 'text', value: 'x' } },
    ];
    store.load(state);
    store.select({ row: 0, column: 1 }, true);
    vi.stubGlobal('document', {
      createElement: () => ({
        getContext: () => ({ measureText: (text: string) => ({ width: text.length * 8 }) }),
      }),
    });
    expect(store.autoFit('column')).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing autofit command');
    expect(payload.value.setAxisSizes?.writes.map((w) => w.sizeCssPx)).toEqual([86, 24]);
    expect(sheetMetrics(store)?.columns.sizeAt(0)).toBe(86);
    expect(sheetMetrics(store)?.columns.sizeAt(1)).toBe(24);
    expect(sheet.column_widths).toEqual([]);
    expect(store.projection?.version).toBe('1');
    store.dispose();
  });
  it('autofit reads new confirmed websocket text before reload', async () => {
    const { store, sent } = await connectedStore();
    const sheet = store.sheet!;
    expect(sheet.cells).toEqual([]);
    store.confirmed.set(`${sheet.rows[0]}/${sheet.columns[0]}`, {
      type: 'text',
      value: 'long text',
    });
    vi.stubGlobal('document', {
      createElement: () => ({
        getContext: () => ({ measureText: (text: string) => ({ width: text.length * 8 }) }),
      }),
    });
    expect(store.autoFit('column')).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing autofit command');
    expect(payload.value.setAxisSizes?.writes.map((write) => write.sizeCssPx)).toEqual([86]);
    expect(sheet.cells).toEqual([]);
    store.dispose();
  });
  it('autofit waits for results of a new confirmed websocket formula before reload', async () => {
    const { store, sent } = await connectedStore();
    const sheet = store.sheet!;
    store.confirmed.set(`${sheet.rows[0]}/${sheet.columns[0]}`, {
      type: 'formula',
      value: { source: '=1' },
    });
    expect(store.autoFit('column')).toBe(false);
    expect(sent).toHaveLength(0);
    expect(store.error).toContain('расчёта формул');
    store.dispose();
  });
  it('keeps the latest resize even when it returns to confirmed width and resets to sheet default', async () => {
    const { store } = await connectedStore();
    const state = settingsProjection(),
      sheet = state.sheets[0]!;
    sheet.default_column_width = 160;
    sheet.column_widths = [{ axis_id: sheet.columns[0]!, size_css_px: 80 }];
    store.load(state);
    expect(store.setAxisSize('column', 0, 100)).toBe(true);
    expect(store.setAxisSize('column', 0, 80)).toBe(true);
    expect(store.pending).toHaveLength(2);
    expect(store.axisSize('column', 0)).toBe(80);
    expect(sheetMetrics(store)?.columns.sizeAt(0)).toBe(80);
    expect(store.setAxisSize('column', 0, undefined)).toBe(true);
    expect(store.axisSize('column', 0)).toBe(160);
    expect(sheetMetrics(store)?.columns.sizeAt(0)).toBe(160);
    expect(sheet.column_widths[0]?.size_css_px).toBe(80);
    store.dispose();
  });
  it('blocks autofill for live hidden axes while retired hidden memberships stay inert', async () => {
    const { store } = await connectedStore();
    const state = settingsProjection(),
      sheet = state.sheets[0]!,
      retired = uuidV7();
    state.retired_row_ids = [retired];
    sheet.hidden_rows = [retired];
    store.load(state);
    expect(store.canAutofill).toBe(true);
    sheet.hidden_columns = [sheet.columns[1]!];
    store.load(structuredClone(state));
    expect(store.canAutofill).toBe(false);
    store.dispose();
  });
  it('autofit keeps hidden axes and merged geometry unchanged', async () => {
    const { store, sent } = await connectedStore();
    const state = settingsProjection(),
      sheet = state.sheets[0]!;
    sheet.hidden_columns = [sheet.columns[1]!];
    sheet.merges = [
      {
        first_row_id: sheet.rows[0]!,
        last_row_id: sheet.rows[1]!,
        first_column_id: sheet.columns[0]!,
        last_column_id: sheet.columns[0]!,
      },
    ];
    store.load(state);
    store.select({ row: 0, column: 2 }, true);
    vi.stubGlobal('document', {
      createElement: () => ({
        getContext: () => ({ measureText: (text: string) => ({ width: text.length * 8 }) }),
      }),
    });
    expect(store.autoFit('column')).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing autofit command');
    expect(payload.value.setAxisSizes?.writes.map((w) => bytesUuid(w.axisId))).toEqual([
      sheet.columns[2],
    ]);
    store.dispose();
  });
  it('hides selected stable rows and retains filter and sheet hidden identities separately', async () => {
    const { store, sent } = await connectedStore();
    const state = settingsProjection();
    state.sheets[0]!.hidden_columns = [state.sheets[0]!.columns[1]!];
    store.load(state);
    store.select({ row: 1, column: 0 }, true);
    expect(store.setAxisVisibility('row', true)).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing visibility command');
    expect(payload.value.sheetSettings?.visibility).toHaveLength(2);
    expect(store.hiddenColumns.has(1)).toBe(true);
    expect(store.hiddenRows.size).toBe(0);
    store.dispose();
  });
  it('reveals hidden sheets by switching to the first visible sheet after refresh', () => {
    const store = new DocumentStore();
    const state = settingsProjection();
    store.load(state);
    const next = structuredClone(state);
    next.sheets.push({
      ...next.sheets[0]!,
      id: uuidV7(),
      name: 'Другой',
      rows: [uuidV7()],
      columns: [uuidV7()],
    });
    next.sheets[0]!.hidden = true;
    store.load(next);
    expect(store.sheet?.name).toBe('Другой');
    expect(store.selectSheet(state.sheets[0]!.id)).toBe(false);
    store.dispose();
  });
  it('copies a sheet with new IDs in one command without creating an optimistic tab', async () => {
    const { store, sent } = await connectedStore();
    store.load(settingsProjection());
    expect(store.moveOrCopySheet(1, true, 'Копия')).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing copy command');
    expect(payload.value.copySheet?.sheet?.rows).toHaveLength(3);
    expect(bytesUuid(payload.value.copySheet!.sheet!.rows[0]!)).not.toBe(rowId);
    expect(store.projection?.sheets).toHaveLength(1);
    store.dispose();
  });
});
it('validates the complete persisted context before preview initialization', () => {
  const valid = { ...projection(), version: '2', calculation_context: previewContext(rowId, '2') };
  expect(decodeProjection(valid).calculation_context).toEqual(valid.calculation_context);
  for (const [key, value] of [
    ['schema_version', 2],
    ['epoch', '3'],
    ['epoch', '18446744073709551616'],
    ['effective_timestamp_utc_ms', '9223372036854775808'],
    ['utc_offset_minutes', 841],
    ['civil_day_since_unix_epoch', 2],
    ['random_seed', 'aa'],
    ['reason', 'unknown'],
  ]) {
    const broken = structuredClone(valid);
    Object.assign(broken.calculation_context, { [String(key)]: value });
    expect(() => decodeProjection(broken)).toThrow();
  }
  const negativeDay = {
    ...valid,
    calculation_context: {
      ...valid.calculation_context,
      effective_timestamp_utc_ms: '0',
      utc_offset_minutes: -1,
      civil_day_since_unix_epoch: -1,
    },
  };
  expect(decodeProjection(negativeDay).calculation_context?.civil_day_since_unix_epoch).toBe(-1);
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
        formula_engine_version: 'pxel-formula-0.16.0',
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
      formula_engine_version: 'pxel-formula-0.16.0',
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
describe('acknowledged own axis history', () => {
  it.each(['insertRows', 'deleteRows', 'insertColumns', 'deleteColumns'] as const)(
    'undoes and redoes %s after canonical hydration while following stable selection',
    async (kind) => {
      const { store, sent } = await connectedStore();
      const before = projection();
      const otherRow = uuidV7(),
        otherColumn = uuidV7();
      before.sheets[0]!.rows.push(otherRow);
      before.sheets[0]!.columns.push(otherColumn);
      before.sheets[0]!.cells.push({
        row_id: otherRow,
        column_id: otherColumn,
        value: { type: 'number', value: 7 },
      });
      store.load(before);
      const submit = {
        insertRows: () => store.insertRowBeforeSelection(),
        deleteRows: () => store.deleteSelectedRow(),
        insertColumns: () => store.insertColumnBeforeSelection(),
        deleteColumns: () => store.deleteSelectedColumn(),
      }[kind];
      expect(submit()).toBe(true);
      const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
      if (payload.case !== 'command' || !payload.value.operation)
        throw new Error('Missing axis command');
      const change = payload.value.operation.kind;
      expect(change.case).toBe(kind);
      const after = structuredClone(before);
      after.version = '2';
      if (change.case === 'insertRows')
        after.sheets[0]!.rows.unshift(bytesUuid(change.value.rows[0]!));
      if (change.case === 'deleteRows') after.sheets[0]!.rows.shift();
      if (change.case === 'insertColumns')
        after.sheets[0]!.columns.unshift(bytesUuid(change.value.columns[0]!));
      if (change.case === 'deleteColumns') after.sheets[0]!.columns.shift();
      store.select({ row: 1, column: 1 });
      let finish: ((result: unknown) => void) | undefined;
      vi.stubGlobal(
        'fetch',
        vi.fn(
          () =>
            new Promise((resolve) => {
              finish = resolve;
            }),
        ),
      );
      const committed = create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: payload.value.operationId,
            committedVersion: 2n,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      });
      const hydration = store.receive(committed);
      expect(store.canUndo).toBe(false);
      expect(store.projection).toEqual(before);
      finish?.({ ok: true, json: async () => after });
      await hydration;
      expect(store.canUndo).toBe(true);
      expect(store.sheet?.rows[store.selected.row]).toBe(otherRow);
      expect(store.sheet?.columns[store.selected.column]).toBe(otherColumn);
      expect(store.undo()).toBe(true);
      const inverse = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
      if (inverse.case !== 'command' || !inverse.value.undoCell) throw new Error('Missing inverse');
      expect(inverse.value.undoCell.targetOperationId).toEqual(payload.value.operationId);
      expect(store.projection).toEqual(after);
      const restored = structuredClone(before);
      restored.version = '3';
      // A canonical projection may include an unrelated retained edit; the
      // client must not restore a local copy of the old whole workbook.
      restored.sheets[0]!.cells[0]!.value = { type: 'number', value: 99 };
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => restored })),
      );
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'committed',
            value: {
              documentId: uuidBytes(documentId),
              operationId: inverse.value.operationId,
              committedVersion: 3n,
              confirmedStateHash: new Uint8Array(32),
            },
          },
        }),
      );
      expect(store.projection).toEqual(restored);
      expect(store.value(store.selected)).toEqual({ type: 'number', value: 99 });
      expect(store.selected).toEqual({ row: 1, column: 1 });
      expect(store.canUndo).toBe(false);
      expect(store.canRedo).toBe(true);
      await store.receive(committed);
      expect(store.canUndo).toBe(false);
      expect(store.redo()).toBe(true);
      const redo = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
      if (redo.case !== 'command' || !redo.value.undoCell) throw new Error('Missing redo');
      expect(redo.value.undoCell.targetOperationId).toEqual(inverse.value.operationId);
      const redone = structuredClone(after);
      redone.version = '4';
      redone.sheets[0]!.cells[0]!.value = { type: 'number', value: 99 };
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => redone })),
      );
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'committed',
            value: {
              documentId: uuidBytes(documentId),
              operationId: redo.value.operationId,
              committedVersion: 4n,
              confirmedStateHash: new Uint8Array(32),
            },
          },
        }),
      );
      expect(store.projection).toEqual(redone);
      expect(store.canUndo).toBe(true);
      expect(store.canRedo).toBe(false);
      expect(store.sheet?.rows[store.selected.row]).toBe(otherRow);
      expect(store.sheet?.columns[store.selected.column]).toBe(otherColumn);
      store.dispose();
    },
  );
});

describe('isolated pending move preview', () => {
  function workerFixture() {
    const requests: { request: Request; worker: PreviewWorker }[] = [];
    class PreviewWorker {
      onmessage: ((event: MessageEvent<Reply>) => void) | null = null;
      onerror: (() => void) | null = null;
      terminated = false;
      postMessage(request: Request) {
        requests.push({ request, worker: this });
      }
      terminate() {
        this.terminated = true;
      }
    }
    vi.stubGlobal('Worker', PreviewWorker);
    const latest = () => {
      const entry = requests.at(-1);
      if (!entry) throw new Error('Expected a Worker request');
      return entry;
    };
    const finish = (
      entry: (typeof requests)[number],
      writes?: PreviewCell[],
      results: PreviewResult['results'] = [],
    ) =>
      entry.worker.onmessage?.(
        new MessageEvent<Reply>('message', {
          data: {
            id: entry.request.id,
            result: {
              document_id: entry.request.data.document_id,
              input_version: entry.request.data.input_version,
              pending_revision: entry.request.data.pending_revision,
              schema_version: 1,
              preview: true,
              complete: true,
              full: entry.request.kind === 'init',
              formula_engine_version: 'pxel-formula-0.16.0',
              compatibility_profile: 'pxel-excel-subset-v1',
              normalized_result_hash: 'a'.repeat(64),
              evaluated_cells: results.length,
              dirty_cells: results.length,
              results,
              cleared: [],
              ...(writes ? { move_writes: writes } : {}),
            },
          },
        }),
      );
    return { requests, latest, finish };
  }
  async function pendingMove() {
    const worker = workerFixture();
    const { store, sent } = await connectedStore();
    const original = projection();
    original.sheets[0]!.rows.push(uuidV7());
    original.sheets[0]!.columns.push(uuidV7(), uuidV7());
    original.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'number', value: 7 },
    });
    store.load(original);
    worker.finish(worker.latest());
    const raw = store.cutRangePayload();
    expect(store.pasteCutRange(raw ?? '', { row: 1, column: 0 })).toBe(true);
    const entry = worker.latest();
    expect(entry.request.kind).toBe('move');
    const destinationRow = original.sheets[0]!.rows[1]!;
    const writes: PreviewCell[] = [
      { sheet_id: sheetId, row_id: rowId, column_id: columnId, value: { type: 'blank' } },
      {
        sheet_id: sheetId,
        row_id: destinationRow,
        column_id: columnId,
        value: { type: 'number', value: 7 },
      },
    ];
    return { ...worker, store, sent, entry, writes, original, destinationRow };
  }

  it('shows scalar move contents and a preview badge without changing confirmed content or hash', async () => {
    const fixture = await pendingMove();
    const { store, entry, writes, original, finish, sent } = fixture;
    const confirmed = [...store.confirmed];
    expect(store.display({ row: 0, column: 0 })).toBe('7');
    expect(store.display({ row: 1, column: 0 })).toBe('');
    finish(entry, writes);
    expect(store.value({ row: 0, column: 0 })).toEqual({ type: 'blank' });
    expect(store.display({ row: 0, column: 0 })).toBe('');
    expect(store.value({ row: 1, column: 0 })).toEqual({ type: 'number', value: 7 });
    expect(store.display({ row: 1, column: 0 })).toBe('7');
    expect(store.previewVisible).toBe(true);
    expect([...store.confirmed]).toEqual(confirmed);
    expect(store.projection).toEqual(original);
    expect(store.calculated.size).toBe(0);
    expect(store.pending).toHaveLength(1);
    expect(
      sent.filter((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'command'),
    ).toHaveLength(1);
    store.dispose();
  });

  it('renders moved and inbound formula text and results exclusively from the matching preview', async () => {
    const fixture = await pendingMove();
    const { store, entry, writes, original, destinationRow, finish } = fixture;
    const externalColumn = original.sheets[0]!.columns[2]!;
    const movedFormula: PreviewCell = {
      sheet_id: sheetId,
      row_id: destinationRow,
      column_id: columnId,
      value: { type: 'formula', value: { source: '=(B2+1)' } },
    };
    const inboundFormula: PreviewCell = {
      sheet_id: sheetId,
      row_id: rowId,
      column_id: externalColumn,
      value: { type: 'formula', value: { source: '=(A2+1)' } },
    };
    finish(
      entry,
      [writes[0]!, movedFormula, inboundFormula],
      [
        { ...movedFormula, value: { type: 'number', value: 8 } },
        { ...inboundFormula, value: { type: 'number', value: 9 } },
      ],
    );
    expect(store.value({ row: 1, column: 0 })).toEqual(movedFormula.value);
    expect(store.value({ row: 0, column: 2 })).toEqual(inboundFormula.value);
    expect(store.display({ row: 1, column: 0 })).toBe('8');
    expect(store.display({ row: 0, column: 2 })).toBe('9');
    expect(store.confirmed.size).toBe(1);
    expect(store.projection).toEqual(original);
    store.dispose();
  });

  it.each([17, 18])(
    'resets a move preview after terminal rejection %i and ignores the late Worker',
    async (code) => {
      const { store, entry, writes, original, finish } = await pendingMove();
      finish(entry, writes);
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => original })),
      );
      const pending = store.pending[0]!;
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'rejected',
            value: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(pending.id),
              code,
              retryable: code === 17,
              resyncRequired: code === 17,
            },
          },
        }),
      );
      expect(entry.worker.terminated).toBe(true);
      finish(entry, writes);
      expect(store.previewVisible).toBe(false);
      expect(store.pending).toHaveLength(0);
      expect(store.display({ row: 0, column: 0 })).toBe('7');
      expect(store.value({ row: 1, column: 0 })).toBeUndefined();
      expect(store.projection).toEqual(original);
      store.dispose();
    },
  );

  it.each([7, 8])(
    'retains preview and exact move bytes during uncertain rejection %i, then retires on receipt',
    async (code) => {
      const { store, entry, writes, original, destinationRow, finish, sent } = await pendingMove();
      finish(entry, writes);
      const pending = store.pending[0]!;
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'rejected',
            value: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(pending.id),
              code,
              retryable: true,
            },
          },
        }),
      );
      expect(store.previewVisible).toBe(true);
      expect(store.display({ row: 1, column: 0 })).toBe('7');
      expect(entry.worker.terminated).toBe(false);
      await vi.advanceTimersByTimeAsync(1500);
      const commands = sent.filter(
        (bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'command',
      );
      expect(commands).toEqual([pending.bytes, pending.bytes]);
      const committed = structuredClone(original);
      committed.version = '2';
      committed.sheets[0]!.cells[0]!.row_id = destinationRow;
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => ({ ok: true, json: async () => committed })),
      );
      await store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'receipt',
            value: {
              documentId: uuidBytes(documentId),
              operationId: uuidBytes(pending.id),
              committedVersion: 2n,
              currentHeadVersion: 2n,
              canonicalEventHash: new Uint8Array(32),
            },
          },
        }),
      );
      finish(entry, writes);
      expect(store.previewVisible).toBe(false);
      expect(store.pending).toHaveLength(0);
      expect(store.projection).toEqual(committed);
      expect(store.display({ row: 1, column: 0 })).toBe('7');
      expect(store.value({ row: 0, column: 0 })).toBeUndefined();
      store.dispose();
    },
  );

  it('drops a move as soon as a newer head requires resync and rejects an old ready reply', async () => {
    const { store, entry, writes, original, finish } = await pendingMove();
    finish(entry, writes);
    let complete: ((value: unknown) => void) | undefined;
    vi.stubGlobal(
      'fetch',
      vi.fn(
        () =>
          new Promise((resolve) => {
            complete = resolve;
          }),
      ),
    );
    const refresh = store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'documentHeadChanged',
          value: {
            documentId: uuidBytes(documentId),
            headVersion: 2n,
          },
        },
      }),
    );
    expect(store.previewVisible).toBe(false);
    expect(store.display({ row: 0, column: 0 })).toBe('7');
    expect(entry.worker.terminated).toBe(true);
    finish(entry, writes);
    expect(store.value({ row: 1, column: 0 })).toBeUndefined();
    complete?.({ ok: true, json: async () => ({ ...original, version: '2' }) });
    await refresh;
    expect(store.pending).toHaveLength(1);
    expect(store.projection?.version).toBe('2');
    store.dispose();
  });

  it('resets from confirmed cells when another draft supersedes the move preview', async () => {
    const { store, entry, writes, finish, latest } = await pendingMove();
    finish(entry, writes);
    store.select({ row: 0, column: 2 });
    store.begin('draft');
    const reset = latest();
    expect(reset.request.kind).toBe('init');
    if (reset.request.kind !== 'init') throw new Error('Expected confirmed preview reset');
    expect(reset.request.data.sheets[0]?.cells).toEqual(
      expect.arrayContaining([
        { row_id: rowId, column_id: columnId, value: { type: 'number', value: 7 } },
      ]),
    );
    expect(reset.request.data.sheets[0]?.cells.some((cell) => cell.row_id !== rowId)).toBe(false);
    finish(entry, writes);
    expect(store.display({ row: 0, column: 0 })).toBe('7');
    expect(store.display({ row: 1, column: 0 })).toBe('');
    expect(store.pending).toHaveLength(1);
    store.dispose();
  });

  it('never makes preview failure or disposal a prerequisite for sending the durable move', async () => {
    const { store, entry, writes, finish, sent } = await pendingMove();
    entry.worker.onerror?.();
    expect(store.previewVisible).toBe(false);
    expect(store.pending).toHaveLength(1);
    expect(
      sent.some((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'command'),
    ).toBe(true);
    store.dispose();
    finish(entry, writes);
    expect(store.previewStatus).toBe('idle');
    expect(store.display({ row: 0, column: 0 })).toBe('7');
    expect(store.value({ row: 1, column: 0 })).toBeUndefined();
  });
});

const historyResponse = (overrides: Record<string, unknown> = {}) => ({
  schema_version: 1,
  document_id: documentId,
  actor_id: documentId,
  through_version: '1',
  earliest_scanned_version: '1',
  undo: [],
  redo: [],
  truncated: false,
  truncation_reason: null,
  ...overrides,
});
describe('drag autofill', () => {
  it('assistant refuses occupied cells and disabling clears all suggestions', async () => {
    const { store } = await connectedStore();
    const state = projection();
    const sheet = state.sheets[0]!;
    sheet.rows.push(...Array.from({ length: 5 }, () => uuidV7()));
    sheet.cells = [7, 7, 7, 99].map((value, row) => ({
      row_id: sheet.rows[row]!,
      column_id: columnId,
      value: { type: 'number', value },
    }));
    store.load(state);
    for (let row = 0; row < 3; row++)
      store.observeAssistant({ kind: 'edit', position: { row, column: 0 } });
    const tip = store.assistantSuggestion!;
    expect(tip.id).toBe('repeat');
    expect(store.applyAssistantSuggestion(tip, 2)).toBe(false);
    expect(store.error).toContain('пустые');
    expect(store.pending).toHaveLength(0);
    expect(store.confirmed.size).toBe(4);
    store.setAssistantEnabled(false);
    expect(store.assistantSuggestion).toBeUndefined();
    expect(store.applyAssistantSuggestion(tip, 2)).toBe(false);
    store.setAssistantEnabled(true);
    expect(store.assistantSuggestion).toBeUndefined();
    store.dispose();
  });
  it('extends a numeric seed as one batch and never modifies confirmed seed cells', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    const sheet = state.sheets[0]!;
    sheet.rows.push(...Array.from({ length: 8 }, () => uuidV7()));
    sheet.cells = [1, 3].map((value, row) => ({
      row_id: sheet.rows[row]!,
      column_id: columnId,
      value: { type: 'number', value },
    }));
    store.load(state);
    store.select({ row: 1, column: 0 }, true);
    const source = store.selectionBounds;
    expect(store.autofill(source, { row: 5, column: 0 }, '1')).toBe(true);
    expect(store.pending).toHaveLength(1);
    expect(store.confirmed.size).toBe(2);
    expect(store.value({ row: 5, column: 0 })).toEqual({ type: 'number', value: 11 });
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing autofill');
    expect(payload.value.operation?.kind.case).toBe('setCellValues');
    expect(store.autofill(source, { row: 6, column: 0 }, '1')).toBe(false);
    store.dispose();
  });
  it('uses the server formula copy path and rejects stale, filtered or read-only gestures', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7(), uuidV7());
    store.load(state);
    const source = store.selectionBounds;
    expect(store.autofill(source, { row: 2, column: 0 }, '0')).toBe(false);
    expect(store.autofill(source, { row: 2, column: 0 }, '1')).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing autofill');
    expect(payload.value.repeatRange?.destinationRows).toBe(2);
    store.pending = [];
    state.capabilities = { ...state.capabilities!, can_edit: false };
    store.load(state);
    expect(store.autofill(source, { row: 2, column: 0 }, '1')).toBe(false);
    store.dispose();
  });
});

async function connectedStore(discoveredHistory = historyResponse()) {
  vi.useFakeTimers();
  vi.stubGlobal('location', { href: 'http://localhost/' });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/undo-history')
        ? Response.json(discoveredHistory)
        : {
            ok: true,
            json: async () => ({
              ...projection(),
              version: String(discoveredHistory.through_version),
            }),
          },
    ),
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

describe('optimistic axis geometry', () => {
  it('renders pending sizes without changing confirmed content and rolls back a rejection', async () => {
    const { store, sent } = await connectedStore();
    const confirmed = structuredClone(store.projection);
    expect(store.setAxisSize('column', 0, 180)).toBe(true);
    expect(sheetMetrics(store)?.columns.sizeAt(0)).toBe(180);
    expect(sheetMetrics(store)).toBe(sheetMetrics(store));
    expect(store.projection).toEqual(confirmed);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing size command');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 3,
          },
        },
      }),
    );
    expect(sheetMetrics(store)?.columns.sizeAt(0)).toBe(112);
    expect(store.projection).toEqual(confirmed);
    store.dispose();
  });

  it('follows stable IDs, keeps the latest pending size, and previews reset separately', async () => {
    const { store, sent } = await connectedStore();
    expect(store.setAxisSize('row', 0, 42)).toBe(true);
    expect(store.setAxisSize('row', 0, 64)).toBe(true);
    expect(sheetMetrics(store)?.rows.sizeAt(0)).toBe(64);
    store.personalView.settings.filters = [
      {
        id: uuidV7(),
        sheet_id: sheetId,
        first_row_id: rowId,
        last_row_id: rowId,
        first_column_id: columnId,
        last_column_id: columnId,
        header_row_id: null,
        conditions: [{ column_id: columnId, predicate: { kind: 'nonblank' } }],
      },
    ];
    expect(sheetMetrics(store)?.rows.sizeAt(0)).toBe(0);
    expect(store.axisSize('row', 0)).toBe(64);
    store.personalView.settings.filters = [];
    store.sheet!.rows = [uuidV7(), rowId];
    expect(sheetMetrics(store)?.rows.sizeAt(0)).toBe(28);
    expect(sheetMetrics(store)?.rows.sizeAt(1)).toBe(64);
    expect(store.setAxisSize('row', 1, undefined)).toBe(true);
    expect(sheetMetrics(store)?.rows.sizeAt(1)).toBe(28);
    expect(store.sheet!.row_heights).toBeUndefined();
    const before = sent.length;
    const pending = store.pending.length;
    expect(store.setAxisSize('row', 1, 28)).toBe(true);
    expect(sent).toHaveLength(before);
    expect(store.pending).toHaveLength(pending);
    store.sheet!.rows = [uuidV7()];
    expect(sheetMetrics(store)?.rows.totalSize).toBe(28);
    store.dispose();
  });
});

describe('durable history discovery', () => {
  const hint = (version: bigint, kind = 0, actor?: string) =>
    create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'documentHeadChanged',
        value: {
          documentId: uuidBytes(documentId),
          headVersion: version,
          headEventKind: kind,
          headActorId: actor ? uuidBytes(actor) : new Uint8Array(),
        },
      },
    });
  it('rejects malformed, excessive and cross-stack duplicate history references', () => {
    const a = { operation_id: uuidV7(), committed_version: '1' };
    const b = { operation_id: uuidV7(), committed_version: '2' };
    const good = historyResponse({ through_version: '2', undo: [a], redo: [b] });
    expect(decodeHistory(good)).toEqual(good);
    for (const bad of [
      { schema_version: 2 },
      { actor_id: documentId.replace('-7000-', '-4000-') },
      { through_version: '02' },
      { through_version: '9223372036854775808' },
      { earliest_scanned_version: '3' },
      { undo: [b, a] },
      { redo: [a] },
      { redo: [{ ...b, committed_version: '1' }] },
      { undo: [{ ...a, content: 'secret' }] },
      {
        undo: Array.from({ length: 101 }, (_, i) => ({
          operation_id: uuidV7(),
          committed_version: String(i + 1),
        })),
      },
      { truncated: true },
      { truncation_reason: 'unknown' },
      { content: 'secret' },
    ])
      expect(() => decodeHistory({ ...good, ...bad })).toThrow();
    expect(
      decodeHistory(historyResponse({ through_version: '0', earliest_scanned_version: '0' })).undo,
    ).toEqual([]);
  });
  it('caps actual streamed history bytes without trusting Content-Length', async () => {
    expect(await readHistory(Response.json(historyResponse()))).toEqual(historyResponse());
    await expect(readHistory(new Response(' '.repeat(65537)))).rejects.toThrow();
    await expect(
      readHistory(new Response('{}', { headers: { 'content-length': '65537' } })),
    ).rejects.toThrow();
  });
  it('loads persisted own history on connection and submits only its target identity', async () => {
    const original = uuidV7();
    const { store, sent } = await connectedStore(
      historyResponse({
        undo: [{ operation_id: original, committed_version: '1' }],
      }),
    );
    expect(store.canUndo).toBe(true);
    expect(store.canRedo).toBe(false);
    expect(store.undo()).toBe(true);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing inverse command');
    expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(original);
    expect(command.value.operation).toBeUndefined();
    store.dispose();
  });
  it('restores redo without inventing an ordinary edit', async () => {
    const compensation = uuidV7();
    const { store, sent } = await connectedStore(
      historyResponse({
        redo: [{ operation_id: compensation, committed_version: '1' }],
      }),
    );
    expect(store.canRedo).toBe(true);
    expect(store.redo()).toBe(true);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing redo command');
    expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(compensation);
    store.dispose();
  });
  it('discovers other-tab own heads but never scans for foreign or system heads', async () => {
    const { store, sent } = await connectedStore();
    const original = uuidV7();
    let version = '2';
    const fetcher = vi.fn(async (url: string) =>
      url.endsWith('/undo-history')
        ? Response.json(
            historyResponse({
              through_version: version,
              undo: [{ operation_id: original, committed_version: '2' }],
            }),
          )
        : Response.json({ ...projection(), version }),
    );
    vi.stubGlobal('fetch', fetcher);
    await store.receive(hint(2n, 1, documentId));
    expect(store.canUndo).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.canUndo).toBe(true);
    expect(fetcher.mock.calls.filter(([url]) => url.endsWith('/undo-history'))).toHaveLength(1);
    version = '3';
    await store.receive(hint(3n, 1, uuidV7()));
    version = '4';
    await store.receive(hint(4n, 2));
    await vi.advanceTimersByTimeAsync(0);
    expect(fetcher.mock.calls.filter(([url]) => url.endsWith('/undo-history'))).toHaveLength(1);
    expect(store.undo()).toBe(true);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing inverse');
    expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(original);
    store.dispose();
  });
  for (const kind of [1, 2]) {
    for (const projectionAhead of [false, true]) {
      it(`discovers skipped own history before a ${kind === 1 ? 'foreign' : 'system'} head even when projection ahead=${projectionAhead}`, async () => {
        const older = uuidV7(),
          missed = uuidV7();
        const { store, sent } = await connectedStore(
          historyResponse({
            through_version: '9',
            undo: [{ operation_id: older, committed_version: '9' }],
          }),
        );
        let historyCalls = 0;
        vi.stubGlobal(
          'fetch',
          vi.fn(async (url: string) => {
            if (!url.endsWith('/undo-history'))
              return Response.json({ ...projection(), version: '11' });
            historyCalls++;
            return Response.json(
              historyResponse({
                through_version: '11',
                undo: [
                  { operation_id: older, committed_version: '9' },
                  { operation_id: missed, committed_version: '10' },
                ],
              }),
            );
          }),
        );
        if (projectionAhead) store.load({ ...projection(), version: '11' });
        await store.receive(hint(11n, kind, kind === 1 ? uuidV7() : undefined));
        expect(store.canUndo).toBe(false);
        await vi.advanceTimersByTimeAsync(0);
        expect(historyCalls).toBe(1);
        expect(store.undo()).toBe(true);
        const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
        if (command.case !== 'command') throw new Error('Missing inverse');
        expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(missed);
        store.dispose();
      });
    }
  }
  it('discovers an unseen own event skipped by a later local receipt instead of treating projection refresh as history coverage', async () => {
    const missed = uuidV7();
    const { store, sent } = await connectedStore(historyResponse({ through_version: '9' }));
    store.set({ row: 0, column: 0 }, { type: 'number', value: 7 });
    const own = store.pending[0]!;
    let historyCalls = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (!url.endsWith('/undo-history'))
          return Response.json({ ...projection(), version: '11' });
        historyCalls++;
        return Response.json(
          historyResponse({
            through_version: '11',
            undo: [
              { operation_id: missed, committed_version: '10' },
              { operation_id: own.id, committed_version: '11' },
            ],
          }),
        );
      }),
    );
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'receipt',
          value: {
            documentId: uuidBytes(documentId),
            operationId: uuidBytes(own.id),
            committedVersion: 11n,
            currentHeadVersion: 11n,
          },
        },
      }),
    );
    expect(store.canUndo).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    expect(historyCalls).toBe(1);
    expect(store.undo()).toBe(true);
    await acknowledgeCellResult(store, sent, undefined);
    expect(store.undo()).toBe(true);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing earlier inverse');
    expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(missed);
    store.dispose();
  });
  it('does not rescan immediate foreign and system successors of a known local ACK', async () => {
    const { store, sent } = await connectedStore(historyResponse({ through_version: '9' }));
    store.set({ row: 0, column: 0 }, { type: 'number', value: 7 });
    await acknowledgeCellResult(store, sent, 7);
    let version = '11';
    const fetcher = vi.fn(async (url: string) =>
      url.endsWith('/undo-history')
        ? Response.json(historyResponse({ through_version: version }))
        : Response.json({ ...projection(), version }),
    );
    vi.stubGlobal('fetch', fetcher);
    await store.receive(hint(11n, 1, uuidV7()));
    version = '12';
    await store.receive(hint(12n, 2));
    await vi.advanceTimersByTimeAsync(0);
    expect(fetcher.mock.calls.filter(([url]) => url.endsWith('/undo-history'))).toHaveLength(0);
    expect(store.canUndo).toBe(true);
    store.dispose();
  });
  it('prunes expired targets on foreign heads and rejects unknown head classifications', async () => {
    const { store } = await connectedStore(
      historyResponse({ undo: [{ operation_id: uuidV7(), committed_version: '1' }] }),
    );
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => Response.json({ ...projection(), version: '10002' })),
    );
    await store.receive(hint(10002n, 1, uuidV7()));
    expect(store.canUndo).toBe(false);
    await expect(store.receive(hint(10003n, 99))).rejects.toThrow();
    await expect(store.receive(hint(10003n, 1))).rejects.toThrow();
    await expect(store.receive(hint(10003n, 2, documentId))).rejects.toThrow();
    store.dispose();
  });
  it('does not repeatedly request denied history on foreign or system heads while confirmed reading continues', async () => {
    const { store } = await connectedStore();
    let version = '2',
      historyCalls = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (!url.endsWith('/undo-history')) return Response.json({ ...projection(), version });
        historyCalls++;
        return new Response(null, { status: 403 });
      }),
    );
    await store.receive(hint(2n));
    await vi.advanceTimersByTimeAsync(0);
    expect(historyCalls).toBe(1);
    version = '3';
    await store.receive(hint(3n, 1, uuidV7()));
    version = '4';
    await store.receive(hint(4n, 2));
    await vi.advanceTimersByTimeAsync(30000);
    expect(store.projection?.version).toBe('4');
    expect(store.canUndo).toBe(false);
    expect(historyCalls).toBe(1);
    store.dispose();
  });
  it('does not let a delayed discovery replace a newer acknowledged edit, and editing never waits for history', async () => {
    const { store, sent } = await connectedStore();
    let resolve!: (response: Response) => void;
    const oldHistory = new Promise<Response>((done) => {
      resolve = done;
    });
    let historyCalls = 0;
    let newest = '';
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (!url.endsWith('/undo-history')) return Response.json({ ...projection(), version: '2' });
        historyCalls++;
        return historyCalls === 1
          ? oldHistory
          : Response.json(
              historyResponse({
                through_version: '3',
                undo: [{ operation_id: newest, committed_version: '3' }],
              }),
            );
      }),
    );
    await store.receive(hint(2n));
    await vi.advanceTimersByTimeAsync(0);
    expect(historyCalls).toBe(1);
    store.begin('7');
    store.commit();
    expect(store.pending).toHaveLength(1);
    expect(sent.map((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case)).toContain(
      'command',
    );
    newest = store.pending[0]!.id;
    await acknowledgeCellResult(store, sent, 7);
    resolve(
      Response.json(
        historyResponse({
          through_version: '2',
          redo: [{ operation_id: uuidV7(), committed_version: '2' }],
        }),
      ),
    );
    await vi.advanceTimersByTimeAsync(1);
    expect(store.canUndo).toBe(true);
    expect(store.canRedo).toBe(false);
    expect(store.undo()).toBe(true);
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (command.case !== 'command') throw new Error('Missing inverse');
    expect(bytesUuid(command.value.undoCell!.targetOperationId)).toBe(newest);
    store.dispose();
  });
  it('resolves uncertain reconnect commands by their original bytes before installing history, then ignores late ACK', async () => {
    const { store, sent } = await connectedStore();
    store.begin('7');
    store.commit();
    const original = store.pending[0]!;
    store.disconnected();
    let historyCalls = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (!url.endsWith('/undo-history')) return Response.json({ ...projection(), version: '2' });
        historyCalls++;
        return Response.json(
          historyResponse({
            through_version: '2',
            undo: [{ operation_id: original.id, committed_version: '2' }],
          }),
        );
      }),
    );
    await store.connected(uuidBytes(documentId), (bytes) => {
      sent.push(bytes);
      return true;
    });
    expect(historyCalls).toBe(0);
    expect(sent.at(-1)).toEqual(original.bytes);
    const receipt = create(ServerEnvelopeSchema, {
      protocolVersion: 1,
      payload: {
        case: 'receipt',
        value: {
          documentId: uuidBytes(documentId),
          operationId: uuidBytes(original.id),
          committedVersion: 2n,
          currentHeadVersion: 2n,
        },
      },
    });
    await store.receive(receipt);
    await vi.advanceTimersByTimeAsync(0);
    expect(historyCalls).toBe(1);
    expect(store.canUndo).toBe(true);
    await store.receive(receipt);
    expect(store.undo()).toBe(true);
    await acknowledgeCellResult(store, sent, undefined);
    expect(store.canUndo).toBe(false);
    expect(store.canRedo).toBe(true);
    store.dispose();
  });
  it('never installs history for another principal or document and keeps normal edits available', async () => {
    for (const bad of [{ actor_id: uuidV7() }, { document_id: uuidV7() }]) {
      const { store } = await connectedStore(
        historyResponse({ ...bad, undo: [{ operation_id: uuidV7(), committed_version: '1' }] }),
      );
      expect(store.canUndo).toBe(false);
      expect(store.error).toContain('другой таблице');
      store.begin('8');
      store.commit();
      expect(store.pending).toHaveLength(1);
      store.dispose();
    }
  });
  it('ignores a stale denied response after switching document and still discovers the new document', async () => {
    const { store } = await connectedStore();
    let resolve!: (response: Response) => void;
    const first = new Promise<Response>((done) => {
      resolve = done;
    });
    const otherDocument = uuidV7(),
      original = uuidV7();
    let nextCalls = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (url.endsWith('/undo-history')) {
          if (!url.includes(otherDocument)) return first;
          nextCalls++;
          return Response.json(
            historyResponse({
              document_id: otherDocument,
              undo: [{ operation_id: original, committed_version: '1' }],
            }),
          );
        }
        return Response.json({ ...projection(), version: '2' });
      }),
    );
    await store.receive(hint(2n));
    await vi.advanceTimersByTimeAsync(0);
    store.load({ ...projection(), document_id: otherDocument });
    resolve(new Response(null, { status: 403 }));
    await vi.advanceTimersByTimeAsync(1);
    expect(nextCalls).toBe(1);
    expect(store.canUndo).toBe(true);
    expect(store.error).toBe('');
    store.dispose();
  });
  it('quarantines an uncertain old actor and restores only its original bytes on A to B to A', async () => {
    const { store, sent } = await connectedStore();
    store.begin('7');
    store.commit();
    const a = store.pending[0]!;
    const bActor = uuidV7();
    let selectedActor = bActor;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) =>
        url.endsWith('/undo-history')
          ? Response.json(historyResponse({ actor_id: selectedActor }))
          : Response.json(projection()),
      ),
    );
    sent.length = 0;
    await store.connected(uuidBytes(bActor), (bytes) => {
      sent.push(bytes);
      return true;
    });
    expect(store.pending).toHaveLength(0);
    expect(
      sent.every((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case !== 'command'),
    ).toBe(true);
    store.begin('9');
    store.commit();
    const b = store.pending[0]!;
    selectedActor = documentId;
    await store.connected(uuidBytes(documentId), (bytes) => {
      sent.push(bytes);
      return true;
    });
    expect(store.pending[0]?.id).toBe(a.id);
    expect(sent.at(-1)).toEqual(a.bytes);
    selectedActor = bActor;
    await store.connected(uuidBytes(bActor), (bytes) => {
      sent.push(bytes);
      return true;
    });
    expect(store.pending[0]?.id).toBe(b.id);
    expect(sent.at(-1)).toEqual(b.bytes);
    store.dispose();
  });
  it('refuses a third actor rather than overwriting an unresolved identity quarantine', async () => {
    const { store, sent } = await connectedStore();
    store.begin('7');
    store.commit();
    const bActor = uuidV7();
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) =>
        url.endsWith('/undo-history')
          ? Response.json(historyResponse({ actor_id: bActor }))
          : Response.json(projection()),
      ),
    );
    await store.connected(uuidBytes(bActor), (bytes) => {
      sent.push(bytes);
      return true;
    });
    store.begin('9');
    store.commit();
    const b = store.pending[0]!;
    const thirdSender = vi.fn(() => true);
    expect(await store.connected(uuidBytes(uuidV7()), thirdSender)).toBe(false);
    expect(thirdSender).not.toHaveBeenCalled();
    expect(store.pending[0]?.bytes).toEqual(b.bytes);
    expect(store.error).toContain('Смена аккаунта приостановлена');
    store.dispose();
  });
  it('counts quarantined intents against every active enqueue budget', async () => {
    const { store } = await connectedStore();
    for (let i = 0; i < 128; i++)
      expect(store.set({ row: 0, column: 0 }, { type: 'number', value: i })).toBe(true);
    const actor = uuidV7();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    state.sheets[0]!.columns.push(uuidV7());
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) =>
        url.endsWith('/undo-history')
          ? Response.json(
              historyResponse({
                actor_id: actor,
                undo: [{ operation_id: uuidV7(), committed_version: '1' }],
              }),
            )
          : Response.json(state),
      ),
    );
    await store.connected(uuidBytes(actor), () => true);
    store.load(state);
    expect(store.pending).toHaveLength(0);
    expect(store.set({ row: 0, column: 0 }, { type: 'number', value: 1000 })).toBe(false);
    expect(store.pasteTsv('1')).toBe(false);
    expect(store.insertRowBeforeSelection()).toBe(false);
    expect(store.insertColumnBeforeSelection()).toBe(false);
    expect(store.deleteSelectedRow()).toBe(false);
    expect(store.deleteSelectedColumn()).toBe(false);
    expect(store.extendRows()).toBe(false);
    expect(store.addSheet()).toBe(false);
    expect(store.cutRangePayload()).toBeUndefined();
    const rich = JSON.stringify({
      schema_version: 1,
      document_id: documentId,
      version: '1',
      sheet_id: sheetId,
      row_id: rowId,
      column_id: columnId,
      rows: 1,
      columns: 2,
    });
    expect(store.pasteRangeCopy(rich)).toBe(false);
    expect(store.canUndo).toBe(false);
    expect(store.undo()).toBe(false);
    expect(store.pending).toHaveLength(0);
    store.dispose();
  });
});
async function acknowledgeCellResult(
  store: DocumentStore,
  sent: Uint8Array[],
  value: number | undefined,
) {
  const commandBytes = [...sent]
    .reverse()
    .find((bytes) => fromBinary(ClientEnvelopeSchema, bytes).payload.case === 'command');
  const command = fromBinary(ClientEnvelopeSchema, commandBytes ?? new Uint8Array()).payload;
  if (command.case !== 'command') throw new Error('Missing pending command');
  const address = {
    sheetId: uuidBytes(sheetId),
    rowId: uuidBytes(rowId),
    columnId: uuidBytes(columnId),
  };
  const message = create(ServerEnvelopeSchema, {
    protocolVersion: 1,
    payload: {
      case: 'committed',
      value: {
        documentId: uuidBytes(documentId),
        operationId: command.value.operationId,
        committedVersion: BigInt(store.projection?.version ?? '0') + 1n,
        canonicalOperation:
          value === undefined
            ? { kind: { case: 'clearCell', value: { address } } }
            : {
                kind: {
                  case: 'setCellValue',
                  value: { address, value: { kind: { case: 'number', value } } },
                },
              },
        confirmedStateHash: new Uint8Array(32),
      },
    },
  });
  await store.receive(message);
  return { command: command.value, message };
}

describe('document projection and pending intents', () => {
  it('bounds acknowledged history to100 and traverses undo redo without optimistic inverse', async () => {
    const { store, sent } = await connectedStore();
    for (let value = 0; value <= 100; value++) {
      expect(store.set({ row: 0, column: 0 }, { type: 'number', value })).toBe(true);
      await acknowledgeCellResult(store, sent, value);
    }
    for (let value = 99; value >= 0; value--) {
      expect(store.undo()).toBe(true);
      expect(store.display({ row: 0, column: 0 })).toBe(String(value + 1));
      await acknowledgeCellResult(store, sent, value);
    }
    expect(store.canUndo).toBe(false);
    expect(store.canRedo).toBe(true);
    for (let value = 1; value <= 100; value++) {
      expect(store.redo()).toBe(true);
      await acknowledgeCellResult(store, sent, value);
    }
    expect(store.canRedo).toBe(false);
    expect(store.display({ row: 0, column: 0 })).toBe('100');
    store.disconnected();
  });
  it('new acknowledged edits branch history and failed inverse drops only its top', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 1 });
    await acknowledgeCellResult(store, sent, 1);
    store.set({ row: 0, column: 0 }, { type: 'number', value: 2 });
    await acknowledgeCellResult(store, sent, 2);
    store.undo();
    await acknowledgeCellResult(store, sent, 1);
    expect(store.canRedo).toBe(true);
    store.set({ row: 0, column: 0 }, { type: 'number', value: 3 });
    await acknowledgeCellResult(store, sent, 3);
    expect(store.canRedo).toBe(false);
    store.undo();
    const command = fromBinary(ClientEnvelopeSchema, sent.at(-1) ?? new Uint8Array()).payload;
    if (command.case !== 'command') throw new Error('Missing inverse');
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'rejected',
          value: {
            documentId: uuidBytes(documentId),
            operationId: command.value.operationId,
            code: 13,
            retryable: false,
          },
        },
      }),
    );
    expect(store.canUndo).toBe(true);
    expect(store.display({ row: 0, column: 0 })).toBe('3');
    store.disconnected();
  });
  it('duplicate inverse receipts never push the same history entry twice', async () => {
    const { store, sent } = await connectedStore();
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => structuredClone(store.projection) })),
    );
    store.set({ row: 0, column: 0 }, { type: 'number', value: 1 });
    await acknowledgeCellResult(store, sent, 1);
    store.undo();
    const undone = await acknowledgeCellResult(store, sent, undefined);
    const duplicate = async (operationId: Uint8Array) =>
      store.receive(
        create(ServerEnvelopeSchema, {
          protocolVersion: 1,
          payload: {
            case: 'receipt',
            value: {
              documentId: uuidBytes(documentId),
              operationId,
              committedVersion: BigInt(store.projection?.version ?? '0'),
              currentHeadVersion: BigInt(store.projection?.version ?? '0'),
              canonicalEventHash: new Uint8Array(32),
            },
          },
        }),
      );
    await duplicate(undone.command.operationId);
    expect(store.redo()).toBe(true);
    const redone = await acknowledgeCellResult(store, sent, 1);
    await duplicate(redone.command.operationId);
    expect(store.canRedo).toBe(false);
    expect(store.undo()).toBe(true);
    await acknowledgeCellResult(store, sent, undefined);
    expect(store.canUndo).toBe(false);
    store.disconnected();
  });
  it('overlapping receipt refresh and canonical ACK consume authored history once', async () => {
    const { store, sent } = await connectedStore();
    store.set({ row: 0, column: 0 }, { type: 'number', value: 1 });
    const original = fromBinary(ClientEnvelopeSchema, sent.at(-1) ?? new Uint8Array()).payload;
    if (original.case !== 'command') throw new Error('Missing authored command');
    let release: (() => void) | undefined;
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        json: async () => {
          await new Promise<void>((resolve) => {
            release = resolve;
          });
          return structuredClone(store.projection);
        },
      })),
    );
    const receipt = store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'receipt',
          value: {
            documentId: uuidBytes(documentId),
            operationId: original.value.operationId,
            committedVersion: 2n,
            currentHeadVersion: 2n,
            canonicalEventHash: new Uint8Array(32),
          },
        },
      }),
    );
    await vi.waitFor(() => expect(release).toBeTypeOf('function'));
    await acknowledgeCellResult(store, sent, 1);
    release?.();
    await receipt;
    expect(store.undo()).toBe(true);
    await acknowledgeCellResult(store, sent, undefined);
    expect(store.canUndo).toBe(false);
    store.disconnected();
  });
  it('retires a committed recalculation ID when a concurrent refresh supersedes hydration', async () => {
    const { store } = await connectedStore();
    const operationIds: string[] = [];
    let release: (() => void) | undefined;
    let firstGet = true;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, options?: RequestInit) => {
        if (options?.method === 'POST') {
          const body = JSON.parse(String(options.body)) as { operation_id: string };
          operationIds.push(body.operation_id);
          return {
            ok: true,
            json: async () => ({ operation_id: body.operation_id, committed_version: '2' }),
          };
        }
        if (firstGet) {
          firstGet = false;
          await new Promise<void>((resolve) => {
            release = resolve;
          });
        }
        return {
          ok: true,
          json: async () => ({
            ...projection(),
            version: '2',
            calculation_context: previewContext(uuidV7(), '1'),
          }),
        };
      }),
    );
    const initial = store.recalculate();
    await vi.waitFor(() => expect(release).toBeTypeOf('function'));
    await store.refresh(2n);
    release?.();
    await initial;
    await store.recalculate();
    expect(operationIds).toHaveLength(2);
    expect(operationIds[1]).not.toBe(operationIds[0]);
    store.dispose();
  });
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
          calculation_context: previewContext(uuidV7(), '1'),
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
    expect(store.canRedo).toBe(true);
    expect(store.redo()).toBe(true);
    expect(store.display({ row: 0, column: 0 })).toBe('');
    const redo = fromBinary(ClientEnvelopeSchema, sent[2] ?? new Uint8Array()).payload;
    if (redo.case !== 'command' || !redo.value.undoCell) throw new Error('Missing redo inverse');
    expect(redo.value.undoCell.targetOperationId).toEqual(command.value.operationId);
    expect(redo.value.operationId).not.toEqual(command.value.operationId);
    await store.receive(
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'committed',
          value: {
            documentId: uuidBytes(documentId),
            operationId: redo.value.operationId,
            committedVersion: 4n,
            canonicalOperation: original.value.operation,
            confirmedStateHash: new Uint8Array(32),
          },
        },
      }),
    );
    expect(store.display({ row: 0, column: 0 })).toBe('42');
    expect(store.canUndo).toBe(true);
    expect(store.canRedo).toBe(false);
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
    expect(store.canUndo).toBe(true);
    store.undo();
    expect(store.pending.at(-1)?.kind).toBe('undo');
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
  it('repeats quoted TSV cells after parsing without splitting embedded tabs or newlines', async () => {
    const { store } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    state.sheets[0]!.columns.push(uuidV7());
    store.load(state);
    store.select({ row: 0, column: 0 });
    store.select({ row: 1, column: 1 }, true);
    expect(store.pasteTsv('"hello\tworld\nnext"')).toBe(true);
    expect(store.pending).toHaveLength(1);
    for (let row = 0; row < 2; row++)
      for (let column = 0; column < 2; column++)
        expect(store.value({ row, column })).toEqual({ type: 'text', value: 'hello\tworld\nnext' });
    expect(store.projection?.sheets[0]?.cells).toHaveLength(0);
    store.dispose();
  });
  it('retains literal Markdown text and repeats its column alignment across the selection', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    state.sheets[0]!.columns.push(uuidV7());
    store.load(state);
    store.select({ row: 0, column: 0 });
    store.select({ row: 1, column: 1 }, true);
    expect(store.pasteMatrix([['=A1']], undefined, true, ['center'])).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing Markdown batch');
    expect(payload.value.setCellBatch?.writes).toHaveLength(4);
    expect(payload.value.setCellBatch?.styles).toHaveLength(4);
    for (let row = 0; row < 2; row++)
      for (let column = 0; column < 2; column++) {
        expect(store.value({ row, column })).toEqual({ type: 'text', value: '=A1' });
        expect(store.style({ row, column }).horizontal).toBe('center');
      }
    store.dispose();
  });
  it('fills a reversed selected rectangle with one pasted value', async () => {
    const { store } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7(), uuidV7());
    state.sheets[0]!.columns.push(uuidV7());
    store.load(state);
    store.select({ row: 2, column: 1 });
    store.select({ row: 0, column: 0 }, true);
    expect(store.pasteTsv('7')).toBe(true);
    expect(store.pending).toHaveLength(1);
    for (let row = 0; row < 3; row++)
      for (let column = 0; column < 2; column++)
        expect(store.value({ row, column })).toEqual({ type: 'number', value: 7 });
    store.dispose();
  });
  it('fills down from the top row as one exact-head command without changing confirmed data', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7(), uuidV7());
    state.sheets[0]!.columns.push(uuidV7());
    store.load(state);
    expect(store.fillDown()).toBe(false);
    store.select({ row: 2, column: 1 });
    store.select({ row: 0, column: 0 }, true);
    expect(store.fillDown()).toBe(true);
    expect(store.pending).toHaveLength(1);
    expect(store.projection?.version).toBe('1');
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing fill command');
    expect(payload.value.copyRange).toBeUndefined();
    expect(payload.value.repeatRange).toMatchObject({
      destinationRows: 2,
      destinationColumns: 2,
      sourceBlock: { rows: 1, columns: 2 },
    });
    expect(bytesUuid(payload.value.repeatRange!.sourceBlock!.source!.rowId)).toBe(rowId);
    expect(bytesUuid(payload.value.repeatRange!.sourceBlock!.destination!.rowId)).toBe(
      state.sheets[0]!.rows[1],
    );
    expect(store.fillDown()).toBe(false);
    store.dispose();
  });
  it('repeats a copied formula through the server and preserves the clipboard version guard', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7(), uuidV7());
    state.sheets[0]!.cells.push({
      row_id: rowId,
      column_id: columnId,
      value: { type: 'formula', value: { source: '=B1+$B$1', current_text: '=B1+$B$1' } },
    });
    store.load(state);
    const raw = store.copyFormulaPayload();
    store.select({ row: 1, column: 0 });
    store.select({ row: 2, column: 0 }, true);
    expect(await store.pasteFormulaCopy(raw!)).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing formula fill');
    expect(payload.value.repeatRange?.destinationRows).toBe(2);
    expect(payload.value.repeatRange?.sourceBlock?.rows).toBe(1);
    state.version = '2';
    store.load(state);
    expect(await store.pasteFormulaCopy(raw!)).toBe(false);
    store.dispose();
  });
  it('fills a single selected cell from the row above', async () => {
    const { store, sent } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    store.load(state);
    store.select({ row: 1, column: 0 });
    expect(store.fillDown()).toBe(true);
    const payload = fromBinary(ClientEnvelopeSchema, sent.at(-1)!).payload;
    if (payload.case !== 'command') throw new Error('Missing single-cell fill');
    expect(payload.value.repeatRange).toMatchObject({
      destinationRows: 1,
      destinationColumns: 1,
      sourceBlock: { rows: 1, columns: 1 },
    });
    expect(bytesUuid(payload.value.repeatRange!.sourceBlock!.source!.rowId)).toBe(rowId);
    expect(bytesUuid(payload.value.repeatRange!.sourceBlock!.destination!.rowId)).toBe(
      state.sheets[0]!.rows[1],
    );
    store.dispose();
  });
  it('tiles a pasted row and refuses incompatible selection atomically', async () => {
    const { store } = await connectedStore();
    const state = projection();
    state.sheets[0]!.rows.push(uuidV7());
    state.sheets[0]!.columns.push(uuidV7(), uuidV7(), uuidV7());
    store.load(state);
    store.select({ row: 1, column: 3 }, true);
    expect(store.pasteTsv('1\t2')).toBe(true);
    expect(store.value({ row: 1, column: 2 })).toEqual({ type: 'number', value: 1 });
    expect(store.value({ row: 1, column: 3 })).toEqual({ type: 'number', value: 2 });
    store.dispose();
    const second = await connectedStore();
    second.store.load(state);
    second.store.select({ row: 1, column: 2 }, true);
    expect(second.store.pasteTsv('1\t2')).toBe(false);
    expect(second.store.pending).toHaveLength(0);
    second.store.dispose();
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
  it('an all-hidden personal filter cannot edit a hidden cell or grow the shared book', async () => {
    const { store, sent } = await connectedStore();
    const sheet = store.sheet!;
    const before = store.projection?.version;
    store.personalView = {
      schema_version: 1,
      revision: 1,
      settings: {
        filters: [
          {
            id: uuidV7(),
            sheet_id: sheet.id,
            first_row_id: sheet.rows[0]!,
            last_row_id: sheet.rows[0]!,
            first_column_id: sheet.columns[0]!,
            last_column_id: sheet.columns[0]!,
            header_row_id: null,
            conditions: [
              {
                column_id: sheet.columns[0]!,
                predicate: { kind: 'number', operator: 'eq', value: 10 },
              },
            ],
          },
        ],
      },
    };
    expect(store.filterState.hidden.size).toBe(1);
    expect(store.allRowsHidden).toBe(true);
    expect(store.canEdit).toBe(false);
    expect(store.canCopy).toBe(false);
    expect(store.extendRows()).toBe(false);
    store.select({ row: 0, column: 25 });
    expect(store.selected).toEqual({ row: 0, column: 0 });
    store.begin();
    expect(store.editing).toBeUndefined();
    expect(store.pending).toHaveLength(0);
    expect(sent).toHaveLength(0);
    expect(store.projection?.version).toBe(before);
    sheet.rows.push(uuidV7());
    expect(store.allRowsHidden).toBe(false);
    expect(store.canEdit).toBe(true);
    expect(store.extendRows()).toBe(false);
    expect(sent).toHaveLength(0);
    store.personalView = { schema_version: 1, revision: 2, settings: { filters: [] } };
    expect(store.allRowsHidden).toBe(false);
    expect(store.canEdit).toBe(true);
    expect(store.canCopy).toBe(true);
    store.dispose();
  });
  it('sort freezes an explicit range and keys without optimistic content or selection changes', async () => {
    const { store, sent } = await connectedStore();
    const before = store.projection;
    const bounds = { startRow: 0, endRow: 0, startColumn: 0, endColumn: 0 };
    expect(store.sortRange(bounds, 0, [{ column: 0, descending: false }], '2')).toBe(false);
    expect(store.sortRange(bounds, 0, [{ column: 0, descending: true }], '1')).toBe(true);
    expect(store.projection).toBe(before);
    expect(store.confirmed.size).toBe(0);
    expect(store.selected).toEqual({ row: 0, column: 0 });
    expect(store.pending).toHaveLength(1);
    const payload = fromBinary(ClientEnvelopeSchema, sent[0]!).payload;
    if (payload.case !== 'command') throw new Error('Missing sort command');
    expect(payload.value.operation).toBeUndefined();
    expect(payload.value.sortRange).toMatchObject({
      schemaVersion: 1,
      rows: 1,
      columns: 1,
      headerRows: 0,
      keys: [{ descending: true }],
    });
    expect(store.sortRange(bounds, 0, [{ column: 0, descending: false }], '1')).toBe(false);
    store.dispose();
  });
  it('sort reads live confirmed cells after a websocket write and waits for formula results', async () => {
    const { store, sent } = await connectedStore();
    const sheet = store.sheet!;
    expect(sheet.cells).toEqual([]);
    store.confirmed.set(`${sheet.rows[0]}/${sheet.columns[0]}`, {
      type: 'formula',
      value: { source: '=1' },
    });
    expect(store.confirmedCells(sheet)).toHaveLength(1);
    const bounds = { startRow: 0, endRow: 0, startColumn: 0, endColumn: 0 };
    expect(store.sortRange(bounds, 0, [{ column: 0, descending: false }], '1')).toBe(false);
    expect(sent).toHaveLength(0);
    store.calculatedVersion = '1';
    expect(store.sortRange(bounds, 0, [{ column: 0, descending: false }], '1')).toBe(true);
    store.dispose();
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
    expect(store.canUndo).toBe(false);
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
      vi.fn(async (url: string) => {
        if (url.endsWith('/undo-history'))
          return Response.json(historyResponse({ through_version: '2' }));
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
  it('isolates new-book creation from old edits and retains its identity after a failed attempt', async () => {
    const { store } = await connectedStore();
    const assign = vi.fn();
    vi.stubGlobal('location', { href: `http://localhost/?document=${documentId}`, assign });
    let finish: ((response: unknown) => void) | undefined;
    const fetch = vi.fn(
      (_url: string, _init: RequestInit) =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    vi.stubGlobal('fetch', fetch);
    const first = store.createDocument();
    expect(store.canEdit).toBe(false);
    expect(store.set({ row: 0, column: 0 }, { type: 'number', value: 9 })).toBe(false);
    expect(store.pending).toHaveLength(0);
    finish?.({ ok: false });
    await first;
    expect(store.canEdit).toBe(true);
    expect(store.projection?.document_id).toBe(documentId);
    const second = store.createDocument();
    finish?.({ ok: true });
    await second;
    expect(fetch.mock.calls[0]?.[0]).toBe('/api/documents');
    expect(fetch.mock.calls[1]?.[0]).toBe('/api/documents');
    expect(fetch.mock.calls[0]?.[1].body).toEqual(fetch.mock.calls[1]?.[1].body);
    expect(assign).toHaveBeenCalledOnce();
    const url = new URL(String(assign.mock.calls[0]?.[0]));
    expect(url.searchParams.get('document')).not.toBe(documentId);
    expect(store.projection).toBeUndefined();
    store.dispose();
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
it('validates schema-six merged ranges and rejects overlap or hidden covered cells', () => {
  const state = projection();
  const sheet = state.sheets[0]!;
  const secondRow = uuidV7(),
    secondColumn = uuidV7();
  sheet.rows.push(secondRow);
  sheet.columns.push(secondColumn);
  const merged = {
    ...state,
    authored_projection_schema_version: 6 as const,
    style_schema_version: 2 as const,
    style_profile_version: 2 as const,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
    sheets: [
      {
        ...sheet,
        styles: [],
        row_heights: [],
        column_widths: [],
        merges: [
          {
            first_row_id: sheet.rows[0]!,
            last_row_id: secondRow,
            first_column_id: sheet.columns[0]!,
            last_column_id: secondColumn,
          },
        ],
      },
    ],
  };
  expect(decodeProjection(structuredClone(merged)).sheets[0]?.merges).toHaveLength(1);
  const overlap = structuredClone(merged);
  overlap.sheets[0]!.merges!.push({ ...overlap.sheets[0]!.merges![0]! });
  expect(() => decodeProjection(overlap)).toThrow();
  const hidden = structuredClone(merged);
  hidden.sheets[0]!.cells.push({
    row_id: secondRow,
    column_id: sheet.columns[0]!,
    value: { type: 'text', value: 'hidden' },
  });
  expect(() => decodeProjection(hidden)).toThrow();
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
  options: {
    sequence?: number;
    final?: boolean;
    cells?: boolean;
    error?: number;
    fullSnapshot?: boolean;
    baseInputVersion?: bigint;
    entries?: Array<{ rowId: string; value?: number }>;
    evaluatedCells?: number;
    dirtyCells?: number;
  } = {},
) {
  const final = options.final ?? true;
  const entries = options.entries ?? [{ rowId, value }];
  return create(ServerEnvelopeSchema, {
    protocolVersion: 1,
    payload: {
      case: 'calculationCommitted',
      value: {
        documentId: uuidBytes(documentId),
        inputVersion: BigInt(version),
        calculationEpoch: 0n,
        formulaEngineVersion: 'pxel-formula-0.16.0',
        compatibilityProfile: 'pxel-excel-subset-v1',
        contextId: uuidBytes(documentId),
        patchSequence: options.sequence ?? 0,
        isFinal: final,
        calculatedStateHash:
          final && !options.error ? new Uint8Array(32).fill(1) : new Uint8Array(),
        cells:
          options.cells === false
            ? []
            : entries.map((entry) => ({
                address: {
                  sheetId: uuidBytes(sheetId),
                  rowId: uuidBytes(entry.rowId),
                  columnId: uuidBytes(columnId),
                },
                ...(entry.value === undefined
                  ? {}
                  : { value: { kind: { case: 'number' as const, value: entry.value } } }),
              })),
        fullSnapshot: options.fullSnapshot ?? true,
        baseInputVersion: options.baseInputVersion,
        evaluatedCells: options.evaluatedCells ?? (options.error ? 0 : entries.length),
        dirtyCells: options.dirtyCells ?? (options.error ? 0 : entries.length),
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
    expect(store.display(cellPosition)).toBe('');

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
    expect(store.canUndo).toBe(true);
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
    expect(store.display(cellPosition)).toBe('');
    await store.receive(calculation('3', 33));
    expect(store.calculatedVersion).toBe('3');
    expect(store.display(cellPosition)).toBe('33');
    expect(fetcher).not.toHaveBeenCalled();
    store.disconnected();
  });
  it('requests a matching calculation delta, preserves untouched results, and clears removed formulas', async () => {
    const { store, sent } = await connectedStore();
    const secondRow = uuidV7();
    const twoFormulas = formulaProjection('2');
    twoFormulas.sheets[0]!.rows.push(secondRow);
    twoFormulas.sheets[0]!.cells.push({
      ...structuredClone(twoFormulas.sheets[0]!.cells[0]!),
      row_id: secondRow,
    });
    store.load(twoFormulas);
    await store.receive(
      calculation('2', 3, {
        entries: [
          { rowId, value: 3 },
          { rowId: secondRow, value: 4 },
        ],
      }),
    );
    expect(store.calculated.size).toBe(2);

    const versionThree = structuredClone(twoFormulas);
    versionThree.version = '3';
    store.load(versionThree);
    const deltaRequest = fromBinary(ClientEnvelopeSchema, sent.at(-1) ?? new Uint8Array()).payload;
    if (deltaRequest.case !== 'requestCalculation') throw new Error('Missing delta request');
    expect(deltaRequest.value.baseInputVersion).toBe(2n);
    await store.receive(
      calculation('3', 9, {
        fullSnapshot: false,
        baseInputVersion: 2n,
        entries: [{ rowId, value: 9 }],
        evaluatedCells: 1,
        dirtyCells: 2,
      }),
    );
    expect(store.calculated.size).toBe(2);
    expect(store.calculated.get(`${rowId}/${columnId}`)).toEqual({ type: 'number', value: 9 });
    expect(store.calculated.get(`${secondRow}/${columnId}`)).toEqual({
      type: 'number',
      value: 4,
    });
    expect(store.performanceMetrics.recalculatedCells).toBe(1);
    expect(store.performanceMetrics.dirtyCells).toBe(2);

    const versionFour = structuredClone(versionThree);
    versionFour.version = '4';
    versionFour.sheets[0]!.cells[0]!.value = { type: 'number', value: 7 };
    store.load(versionFour);
    await store.receive(
      calculation('4', 0, {
        fullSnapshot: false,
        baseInputVersion: 3n,
        entries: [{ rowId }],
        evaluatedCells: 0,
        dirtyCells: 1,
      }),
    );
    expect(store.calculated.size).toBe(1);
    expect(store.calculated.has(`${rowId}/${columnId}`)).toBe(false);
    expect(store.calculated.get(`${secondRow}/${columnId}`)).toEqual({
      type: 'number',
      value: 4,
    });
    expect(store.display({ row: 0, column: 0 })).toBe('7');
    store.disconnected();
  });
  it('rejects a delta with a stale base without replacing the last confirmed calculation', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection('2'));
    await store.receive(calculation('2', 3));
    const versionThree = formulaProjection('3');
    store.load(versionThree);
    await store.receive(
      calculation('3', 99, {
        fullSnapshot: false,
        baseInputVersion: 1n,
      }),
    );
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculatedVersion).toBe('2');
    expect(store.calculated.get(`${rowId}/${columnId}`)).toEqual({ type: 'number', value: 3 });
    expect(store.display(cellPosition)).toBe('3');

    store.load(versionThree);
    await store.receive(calculation('3', 5));
    expect(store.calculationStatus).toBe('complete');
    expect(store.calculatedVersion).toBe('3');
    expect(store.display(cellPosition)).toBe('5');
    store.disconnected();
  });
  it('rejects deltas with stale cache keys or an empty CalculatedValue message', async () => {
    const { store } = await connectedStore();
    const secondRow = uuidV7();
    const thirdRow = uuidV7();
    const twoFormulas = formulaProjection('2');
    twoFormulas.sheets[0]!.rows.push(secondRow, thirdRow);
    twoFormulas.sheets[0]!.cells.push({
      ...structuredClone(twoFormulas.sheets[0]!.cells[0]!),
      row_id: secondRow,
    });
    store.load(twoFormulas);
    await store.receive(
      calculation('2', 3, {
        entries: [
          { rowId, value: 3 },
          { rowId: secondRow, value: 4 },
        ],
      }),
    );

    const shiftedFormulas = structuredClone(twoFormulas);
    shiftedFormulas.version = '3';
    shiftedFormulas.sheets[0]!.cells[0]!.value = { type: 'number', value: 7 };
    shiftedFormulas.sheets[0]!.cells.push({
      ...structuredClone(shiftedFormulas.sheets[0]!.cells[1]!),
      row_id: thirdRow,
    });
    store.load(shiftedFormulas);
    await store.receive(
      calculation('3', 0, {
        fullSnapshot: false,
        baseInputVersion: 2n,
        entries: [],
        evaluatedCells: 0,
        dirtyCells: 2,
      }),
    );
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculatedVersion).toBe('2');

    const scalarProjection = formulaProjection('3');
    scalarProjection.sheets[0]!.rows.push(secondRow);
    scalarProjection.sheets[0]!.cells.push({
      ...structuredClone(scalarProjection.sheets[0]!.cells[0]!),
      row_id: secondRow,
    });
    scalarProjection.sheets[0]!.cells[0]!.value = { type: 'number', value: 7 };
    store.load(scalarProjection);
    const malformed = calculation('3', 0, {
      fullSnapshot: false,
      baseInputVersion: 2n,
      entries: [],
      evaluatedCells: 0,
      dirtyCells: 1,
    });
    if (malformed.payload.case !== 'calculationCommitted')
      throw new Error('Missing calculation fixture');
    malformed.payload.value.cells = [
      create(CalculatedCellSchema, {
        address: create(CellAddressSchema, {
          sheetId: uuidBytes(sheetId),
          rowId: uuidBytes(rowId),
          columnId: uuidBytes(columnId),
        }),
        value: create(CalculatedValueSchema, {}),
      }),
    ];
    await store.receive(malformed);
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculatedVersion).toBe('2');
    expect(store.calculated.has(`${rowId}/${columnId}`)).toBe(true);
    store.disconnected();
  });
  it('does not promote values attached to a failed calculation', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2', 999, { error: 1 }));
    expect(store.calculationStatus).toBe('failed');
    expect(store.calculated.size).toBe(0);
    expect(store.calculatedVersion).toBeUndefined();
    expect(store.display(cellPosition)).toBe('');
    expect(store.value(cellPosition)?.type).toBe('formula');
    expect(store.calculationError).not.toBe('');
    store.disconnected();
  });
  it('never treats a result for a different input version as current', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection('2'));
    await store.receive(calculation('1', 99));
    expect(store.calculated.size).toBe(0);
    expect(store.display(cellPosition)).toBe('');
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
  it('retains the last result as presentation while the new formula remains pending', async () => {
    const { store } = await connectedStore();
    store.load(formulaProjection());
    await store.receive(calculation('2'));
    expect(store.calculationStatus).toBe('complete');
    store.set(cellPosition, parseInput('=4+5'));
    expect(store.display(cellPosition)).toBe('3');
    expect(store.value(cellPosition)).toEqual(parseInput('=4+5'));
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
    projection.calculation_context = previewContext(rowId, '2');
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
