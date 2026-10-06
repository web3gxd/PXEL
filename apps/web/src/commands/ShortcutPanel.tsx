import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import {
  KeyboardIcon,
  MagnifyingGlassIcon,
  XIcon,
  ArrowCounterClockwiseIcon,
} from '@phosphor-icons/react';
import type { DocumentStore, Position } from '../document/store';
import { columnLabel } from '../grid/viewport';
import { bindingOf, type ShortcutRegistry } from './registry';
import type { GridNavigation } from './spreadsheet';
import { scrollbarPreference, useScrollbarMode } from '../ui/preferences';
import { Select, SelectOption } from '../ui/Select';

export function ShortcutPanel({
  registry,
  document,
  navigation,
}: {
  registry: ShortcutRegistry;
  document: DocumentStore;
  navigation: GridNavigation;
}) {
  useSyncExternalStore(registry.subscribe, registry.snapshot);
  const scrollbarMode = useScrollbarMode();
  const dialog = useRef<HTMLDialogElement>(null);
  const [query, setQuery] = useState('');
  const [assigning, setAssigning] = useState<string>();
  const [error, setError] = useState('');
  useEffect(() => {
    setQuery('');
    setAssigning(undefined);
    setError('');
    if (registry.panel) dialog.current?.showModal();
    else dialog.current?.close();
  }, [registry.panel]);
  const find = (): Position[] => {
    const sheet = document.sheet;
    if (!sheet || !query.trim()) return [];
    const rows = new Map(sheet.rows.map((id, i) => [id, i])),
      columns = new Map(sheet.columns.map((id, i) => [id, i]));
    const addresses = new Set([...document.confirmed.keys()]);
    for (const op of document.pending) {
      if (op.kind === 'cell') addresses.add(`${op.row}/${op.column}`);
      if (op.kind === 'batch')
        for (const write of op.writes) addresses.add(`${write.row}/${write.column}`);
    }
    const found: Position[] = [];
    for (const address of addresses) {
      const [rowId, columnId] = address.split('/');
      const row = rows.get(rowId!),
        column = columns.get(columnId!);
      if (
        row !== undefined &&
        column !== undefined &&
        document.display({ row, column }).toLocaleLowerCase().includes(query.toLocaleLowerCase())
      )
        found.push({ row, column });
      if (found.length >= 100) break;
    }
    return found;
  };
  const panel = registry.panel;
  return (
    <dialog
      className="command-dialog"
      ref={dialog}
      onClose={() => registry.open(undefined)}
      aria-label={
        panel === 'find' ? 'Поиск на листе' : panel === 'settings' ? 'Настройки' : 'Горячие клавиши'
      }
    >
      {panel && (
        <>
          <header className="command-heading">
            <KeyboardIcon size={22} />
            <h2>
              {panel === 'find'
                ? 'Поиск на листе'
                : panel === 'insert'
                  ? 'Вставить'
                  : panel === 'delete'
                    ? 'Удалить'
                    : panel === 'settings'
                      ? 'Настройки'
                      : 'Горячие клавиши'}
            </h2>
            <button
              className="tool"
              aria-label="Закрыть окно"
              onClick={() => registry.open(undefined)}
            >
              <XIcon size={18} />
            </button>
          </header>
          {(panel === 'help' || panel === 'settings') && (
            <div className="command-tabs">
              <button aria-pressed={panel === 'help'} onClick={() => registry.open('help')}>
                Все сочетания
              </button>
              <button aria-pressed={panel === 'settings'} onClick={() => registry.open('settings')}>
                Настройки
              </button>
            </div>
          )}
          {panel === 'settings' && (
            <div className="scrollbar-setting">
              <label>
                Скроллбар{' '}
                <Select
                  aria-label="Режим скроллбара"
                  value={scrollbarMode}
                  onValueChange={(mode) => scrollbarPreference.set(mode as 'smart' | 'ordinary')}
                >
                  <SelectOption value="smart">Смарт</SelectOption>
                  <SelectOption value="ordinary">Обычный</SelectOption>
                </Select>
              </label>
              <p>
                {scrollbarMode === 'smart'
                  ? 'Смарт: точнее в заполненной области, свободный край остаётся доступным.'
                  : 'Обычный: равномерная шкала по всей длине листа.'}{' '}
                Колесо и тачпад прокручивают линейно в обоих режимах.
              </p>
            </div>
          )}
          {['help', 'settings', 'find'].includes(panel) && (
            <label className="command-search">
              <MagnifyingGlassIcon size={18} />
              <input
                autoFocus
                aria-label={panel === 'find' ? 'Найти значение' : 'Найти команду'}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={
                  panel === 'find' ? 'Текст или число' : 'Название команды или сочетание'
                }
              />
            </label>
          )}
          {error && <p role="alert">{error}</p>}
          <div className="command-list">
            {(panel === 'help' || panel === 'settings') &&
              registry.commands
                .filter((command) =>
                  `${command.label} ${registry.description(command.id)} ${command.group}`
                    .toLocaleLowerCase()
                    .includes(query.toLocaleLowerCase()),
                )
                .map((command) => (
                  <div className="command-row" key={command.id}>
                    <span>
                      <small>{command.group}</small>
                      {command.label}
                    </span>
                    <kbd>{registry.description(command.id) || 'Не назначено'}</kbd>
                    {panel === 'settings' && (
                      <>
                        <button
                          className="command-assign"
                          aria-label={`Назначить: ${command.label}`}
                          onClick={() => {
                            setAssigning(command.id);
                            setError('');
                          }}
                          onKeyDown={(event) => {
                            if (assigning !== command.id) return;
                            event.preventDefault();
                            event.stopPropagation();
                            if (event.key === 'Escape') {
                              setAssigning(undefined);
                              return;
                            }
                            if (['Control', 'Meta', 'Shift', 'Alt'].includes(event.key)) return;
                            const error = registry.assign(command.id, [
                              bindingOf(event.nativeEvent),
                            ]);
                            setError(error ?? '');
                            if (!error) setAssigning(undefined);
                          }}
                        >
                          {assigning === command.id ? 'Нажмите сочетание…' : 'Назначить'}
                        </button>
                        <button
                          className="tool"
                          aria-label={`Сбросить: ${command.label}`}
                          onClick={() => registry.reset(command.id)}
                        >
                          <ArrowCounterClockwiseIcon size={16} />
                        </button>
                      </>
                    )}
                  </div>
                ))}
            {panel === 'find' &&
              find().map((position) => (
                <button
                  className="find-result"
                  key={`${position.row}/${position.column}`}
                  onClick={() => {
                    navigation.move?.(position.row, position.column);
                    registry.open(undefined);
                    globalThis.document.querySelector<HTMLElement>('.grid-scroll')?.focus();
                  }}
                >
                  <kbd>
                    {columnLabel(position.column)}
                    {position.row + 1}
                  </kbd>
                  <span>{document.display(position)}</span>
                </button>
              ))}
            {(panel === 'insert' || panel === 'delete') && (
              <div className="axis-actions">
                {['Строка', 'Столбец'].map((label, index) => (
                  <button
                    key={label}
                    onClick={() => {
                      if (panel === 'insert') {
                        if (index) document.insertColumnBeforeSelection();
                        else document.insertRowBeforeSelection();
                      } else {
                        if (index) document.deleteSelectedColumn();
                        else document.deleteSelectedRow();
                      }
                      registry.open(undefined);
                    }}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}
          </div>
          {panel === 'settings' && (
            <footer>
              <button className="command-assign" onClick={() => registry.reset()}>
                Сбросить все сочетания
              </button>
              <span>Сохраняются в этом браузере</span>
            </footer>
          )}
        </>
      )}
    </dialog>
  );
}
