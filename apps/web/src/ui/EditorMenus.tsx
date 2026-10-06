import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { CaretRight, type Icon } from '@phosphor-icons/react';

export type EditorAction = {
  label: string;
  icon: Icon;
  run?: () => void;
  children?: EditorAction[];
  unavailable?: string;
  disabled?: boolean;
  shortcut?: string;
  testId?: string;
};
export type EditorMenuGroup = { label: string; actions: EditorAction[]; content?: ReactNode };

function MenuAction({ action, close }: { action: EditorAction; close: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const ActionIcon = action.icon;
  return (
    <div className="editor-submenu" ref={root}>
      <button
        type="button"
        disabled={action.disabled}
        title={action.unavailable}
        data-testid={action.testId}
        data-submenu={action.children ? action.label : undefined}
        aria-expanded={action.children ? expanded : undefined}
        aria-controls={action.children ? id : undefined}
        onMouseDown={(event) => event.preventDefault()}
        onKeyDown={(event) => {
          if (action.children && event.key === 'ArrowRight') {
            event.preventDefault();
            event.stopPropagation();
            setExpanded(true);
            requestAnimationFrame(() =>
              root.current
                ?.querySelector<HTMLButtonElement>('.editor-submenu-content button:not(:disabled)')
                ?.focus(),
            );
          }
        }}
        onClick={() => {
          if (action.children) setExpanded(!expanded);
          else {
            action.run?.();
            close();
          }
        }}
      >
        <ActionIcon size={18} aria-hidden="true" />
        <span>{action.label}</span>
        {action.shortcut && <kbd>{action.shortcut}</kbd>}
        {action.unavailable && <small className="menu-unavailable">Недоступно</small>}
        {action.children && <CaretRight className="submenu-caret" size={14} aria-hidden="true" />}
      </button>
      {expanded && action.children && (
        <div
          className="editor-submenu-content"
          id={id}
          role="group"
          aria-label={action.label}
          onKeyDown={(event) => {
            if (event.key !== 'ArrowLeft') return;
            event.preventDefault();
            event.stopPropagation();
            setExpanded(false);
            root.current?.querySelector<HTMLButtonElement>(':scope > button')?.focus();
          }}
        >
          {action.children.map((child) => (
            <MenuAction key={child.label} action={child} close={close} />
          ))}
        </div>
      )}
    </div>
  );
}

/** Native buttons in a disclosure: Tab remains available, arrows add quick navigation. */
export function EditorMenus({ groups }: { groups: EditorMenuGroup[] }) {
  const [open, setOpen] = useState<string>();
  const root = useRef<HTMLElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const id = useId();
  const trigger = (label: string) =>
    root.current?.querySelector<HTMLButtonElement>(`[data-menu="${label}"]`);
  const close = (restore = false) => {
    if (restore && open) trigger(open)?.focus();
    setOpen(undefined);
  };
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(undefined);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      const nested = root.current?.querySelectorAll<HTMLButtonElement>(
        '[data-submenu][aria-expanded="true"]',
      );
      const last = nested?.[nested.length - 1];
      if (last) {
        last.click();
        last.focus();
        return;
      }
      setOpen(undefined);
      opener.current?.focus();
    };
    window.addEventListener('pointerdown', outside);
    window.addEventListener('keydown', escape, true);
    return () => {
      window.removeEventListener('pointerdown', outside);
      window.removeEventListener('keydown', escape, true);
    };
  }, [open]);
  const focusItem = (last = false) => {
    requestAnimationFrame(() => {
      const items = root.current?.querySelectorAll<HTMLButtonElement>(
        '.editor-menu-popup button:not(:disabled)',
      );
      (last ? items?.[items.length - 1] : items?.[0])?.focus();
    });
  };
  return (
    <nav
      className="document-menu"
      aria-label="Меню таблицы"
      ref={root}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) close();
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && open) {
          event.preventDefault();
          event.stopPropagation();
          close(true);
        }
      }}
    >
      {groups.map((group, index) => (
        <div className="editor-menu" key={group.label}>
          <button
            type="button"
            className="editor-menu-trigger"
            data-menu={group.label}
            aria-expanded={open === group.label}
            aria-controls={`${id}-${index}`}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => {
              if (!open)
                opener.current =
                  document.activeElement instanceof HTMLElement ? document.activeElement : null;
              setOpen(open === group.label ? undefined : group.label);
            }}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault();
                opener.current = event.currentTarget;
                setOpen(group.label);
                focusItem(event.key === 'ArrowUp');
              }
              if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
                event.preventDefault();
                const next =
                  groups[
                    (index + (event.key === 'ArrowRight' ? 1 : groups.length - 1)) % groups.length
                  ];
                if (!next) return;
                trigger(next.label)?.focus();
                if (open) setOpen(next.label);
              }
            }}
          >
            {group.label}
          </button>
          {open === group.label && (
            <div
              className="editor-menu-popup"
              id={`${id}-${index}`}
              role="group"
              aria-label={group.label}
              onKeyDown={(event) => {
                if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
                event.preventDefault();
                const items = Array.from(
                  event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'),
                );
                const current = items.indexOf(document.activeElement as HTMLButtonElement);
                const next =
                  event.key === 'Home'
                    ? 0
                    : event.key === 'End'
                      ? items.length - 1
                      : (current + (event.key === 'ArrowDown' ? 1 : items.length - 1)) %
                        items.length;
                items[next]?.focus();
              }}
            >
              {group.content}
              {group.actions.map((action) => (
                <MenuAction key={action.label} action={action} close={() => close(true)} />
              ))}
            </div>
          )}
        </div>
      ))}
    </nav>
  );
}
