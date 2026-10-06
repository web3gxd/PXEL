import { describe, expect, it } from 'vitest';
import { bindingOf, ShortcutRegistry, type Command } from './registry';
const command = (
  id: string,
  bindings: string[],
  contexts: Command<string>['contexts'] = ['grid'],
): Command<string> => ({ id, bindings, contexts, label: id, group: 'test', execute: () => {} });
describe('central shortcut preferences', () => {
  it('detects intersecting contexts while allowing editor-only reuse', () => {
    const registry = new ShortcutRegistry([
      command('a', ['Ctrl+B']),
      command('b', []),
      command('c', ['Ctrl+B'], ['editor']),
    ]);
    expect(registry.assign('b', ['Ctrl+B'])).toContain('Конфликт');
    expect(registry.assign('b', ['Ctrl+I'])).toBeUndefined();
    expect(registry.bindings('b')).toEqual(['Ctrl+I']);
  });
  it('persists, reloads and resets overrides', () => {
    let saved: string | null = null;
    const storage = {
      getItem: () => saved,
      setItem: (_: string, value: string) => {
        saved = value;
      },
    };
    const commands = [command('a', ['Ctrl+B'])];
    const registry = new ShortcutRegistry(commands, storage);
    registry.assign('a', ['Ctrl+Shift+B']);
    const restored = new ShortcutRegistry(commands, storage);
    expect(restored.bindings('a')).toEqual(['Ctrl+Shift+B']);
    restored.reset();
    expect(restored.bindings('a')).toEqual(['Ctrl+B']);
  });
  it('rejects system shortcuts', () =>
    expect(new ShortcutRegistry([command('a', [])]).assign('a', ['Cmd+Space'])).toContain(
      'системное',
    ));
  it('uses the physical slash and retains the actual Ctrl/Cmd modifier', () =>
    expect(
      bindingOf({
        key: '?',
        code: 'Slash',
        ctrlKey: true,
        metaKey: false,
        altKey: false,
        shiftKey: false,
      }),
    ).toBe('Ctrl+/'));
});
