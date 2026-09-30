import { canvasFont, colorCss, type Border, type CellStyle } from '../document/styles';

export function drawCell(
  context: CanvasRenderingContext2D,
  style: Readonly<CellStyle>,
  text: string,
  x: number,
  y: number,
  width: number,
  height: number,
  numeric: boolean,
) {
  context.save();
  context.beginPath();
  context.rect(x + 1, y + 1, width - 2, height - 2);
  context.clip();
  context.font = canvasFont(style);
  context.fillStyle = colorCss(style.font_color);
  const align = style.horizontal === 'general' ? (numeric ? 'right' : 'left') : style.horizontal;
  context.textAlign = align;
  context.textBaseline = 'top';
  const lineHeight = (style.font_size_hundredth_points / 75) * 1.2;
  const lines: string[] = [];
  const limit = Math.max(1, Math.ceil((height - 4) / lineHeight));
  if (style.wrap) {
    let line = '';
    // Only visible lines are measured; long cells cannot cause unbounded wrapping.
    for (const point of text) {
      if (point === '\n' || (line && context.measureText(line + point).width > width - 14)) {
        lines.push(line);
        line = '';
        if (lines.length >= limit) break;
      }
      if (point !== '\n' && point !== '\r') line += point;
    }
    if (lines.length < limit) lines.push(line);
  } else lines.push(text.split(/\r?\n/, 1)[0] ?? '');
  const px = align === 'right' ? x + width - 7 : align === 'center' ? x + width / 2 : x + 7;
  const block = lines.length * lineHeight;
  const py =
    style.vertical === 'top'
      ? y + 3
      : style.vertical === 'center'
        ? y + (height - block) / 2
        : y + height - block - 3;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index] ?? '',
      top = py + index * lineHeight;
    context.fillText(line, px, top);
    if (style.font_strike || style.font_underline !== 'none') {
      const measured = context.measureText(line).width;
      const start = align === 'right' ? px - measured : align === 'center' ? px - measured / 2 : px;
      const stroke = (offset: number) => {
        context.beginPath();
        context.moveTo(start, top + offset);
        context.lineTo(start + measured, top + offset);
        context.stroke();
      };
      context.strokeStyle = colorCss(style.font_color);
      context.lineWidth = 1;
      context.setLineDash([]);
      if (style.font_strike) stroke(lineHeight * 0.45);
      if (style.font_underline !== 'none') stroke(lineHeight * 0.9);
      if (style.font_underline === 'double') stroke(lineHeight * 0.9 + 2);
    }
  }
  context.restore();
}
export function drawCellBorders(
  context: CanvasRenderingContext2D,
  style: Readonly<CellStyle>,
  x: number,
  y: number,
  width: number,
  height: number,
) {
  const edge = (
    border: Border | null,
    x1: number,
    y1: number,
    x2: number,
    y2: number,
    dx: number,
    dy: number,
  ) => {
    if (!border) return;
    context.strokeStyle = colorCss(border.color);
    context.lineWidth = border.line === 'medium' ? 2 : border.line === 'thick' ? 3 : 1;
    context.setLineDash(border.line === 'dashed' ? [4, 3] : border.line === 'dotted' ? [1, 2] : []);
    const stroke = (offset: number) => {
      context.beginPath();
      context.moveTo(x1 + dx * offset, y1 + dy * offset);
      context.lineTo(x2 + dx * offset, y2 + dy * offset);
      context.stroke();
    };
    stroke(0);
    if (border.line === 'double') stroke(2);
  };
  context.save();
  edge(style.border_top, x, y, x + width, y, 0, 1);
  edge(style.border_right, x + width, y, x + width, y + height, -1, 0);
  edge(style.border_bottom, x, y + height, x + width, y + height, 0, -1);
  edge(style.border_left, x, y, x, y + height, 1, 0);
  context.restore();
}
