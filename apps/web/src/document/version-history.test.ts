import { expect, it } from 'vitest';
import {
  decodeActivity,
  decodeDiff,
  decodeHistorical,
  describeHistorical,
} from './version-history';
import { uuidV7 } from './store';
it('loads a marked historical container without live capabilities or mutating the response', () => {
  const row = uuidV7(),
    column = uuidV7(),
    sheet = uuidV7();
  const raw = {
    schema_version: 1,
    read_only: true,
    projection: {
      document_id: uuidV7(),
      version: '1',
      state_hash: '00'.repeat(32),
      capabilities: null,
      sheets: [{ id: sheet, name: 'История', rows: [row], columns: [column], cells: [] }],
    },
    calculated: [
      { sheet_id: sheet, row_id: row, column_id: column, value: { type: 'number', value: 14 } },
    ],
  };
  const copy = structuredClone(raw),
    historical = decodeHistorical(raw);
  expect(historical.read_only).toBe(true);
  expect(historical.projection.capabilities).toBeUndefined();
  expect(raw).toEqual(copy);
  expect(() => decodeHistorical({ ...raw, read_only: false })).toThrow();
  expect(() =>
    decodeHistorical({ ...raw, calculated: [...raw.calculated, ...raw.calculated] }),
  ).toThrow();
});
it('rejects unknown history versions duplicate events and broken descending cursors', () => {
  const event = {
    operation_id: uuidV7(),
    actor_id: uuidV7(),
    created_at: 100,
    version: '8',
    operation_type: 'cell_write',
    structural: false,
    footprints: [
      { sheet_id: uuidV7(), addresses: [[uuidV7(), uuidV7()]], row_ids: [], column_ids: [] },
    ],
  };
  const raw = { schema_version: 1, through_version: '8', events: [event], next_before: '8' };
  expect(decodeActivity(raw).events.length).toBe(1);
  expect(() => decodeActivity({ ...raw, schema_version: 2 })).toThrow();
  expect(() => decodeActivity({ ...raw, events: [event, event] })).toThrow();
  expect(() => decodeActivity({ ...raw, next_before: '9' })).toThrow();
  expect(() =>
    decodeActivity({ ...raw, events: [{ ...event, operation_type: 'unknown' }] }),
  ).toThrow();
});
it('loads a mixed activity page containing durable sheet settings and sheet copy', () => {
  const events = ['sheet_settings', 'copy_sheet', 'cell_write'].map((operation_type, index) => ({
    operation_id: uuidV7(),
    actor_id: uuidV7(),
    created_at: 100,
    version: String(3 - index),
    operation_type,
    structural: index < 2,
    footprints: [],
  }));
  expect(
    decodeActivity({
      schema_version: 1,
      through_version: '3',
      events,
      next_before: null,
    }).events.map((event) => event.operation_type),
  ).toEqual(['sheet_settings', 'copy_sheet', 'cell_write']);
});
it('preserves before after and requires explicit structural context for formula shifts', () => {
  const change = {
    category: 'formula',
    sheet_id: uuidV7(),
    entity_id: uuidV7() + '/' + uuidV7(),
    property: 'formula',
    before: { authored_source: '=A1', current_text: '=A1' },
    after: { authored_source: '=A1', current_text: '=A2' },
    structural_context: true,
  };
  const raw = {
    schema_version: 1,
    document_id: uuidV7(),
    from_version: '1',
    to_version: '2',
    structural_context: true,
    changes: [change],
  };
  expect(describeHistorical(decodeDiff(raw).changes[0]!.after)).toBe('=A2');
  expect(() => decodeDiff({ ...raw, schema_version: 2 })).toThrow();
  expect(() => decodeDiff({ ...raw, structural_context: false })).toThrow();
  expect(() => decodeDiff({ ...raw, changes: [change, change] })).toThrow();
  expect(() => decodeHistorical({ schema_version: 2, projection: {}, calculated: [] })).toThrow();
});
