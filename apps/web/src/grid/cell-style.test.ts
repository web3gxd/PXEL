import { describe, expect, it, vi } from 'vitest';
import { DEFAULT_STYLE, canvasFont } from '../document/styles';
import { drawCell, drawCellBorders } from './cell-style';
function context() {
  return {
    save: vi.fn(),
    restore: vi.fn(),
    beginPath: vi.fn(),
    rect: vi.fn(),
    clip: vi.fn(),
    moveTo: vi.fn(),
    lineTo: vi.fn(),
    stroke: vi.fn(),
    fillText: vi.fn(),
    setLineDash: vi.fn(),
    measureText: vi.fn((text: string) => ({ width: [...text].length * 7 })),
    font: '',
    textAlign: '',
    textBaseline: '',
    fillStyle: '',
    strokeStyle: '',
    lineWidth: 0,
  };
}
describe('visible cell formatting', () => {
  it('uses safe font, numeric alignment, explicit alignment and cell clipping', () => {
    const ctx = context(),
      style = { ...DEFAULT_STYLE, font_bold: true, font_color: [255, 0, 0] as const };
    drawCell(ctx as unknown as CanvasRenderingContext2D, style, '123', 10, 20, 112, 28, true);
    expect(ctx.font).toBe(canvasFont(style));
    expect(ctx.fillStyle).toBe('#ff0000');
    expect(ctx.textAlign).toBe('right');
    expect(ctx.rect).toHaveBeenCalledWith(11, 21, 110, 26);
    expect(ctx.fillText).toHaveBeenCalledWith('123', 115, 29);
    drawCell(
      ctx as unknown as CanvasRenderingContext2D,
      { ...style, horizontal: 'center', vertical: 'top' },
      '123',
      10,
      20,
      112,
      28,
      false,
    );
    expect(ctx.fillText).toHaveBeenLastCalledWith('123', 66, 23);
  });
  it('bounds wrapping by visible lines and draws independent styled blank borders', () => {
    const ctx = context();
    drawCell(
      ctx as unknown as CanvasRenderingContext2D,
      { ...DEFAULT_STYLE, wrap: true },
      'a'.repeat(32768),
      0,
      0,
      112,
      28,
      false,
    );
    expect(ctx.measureText.mock.calls.length).toBeLessThan(40);
    expect(ctx.fillText.mock.calls.length).toBeLessThanOrEqual(2);
    drawCellBorders(
      ctx as unknown as CanvasRenderingContext2D,
      {
        ...DEFAULT_STYLE,
        border_left: { line: 'double', color: [1, 2, 3] },
        border_top: { line: 'dashed', color: [4, 5, 6] },
      },
      0,
      0,
      112,
      28,
    );
    expect(ctx.setLineDash).toHaveBeenCalledWith([4, 3]);
    expect(ctx.moveTo).toHaveBeenCalledWith(2, 0);
    expect(ctx.lineTo).toHaveBeenCalledWith(2, 28);
    expect(ctx.stroke).toHaveBeenCalledTimes(3);
  });
});
