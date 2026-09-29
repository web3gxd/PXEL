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
    formula_engine_version: 'pxel-formula-0.4.0',
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
