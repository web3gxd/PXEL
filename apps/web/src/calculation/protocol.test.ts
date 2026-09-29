import { expect, it } from 'vitest';
import { toPreviewValue } from './protocol';
it('converts HTTP formula display metadata into the current-axis Worker DTO', () => {
  expect(
    toPreviewValue({
      type: 'formula',
      value: {
        source: '=A1',
        authored_source: '=A1',
        current_text: '=A2',
        canonical: '=A1',
        ast_schema: 1,
        compatibility_profile: 'pxel-excel-subset-v1',
        bound_at_version: '2',
      },
    }),
  ).toEqual({
    type: 'formula',
    value: {
      source: '=A2',
      ast_schema: 1,
      compatibility_profile: 'pxel-excel-subset-v1',
      bound_at_version: '2',
    },
  });
});
it('retains a consistent canonical hint and omits absent fields', () => {
  expect(
    toPreviewValue({
      type: 'formula',
      value: { source: '=1+2', current_text: '=(1+2)', canonical: '=(1+2)' },
    }),
  ).toEqual({
    type: 'formula',
    value: { source: '=(1+2)', canonical: '=(1+2)' },
  });
  expect(toPreviewValue({ type: 'formula', value: { source: '=2+3' } })).toEqual({
    type: 'formula',
    value: { source: '=2+3' },
  });
  expect(toPreviewValue({ type: 'number', value: 3 })).toEqual({ type: 'number', value: 3 });
});
