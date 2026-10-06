import { describe, expect, it } from 'vitest';
import { AssistantEngine } from './engine';

describe('local assistant engine', () => {
  it('recognizes consecutive actions and discards all observations when disabled', () => {
    const engine = new AssistantEngine();
    for (let row = 0; row < 3; row++)
      engine.observe({ kind: 'edit', position: { row, column: 0 } });
    const values = () => ({ type: 'text' as const, value: 'same' });
    expect(engine.suggest(values)?.id).toBe('repeat');
    engine.setEnabled(false);
    engine.observe({ kind: 'paste' });
    expect(
      engine.suggest(() => {
        throw new Error('Disabled analysis must not read cells');
      }),
    ).toBeUndefined();
    engine.setEnabled(true);
    expect(engine.suggest(values)).toBeUndefined();
  });
  it('recognizes numeric progressions in either axis without retaining contents', () => {
    const engine = new AssistantEngine();
    for (let column = 0; column < 3; column++)
      engine.observe({ kind: 'edit', position: { row: 0, column } });
    expect(engine.suggest((p) => ({ type: 'number', value: p.column * 2 + 1 }))?.id).toBe('series');
    expect(
      engine.suggest((p) => ({ type: 'number', value: p.column === 2 ? 9 : p.column })),
    ).toBeUndefined();
  });
  it('ignores unrelated edits, suppresses dismissed tips and supports several action families', () => {
    const engine = new AssistantEngine();
    for (const kind of ['paste', 'copy', 'undo', 'format'] as const) {
      engine.reset();
      for (let i = 0; i < 3; i++) engine.observe({ kind });
      expect(engine.suggest(() => undefined)?.id).toBe(kind);
      engine.dismiss(kind);
      expect(engine.suggest(() => undefined)).toBeUndefined();
    }
    engine.reset();
    for (let row = 0; row < 3; row++)
      engine.observe({ kind: 'edit', position: { row: row * 2, column: 0 } });
    expect(engine.suggest(() => ({ type: 'number', value: 7 }))).toBeUndefined();
  });
  it('offers formula shortcuts without generating writes or retaining formula source', () => {
    const engine = new AssistantEngine();
    for (let row = 0; row < 3; row++)
      engine.observe({ kind: 'edit', position: { row, column: 0 } });
    const tip = engine.suggest(() => ({ type: 'formula', value: { source: '=A1' } }));
    expect(tip?.id).toBe('formula');
    expect(tip?.source).toBeUndefined();
    expect(JSON.stringify(engine)).not.toContain('=A1');
  });
});
