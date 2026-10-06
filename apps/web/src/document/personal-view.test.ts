import { describe, expect, it } from 'vitest';
import { decodePersonalView } from './personal-view';

const id = (n: number) => `01990000-0000-7000-8000-${n.toString().padStart(12, '0')}`;
const fixture = () => ({
  schema_version: 1,
  revision: 1,
  settings: {
    filters: [
      {
        id: id(1),
        sheet_id: id(2),
        first_row_id: id(3),
        last_row_id: id(4),
        first_column_id: id(5),
        last_column_id: id(5),
        header_row_id: null,
        conditions: [
          { column_id: id(5), predicate: { kind: 'number', operator: 'gte', value: 10 } },
        ],
      },
    ],
  },
});

describe('personal view boundary', () => {
  it('preserves bounded warning dismissals and rejects malformed or duplicate fingerprints', () => {
    const base = fixture();
    const fingerprint = 'ab'.repeat(32);
    const settings = { ...base.settings, hardcode_dismissals: [fingerprint] };
    expect(decodePersonalView({ ...base, settings }).settings.hardcode_dismissals).toEqual([
      fingerprint,
    ]);
    expect(() =>
      decodePersonalView({
        ...base,
        settings: { ...settings, hardcode_dismissals: [fingerprint, fingerprint] },
      }),
    ).toThrow();
    expect(() =>
      decodePersonalView({
        ...base,
        settings: { ...settings, hardcode_dismissals: ['original formula'] },
      }),
    ).toThrow();
  });
  it('accepts the independent revision and rejects unknown versions and unsafe revisions', () => {
    expect(decodePersonalView(fixture()).revision).toBe(1);
    expect(() => decodePersonalView({ ...fixture(), schema_version: 2 })).toThrow();
    expect(() =>
      decodePersonalView({ ...fixture(), revision: Number.MAX_SAFE_INTEGER + 1 }),
    ).toThrow();
  });
  it('rejects unknown predicates, non-finite numbers and duplicate column conditions atomically', () => {
    const book = fixture(),
      filter = book.settings.filters[0]!;
    filter.conditions[0]!.predicate.value = Infinity;
    expect(() => decodePersonalView(book)).toThrow();
    filter.conditions[0]!.predicate.value = 10;
    filter.conditions[0]!.predicate.operator = 'approximately';
    expect(() => decodePersonalView(book)).toThrow();
    filter.conditions[0]!.predicate.operator = 'gte';
    filter.conditions.push(filter.conditions[0]!);
    expect(() => decodePersonalView(book)).toThrow();
  });
});
