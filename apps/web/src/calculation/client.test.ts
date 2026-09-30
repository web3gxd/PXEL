import { afterEach, expect, it, vi } from 'vitest';
import { PreviewClient, type WorkerPort } from './client';
import type { PreviewInit, PreviewResult, Reply, Request } from './protocol';

const id = '01994100-0000-7000-8000-000000000001';
const address = { sheet_id: id, row_id: id, column_id: id };
const initial: PreviewInit = {
  schema_version: 1,
  document_id: id,
  input_version: '1',
  pending_revision: '0',
  sheets: [{ id, name: 'Sheet', rows: [id], columns: [id], cells: [] }],
};
function harness() {
  vi.useFakeTimers();
  const requests: Request[] = [];
  const worker: WorkerPort = {
    postMessage: (request: Request) => {
      requests.push(request);
    },
    terminate: vi.fn(),
    onmessage: null,
    onerror: null,
  };
  const update = vi.fn();
  const client = new PreviewClient(update, () => worker);
  const reply = (message: Reply) =>
    worker.onmessage?.call(worker as Worker, { data: message } as MessageEvent<Reply>);
  const result = (request: Request, number = 3): PreviewResult => ({
    ...request.data,
    schema_version: 1,
    preview: true,
    complete: true,
    full: request.kind === 'init',
    formula_engine_version: 'pxel-formula-0.14.0',
    compatibility_profile: 'pxel-excel-subset-v1',
    normalized_result_hash: 'a'.repeat(64),
    evaluated_cells: 1,
    dirty_cells: 1,
    results: [{ ...address, value: { type: 'number', value: number } }],
    cleared: [],
  });
  client.reset(initial);
  return { client, worker, requests, update, reply, result };
}
afterEach(() => vi.useRealTimers());
it('uses atomic authored patches and coalesces independent style leaves while init is pending', () => {
  const h = harness();
  const init4: PreviewInit = {
    ...initial,
    schema_version: 4,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
  };
  h.client.reset(init4);
  const init = h.requests.at(-1)!;
  h.client.patch(
    { ...initial, pending_revision: '1' },
    [],
    [{ ...address, changes: [{ property: 'font_bold', value: true }] }],
  );
  h.client.patch(
    { ...initial, pending_revision: '2' },
    [],
    [
      {
        ...address,
        changes: [
          { property: 'font_italic', value: true },
          { property: 'font_bold', value: null },
        ],
      },
    ],
  );
  h.reply({
    id: init.id,
    result: {
      ...h.result(init),
      schema_version: 2,
      normalized_authored_state_hash: 'b'.repeat(64),
    },
  });
  const patch = h.requests.at(-1)!;
  expect(patch.kind).toBe('patch');
  expect(patch.data).toMatchObject({
    schema_version: 2,
    pending_revision: '2',
    cells: [],
    styles: [
      {
        ...address,
        changes: [
          { property: 'font_bold', value: null },
          { property: 'font_italic', value: true },
        ],
      },
    ],
  });
  h.reply({
    id: patch.id,
    result: {
      ...h.result(patch),
      schema_version: 2,
      normalized_authored_state_hash: 'c'.repeat(64),
    },
  });
  expect(h.update).toHaveBeenLastCalledWith(
    'ready',
    expect.any(Map),
    expect.objectContaining({ pending_revision: '2' }),
    undefined,
    'c'.repeat(64),
  );
});
it('rejects a result-only schema downgrade for authored preview hydration', () => {
  const h = harness();
  h.client.reset({
    ...initial,
    schema_version: 4,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
  });
  const init = h.requests.at(-1)!;
  h.reply({ id: init.id, result: h.result(init) });
  expect(h.update).toHaveBeenLastCalledWith('failed');
});
it('keeps an atomic invalid draft recoverable and retains its independent style changes', () => {
  const h = harness();
  h.client.reset({
    ...initial,
    schema_version: 4,
    last_axis_change_version: '1',
    retired_row_ids: [],
    retired_column_ids: [],
    row_activation_floors: [],
    column_activation_floors: [],
  });
  const init = h.requests.at(-1)!;
  h.reply({
    id: init.id,
    result: {
      ...h.result(init),
      schema_version: 2,
      normalized_authored_state_hash: 'b'.repeat(64),
    },
  });
  h.client.patch(
    { ...initial, pending_revision: '1' },
    [{ ...address, value: { type: 'formula', value: { source: '=SUM(' } } }],
    [{ ...address, changes: [{ property: 'font_bold', value: true }] }],
  );
  const invalid = h.requests.at(-1)!;
  h.reply({ id: invalid.id, error: 'InvalidFormula', phase: 'patch' });
  expect(h.update).toHaveBeenLastCalledWith('invalid');
  h.client.patch({ ...initial, pending_revision: '2' }, [
    { ...address, value: { type: 'formula', value: { source: '=2+3' } } },
  ]);
  const retry = h.requests.at(-1)!;
  expect(retry.kind).toBe('patch');
  expect(retry.data).toMatchObject({
    expected_pending_revision: '0',
    pending_revision: '2',
    cells: [{ ...address, value: { type: 'formula', value: { source: '=2+3' } } }],
    styles: [{ ...address, changes: [{ property: 'font_bold', value: true }] }],
  });
  h.reply({
    id: retry.id,
    result: {
      ...h.result(retry),
      schema_version: 2,
      normalized_authored_state_hash: 'c'.repeat(64),
    },
  });
  expect(h.update.mock.lastCall?.[0]).toBe('ready');
});
it('queues geometry during initialization and publishes move contents separately from calculated results', () => {
  const h = harness();
  const geometry = { source: address, destination: address, rows: 1, columns: 1 };
  expect(h.client.move({ ...initial, pending_revision: '1' }, geometry)).toBe(true);
  expect(h.requests).toHaveLength(1);
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  const move = h.requests[1]!;
  expect(move.kind).toBe('move');
  expect(move.data).toEqual({
    ...geometry,
    schema_version: 1,
    document_id: id,
    input_version: '1',
    pending_revision: '1',
    expected_pending_revision: '0',
  });
  h.reply({
    id: move.id,
    result: { ...h.result(move), move_writes: [{ ...address, value: { type: 'blank' } }] },
  });
  expect(h.update.mock.lastCall?.[0]).toBe('ready');
  expect(h.update.mock.lastCall?.[3].get(`${id}/${id}/${id}`)).toEqual({ type: 'blank' });
  h.client.dispose();
});
it('declines a move when unresolved cell edits or a second move exist', () => {
  const h = harness();
  const geometry = { source: address, destination: address, rows: 1, columns: 1 };
  h.client.patch({ ...initial, pending_revision: '1' }, [
    { ...address, value: { type: 'number', value: 1 } },
  ]);
  expect(h.client.move({ ...initial, pending_revision: '2' }, geometry)).toBe(false);
  h.client.dispose();
  const other = harness();
  expect(other.client.move({ ...initial, pending_revision: '1' }, geometry)).toBe(true);
  expect(other.client.move({ ...initial, pending_revision: '2' }, geometry)).toBe(false);
  other.client.dispose();
});
it('rejects missing, duplicate and nonfinite move content replies without promoting a preview', () => {
  for (const writes of [
    undefined,
    [{ ...address, value: { type: 'number', value: Infinity } }],
    [
      { ...address, value: { type: 'blank' } },
      { ...address, value: { type: 'blank' } },
    ],
  ]) {
    const h = harness();
    const init = h.requests[0]!;
    h.reply({ id: init.id, result: h.result(init) });
    h.client.move(
      { ...initial, pending_revision: '1' },
      { source: address, destination: address, rows: 1, columns: 1 },
    );
    const move = h.requests[1]!;
    h.reply({ id: move.id, result: { ...h.result(move), move_writes: writes } as PreviewResult });
    expect(h.update.mock.lastCall?.[0]).toBe('failed');
    expect(
      h.update.mock.calls.some(([status, , , contents]) => status === 'ready' && contents),
    ).toBe(false);
  }
});
it('rejects move output byte overflow and contents attached to a normal reply', () => {
  const h = harness();
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  h.client.move(
    { ...initial, pending_revision: '1' },
    { source: address, destination: address, rows: 1, columns: 1 },
  );
  const move = h.requests[1]!;
  const writes = Array.from({ length: 130 }, (_, index) => ({
    ...address,
    row_id: `01994100-0000-7000-8000-${String(index).padStart(12, '0')}`,
    value: { type: 'text' as const, value: 'x'.repeat(32768) },
  }));
  h.reply({ id: move.id, result: { ...h.result(move), move_writes: writes } });
  expect(h.update.mock.lastCall?.[0]).toBe('failed');
  const other = harness();
  const otherInit = other.requests[0]!;
  other.reply({
    id: otherInit.id,
    result: { ...other.result(otherInit), move_writes: [{ ...address, value: { type: 'blank' } }] },
  });
  expect(other.update.mock.lastCall?.[0]).toBe('failed');
});
it('does not publish an old moved projection after confirmed reset or a worker failure', () => {
  const h = harness();
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  h.client.move(
    { ...initial, pending_revision: '1' },
    { source: address, destination: address, rows: 1, columns: 1 },
  );
  const move = h.requests[1]!;
  h.client.reset({ ...initial, input_version: '2', pending_revision: '2' });
  h.reply({
    id: move.id,
    result: { ...h.result(move), move_writes: [{ ...address, value: { type: 'blank' } }] },
  });
  expect(h.update.mock.lastCall?.[0]).toBe('pending');
  expect(h.update.mock.calls.some(([, , , contents]) => contents)).toBe(false);
  h.client.dispose();
});
it('declines invalid or cross-sheet geometry and handles typed move refusal without changing canonical state', () => {
  const h = harness();
  const geometry = { source: address, destination: address, rows: 1, columns: 1 };
  for (const invalid of [
    { ...geometry, rows: 0 },
    { ...geometry, rows: 1001 },
    { ...geometry, destination: { ...address, sheet_id: '01994100-0000-7000-8000-000000000002' } },
  ])
    expect(h.client.move({ ...initial, pending_revision: '1' }, invalid)).toBe(false);
  expect(h.client.move({ ...initial, pending_revision: '1' }, geometry)).toBe(true);
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  h.reply({ id: h.requests[1]!.id, phase: 'move', error: 'UnsupportedMoveDependency' });
  expect(h.worker.terminate).toHaveBeenCalledOnce();
  expect(h.update.mock.lastCall?.[0]).toBe('failed');
});
it('coalesces edits while initialization is running, rejects stale UI promotion and keeps the result cache', () => {
  const h = harness();
  for (let revision = 1; revision <= 20; revision++)
    h.client.patch({ ...initial, pending_revision: String(revision) }, [
      { ...address, value: { type: 'number', value: revision } },
    ]);
  expect(h.requests).toHaveLength(1);
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  expect(h.update.mock.calls.some(([status]) => status === 'ready')).toBe(false);
  const patch = h.requests[1]!;
  expect(patch.kind).toBe('apply');
  if (patch.kind !== 'apply') throw new Error('Missing patch');
  expect(patch.data.cells).toEqual([{ ...address, value: { type: 'number', value: 20 } }]);
  expect(patch.data.expected_pending_revision).toBe('0');
  expect('sheets' in patch.data).toBe(false);
  h.reply({ id: patch.id, result: { ...h.result(patch), results: [] } });
  expect(h.update.mock.lastCall?.[0]).toBe('ready');
  expect(h.update.mock.lastCall?.[1].get(`${id}/${id}/${id}`)).toEqual({
    type: 'number',
    value: 3,
  });
  h.client.dispose();
});
it('recovers from incomplete draft syntax without resetting or advancing acknowledged state', () => {
  const h = harness();
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: h.result(init) });
  h.client.patch({ ...initial, pending_revision: '1' }, [
    { ...address, value: { type: 'formula', value: { source: '=' } } },
  ]);
  h.reply({ id: h.requests[1]!.id, error: 'InvalidFormula', phase: 'apply' });
  expect(h.worker.terminate).not.toHaveBeenCalled();
  expect(h.update.mock.lastCall?.[0]).toBe('invalid');
  h.client.patch({ ...initial, pending_revision: '2' }, [
    { ...address, value: { type: 'formula', value: { source: '=2+3' } } },
  ]);
  const patch = h.requests[2]!;
  if (patch.kind !== 'apply') throw new Error('Missing patch');
  expect(patch.data.expected_pending_revision).toBe('0');
  h.reply({ id: patch.id, result: h.result(patch, 5) });
  expect(h.update.mock.lastCall?.[0]).toBe('ready');
  expect(h.requests.filter((request) => request.kind === 'init')).toHaveLength(1);
  h.client.dispose();
});
it('terminates on calculation errors after an applied mutation and on timeout', () => {
  const h = harness();
  h.reply({ id: h.requests[0]!.id, error: 'InvalidFormula', phase: 'calculate' });
  expect(h.worker.terminate).toHaveBeenCalledOnce();
  expect(h.update.mock.lastCall?.[0]).toBe('failed');
  const other = harness();
  vi.advanceTimersByTime(5000);
  expect(other.worker.terminate).toHaveBeenCalledOnce();
});
it('refuses incorrect result stamps without publishing values', () => {
  const h = harness();
  const init = h.requests[0]!;
  h.reply({ id: init.id, result: { ...h.result(init), input_version: '2' } });
  expect(h.update.mock.lastCall?.[0]).toBe('failed');
  expect(h.worker.terminate).toHaveBeenCalledOnce();
});
it('refuses a cached worker from the previous engine release', () => {
  const h = harness();
  const init = h.requests[0]!;
  h.reply({
    id: init.id,
    result: { ...h.result(init), formula_engine_version: 'pxel-formula-0.1.0' },
  });
  expect(h.update.mock.lastCall?.[0]).toBe('failed');
  expect(h.update.mock.calls.some(([status]) => status === 'ready')).toBe(false);
  expect(h.worker.terminate).toHaveBeenCalledOnce();
});
it('fails without synthetic values when the worker cannot load and ignores late disposed replies', () => {
  const update = vi.fn();
  const client = new PreviewClient(update, () => {
    throw new Error('Missing worker asset');
  });
  client.reset(initial);
  expect(update.mock.lastCall?.[0]).toBe('failed');
  expect(update.mock.calls.some(([status]) => status === 'ready')).toBe(false);
  const h = harness();
  const init = h.requests[0]!;
  h.client.dispose();
  h.reply({ id: init.id, result: h.result(init) });
  expect(h.update.mock.calls.some(([status]) => status === 'ready')).toBe(false);
  expect(h.worker.terminate).toHaveBeenCalledOnce();
});
