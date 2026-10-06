export type CommandContext = 'grid' | 'editor' | 'dialog' | 'chrome';
export type Binding = string;
export type CommandId =
  | 'help'
  | 'undo'
  | 'redo'
  | 'copy'
  | 'cut'
  | 'paste'
  | 'find'
  | 'bold'
  | 'italic'
  | 'underline'
  | 'strike'
  | 'clear-format'
  | 'select-row'
  | 'select-column'
  | 'select-all'
  | 'insert'
  | 'delete-axis'
  | 'edit'
  | `move-${string}`
  | `clear-${'Delete' | 'Backspace'}`
  | `align-${'left' | 'center' | 'right'}`;
export type Command<Id extends string = CommandId> = {
  id: Id;
  label: string;
  group: string;
  bindings: readonly Binding[];
  contexts: readonly CommandContext[];
  enabled?: () => boolean;
  execute: () => void;
  native?: boolean;
};
export const isMac = () =>
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
export function bindingOf(
  event: Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>,
): Binding {
  const parts: string[] = [];
  if (event.ctrlKey) parts.push('Ctrl');
  if (event.metaKey) parts.push('Cmd');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey) parts.push('Shift');
  const key =
    event.code === 'Slash'
      ? '/'
      : event.code === 'Space'
        ? 'Space'
        : event.key.length === 1
          ? event.key.toUpperCase()
          : event.key;
  parts.push(key);
  return parts.join('+');
}
const RESERVED = new Set([
  'Alt+F4',
  'Cmd+Q',
  'Cmd+Space',
  'Ctrl+Alt+Delete',
  'Ctrl+L',
  'Cmd+L',
  'Ctrl+W',
  'Cmd+W',
  'Ctrl+T',
  'Cmd+T',
  'Ctrl+R',
  'Cmd+R',
]);
export class ShortcutRegistry<Id extends string = CommandId> {
  private overrides: Record<string, Binding[]> = {};
  private revision = 0;
  private listeners = new Set<() => void>();
  panel: 'help' | 'settings' | 'find' | 'insert' | 'delete' | undefined;
  constructor(
    public readonly commands: readonly Command<Id>[],
    private readonly storage:
      Pick<Storage, 'getItem' | 'setItem'> | undefined = typeof localStorage === 'undefined'
      ? undefined
      : localStorage,
  ) {
    try {
      const raw = storage?.getItem('pxel.shortcuts.v1');
      const state = raw && (JSON.parse(raw) as { version?: number; bindings?: unknown });
      if (state && state.version === 1 && state.bindings && typeof state.bindings === 'object') {
        for (const [id, bindings] of Object.entries(state.bindings)) {
          if (
            commands.some((command) => command.id === id) &&
            Array.isArray(bindings) &&
            bindings.length <= 4 &&
            bindings.every(
              (binding) =>
                typeof binding === 'string' && binding.length <= 80 && !RESERVED.has(binding),
            )
          ) {
            if (!this.conflicts(id as Id, bindings).length) this.overrides[id] = bindings;
          }
        }
      }
    } catch {
      /* Invalid or unavailable preferences keep the defaults. */
    }
  }
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  snapshot = () => this.revision;
  private notify() {
    this.revision++;
    this.listeners.forEach((fn) => fn());
  }
  open(panel: typeof this.panel) {
    this.panel = panel;
    this.notify();
  }
  bindings(id: Id) {
    return this.overrides[id] ?? this.commands.find((command) => command.id === id)?.bindings ?? [];
  }
  description(id: Id) {
    return this.bindings(id).join(' / ');
  }
  conflicts(id: Id, bindings: readonly Binding[]) {
    const target = this.commands.find((command) => command.id === id);
    if (!target) return ['Неизвестная команда'];
    return this.commands
      .filter(
        (command) =>
          command.id !== id &&
          command.contexts.some((context) => target.contexts.includes(context)) &&
          this.bindings(command.id).some((binding) => bindings.includes(binding)),
      )
      .map((command) => command.label);
  }
  assign(id: Id, bindings: Binding[]): string | undefined {
    if (bindings.some((binding) => RESERVED.has(binding))) return 'Это системное сочетание.';
    const conflicts = this.conflicts(id, bindings);
    if (conflicts.length) return `Конфликт: ${conflicts.join(', ')}`;
    this.overrides[id] = [...bindings];
    this.save();
    return undefined;
  }
  reset(id?: string) {
    if (id) delete this.overrides[id];
    else this.overrides = {};
    this.save();
  }
  private save() {
    try {
      this.storage?.setItem(
        'pxel.shortcuts.v1',
        JSON.stringify({ version: 1, bindings: this.overrides }),
      );
    } catch {
      /* Session preferences still work. */
    }
    this.notify();
  }
  run(id: Id) {
    const command = this.commands.find((command) => command.id === id);
    if (!command || command.enabled?.() === false) return false;
    command.execute();
    return true;
  }
  handle(event: KeyboardEvent, context: CommandContext): boolean {
    if (event.isComposing || event.key === 'Process' || event.getModifierState('AltGraph'))
      return false;
    const binding = bindingOf(event);
    const command = this.commands.find(
      (command) =>
        command.contexts.includes(context) && this.bindings(command.id).includes(binding),
    );
    if (!command || command.enabled?.() === false) return false;
    // Default clipboard bindings preserve the trusted browser clipboard event.
    if (command.native && command.bindings.includes(binding)) return true;
    event.preventDefault();
    command.execute();
    return true;
  }
}
