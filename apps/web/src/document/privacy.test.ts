import { expect, it } from 'vitest';
import { decodeProjection, decodeRedactedProjection, DocumentStore, uuidV7 } from './store';
import {
  decodePrivacyPolicy,
  decodePrivacyReport,
  privacyExceptionBatch,
  privacyRangeFindings,
  type PrivacyFinding,
  type PrivacyReport,
} from './privacy';
function safe() {
  const sheet = uuidV7(),
    row = uuidV7(),
    column = uuidV7();
  return {
    redacted_projection_schema_version: 1,
    style_schema_version: 2,
    style_profile_version: 2,
    document_id: uuidV7(),
    version: '4',
    privacy_revision: 2,
    sheets: [
      {
        id: sheet,
        name: 'Лист',
        rows: [row],
        columns: [column],
        cells: [{ row_id: row, column_id: column, value: { type: 'text', value: '[СКРЫТО]' } }],
        styles: [],
        row_heights: [],
        column_widths: [],
        merges: [],
        frozen_rows: 0,
      },
    ],
    capabilities: {
      schema_version: 2,
      can_edit: false,
      can_comment: true,
      can_share: false,
      can_export: false,
      can_copy: true,
      anonymized: true,
      is_guest: true,
      disclosure_stamp: { document_version: 4, book_revision: 2, link_revision: 1 },
    },
  };
}
it('range exclusions normalize endpoints stay on one sheet and reject an oversized batch atomically', () => {
  const raw = safe(),
    projection = decodeRedactedProjection(raw);
  projection.sheets[0]!.rows = Array.from({ length: 25 }, () => uuidV7());
  projection.sheets[0]!.columns = Array.from({ length: 5 }, () => uuidV7());
  const finding = (
    row: number,
    column: number,
    sheet = projection.sheets[0]!.id,
  ): PrivacyFinding => ({
    sheet_id: sheet,
    row_id: uuidV7(),
    column_id: uuidV7(),
    row,
    column,
    kind: 'passport',
    fingerprint: `${row}${column}`.padStart(64, '0'),
  });
  const findings = [finding(1, 1), finding(19, 3), finding(20, 3), finding(1, 1, uuidV7())];
  const report: PrivacyReport = {
    schema_version: 1,
    document_version: '4',
    policy_revision: 2,
    affected: 4,
    projection,
    findings,
  };
  expect(privacyRangeFindings(report, projection.sheets[0]!.id, ' d20:b2 ')).toEqual(
    findings.slice(0, 2),
  );
  expect(privacyRangeFindings(report, projection.sheets[0]!.id, 'B2')).toEqual([findings[0]]);
  expect(() => privacyRangeFindings(report, projection.sheets[0]!.id, 'B0:D20')).toThrow();
  expect(() => privacyRangeFindings(report, projection.sheets[0]!.id, 'B2:Z20')).toThrow();
  const existing = Array.from({ length: 1000 }, (_, i) => i.toString(16).padStart(64, '0'));
  expect(privacyExceptionBatch(existing, [{ ...findings[0]!, fingerprint: existing[0]! }])).toEqual(
    existing,
  );
  expect(() =>
    privacyExceptionBatch(existing, [{ ...findings[0]!, fingerprint: 'f'.repeat(64) }]),
  ).toThrow('1000');
  expect(existing).toHaveLength(1000);
});
it('decodes a separate scalar representation and rejects formula context history and editable rights', () => {
  const raw = safe(),
    projection = decodeProjection(raw);
  expect(projection.representation).toBe('anonymized');
  expect(projection.calculation_context).toBeUndefined();
  expect(() =>
    decodeRedactedProjection({ ...raw, calculation_context: { random_seed: 'original' } }),
  ).toThrow();
  expect(() => decodeRedactedProjection({ ...raw, history: [] })).toThrow();
  expect(() =>
    decodeRedactedProjection({
      ...raw,
      sheets: [
        {
          ...raw.sheets[0],
          cells: [
            {
              ...raw.sheets[0]!.cells[0],
              value: { type: 'text', value: '[СКРЫТО]', source: 'secret' },
            },
          ],
        },
      ],
    }),
  ).toThrow();
  expect(() =>
    decodeRedactedProjection({ ...raw, redacted_projection_schema_version: 2 }),
  ).toThrow();
  expect(() =>
    decodeRedactedProjection({ ...raw, capabilities: { ...raw.capabilities, can_edit: true } }),
  ).toThrow();
  expect(() =>
    decodeRedactedProjection({
      ...raw,
      sheets: [
        {
          ...raw.sheets[0],
          cells: [
            {
              row_id: raw.sheets[0]!.rows[0],
              column_id: raw.sheets[0]!.columns[0],
              value: { type: 'formula', value: { source: 'secret' } },
            },
          ],
        },
      ],
    }),
  ).toThrow();
});
it('keeps local masks separate from authored values and refuses stale safe copying', () => {
  const raw = safe(),
    masked = decodeProjection(raw);
  const original = decodeProjection({
    ...structuredClone(masked),
    representation: undefined,
    capabilities: { ...masked.capabilities, anonymized: false, is_guest: false, can_edit: true },
  });
  original.sheets[0]!.cells[0]!.value = { type: 'text', value: 'original@example.com' };
  const store = new DocumentStore();
  original.capabilities!.can_share = true;
  store.load(original);
  const hash = store.projection?.state_hash;
  store.setPrivacyMask(masked);
  expect(store.canEdit).toBe(true);
  store.begin();
  expect(store.editing?.text).toBe('original@example.com');
  store.cancel();
  expect(store.display({ row: 0, column: 0 })).toBe('[СКРЫТО]');
  expect(store.privacyOriginalDisplay({ row: 0, column: 0 })).toBe('original@example.com');
  expect(store.confirmed.values().next().value).toEqual({
    type: 'text',
    value: 'original@example.com',
  });
  expect(store.pending).toHaveLength(0);
  expect(store.projection?.state_hash).toBe(hash);
  expect(store.copyRedactedSelectionTsv(masked)).toBe('[СКРЫТО]');
  expect(() => store.copyRedactedSelectionTsv({ ...masked, version: '3' })).toThrow();
  store.privacyMask = { ...masked, version: '3' };
  expect(store.privacyOriginalDisplay({ row: 0, column: 0 })).toBeUndefined();
  store.begin();
  expect(store.editing?.text).toBe('original@example.com');
  store.cancel();
  store.privacyMask = masked;
  store.projection!.capabilities!.is_guest = true;
  expect(store.privacyOriginalDisplay({ row: 0, column: 0 })).toBeUndefined();
  store.projection!.capabilities!.is_guest = false;
  store.setPrivacyMask(undefined);
  store.refreshPrivacyMask(masked);
  expect(store.privacyMask).toBeUndefined();
  expect(store.display({ row: 0, column: 0 })).toBe('original@example.com');
  store.setPrivacyMask(masked);
  store.begin('replacement@example.com');
  store.refreshPrivacyMask(masked);
  expect(store.editing?.text).toBe('replacement@example.com');
  expect(store.privacyMask).toBe(masked);
  store.cancel();
  store.projection!.capabilities!.can_edit = false;
  expect(store.canEdit).toBe(false);
  store.begin();
  expect(store.editing).toBeUndefined();
  store.dispose();
});
it('validates opt-in types exact fingerprints and report projection revision together', () => {
  const policy = {
    schema_version: 1,
    revision: 0,
    access_epoch: 0,
    rules: { enabled: false, types: [], exceptions: [] },
  };
  expect(decodePrivacyPolicy(policy).rules.enabled).toBe(false);
  expect(() =>
    decodePrivacyPolicy({
      ...policy,
      rules: { enabled: true, types: ['email', 'email'], exceptions: [] },
    }),
  ).toThrow();
  expect(() =>
    decodePrivacyPolicy({
      ...policy,
      rules: { enabled: true, types: ['unknown'], exceptions: [] },
    }),
  ).toThrow();
  const raw = safe();
  expect(
    decodePrivacyReport({
      schema_version: 1,
      document_version: '4',
      policy_revision: 2,
      affected: 0,
      findings: [],
      projection: raw,
    }).projection.representation,
  ).toBe('anonymized');
  expect(() =>
    decodePrivacyReport({
      schema_version: 1,
      document_version: '4',
      policy_revision: 3,
      affected: 0,
      findings: [],
      projection: raw,
    }),
  ).toThrow();
});
