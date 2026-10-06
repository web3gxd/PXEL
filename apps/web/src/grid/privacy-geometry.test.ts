import { expect, it } from 'vitest';
import { visiblePrivacyRect } from './privacy-geometry';

it('covers partial cells at every viewport edge without painting headers', () => {
  const viewport = { left: 46, top: 28, width: 300, height: 200 };
  expect(visiblePrivacyRect({ left: 46, top: 28, width: 112, height: 28 }, viewport)).toEqual({
    left: 47,
    top: 29,
    width: 110,
    height: 26,
  });
  expect(visiblePrivacyRect({ left: 0, top: 10, width: 112, height: 28 }, viewport)).toEqual({
    left: 46,
    top: 28,
    width: 65,
    height: 9,
  });
  expect(visiblePrivacyRect({ left: 300, top: 210, width: 112, height: 28 }, viewport)).toEqual({
    left: 301,
    top: 211,
    width: 45,
    height: 17,
  });
  expect(
    visiblePrivacyRect({ left: -200, top: 10, width: 112, height: 28 }, viewport),
  ).toBeUndefined();
  expect(
    visiblePrivacyRect({ left: 0, top: NaN, width: 112, height: 28 }, viewport),
  ).toBeUndefined();
});
it('clips merged cells and scrolling rows below a frozen pane', () => {
  expect(
    visiblePrivacyRect(
      { left: -100, top: -100, width: 500, height: 300 },
      { left: 46, top: 84, width: 300, height: 200 },
    ),
  ).toEqual({ left: 46, top: 84, width: 300, height: 115 });
});
