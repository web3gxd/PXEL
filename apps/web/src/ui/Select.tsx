import {
  Children,
  isValidElement,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type KeyboardEvent,
} from 'react';
import { CaretDownIcon, CheckIcon } from '@phosphor-icons/react';
import './select.css';

type OptionProps = { value?: string | number | undefined; disabled?: boolean; children: ReactNode };
// Declarative option data; the select renders the listbox itself.
export function SelectOption(_props: OptionProps) {
  return null;
}
function textOf(children: ReactNode): string {
  return Children.toArray(children)
    .map((child) =>
      isValidElement<{ children?: ReactNode }>(child)
        ? textOf(child.props.children)
        : String(child),
    )
    .join('');
}
type Props = {
  value: string | number;
  onValueChange: (value: string) => void;
  children: ReactNode;
  disabled?: boolean;
  className?: string;
  'aria-label': string;
};

export function Select({
  value,
  onValueChange,
  children,
  disabled,
  className,
  'aria-label': label,
}: Props) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const search = useRef({ text: '', at: 0 });
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const options = Children.toArray(children)
    .filter(isValidElement<OptionProps>)
    .map((child) => ({
      value: String(child.props.value ?? textOf(child.props.children)),
      label: textOf(child.props.children),
      disabled: !!child.props.disabled,
    }));
  const selected = options.findIndex((option) => option.value === String(value));
  const enabled = options
    .map((option, index) => (option.disabled ? -1 : index))
    .filter((index) => index >= 0);

  const close = () => setOpen(false);
  const show = () => {
    if (disabled || trigger.current?.matches(':disabled') || !enabled.length) return;
    search.current = { text: '', at: 0 };
    setActive(selected >= 0 && !options[selected]?.disabled ? selected : enabled[0]!);
    setOpen(true);
  };
  const choose = (index: number) => {
    const option = options[index];
    if (!option || option.disabled || disabled || trigger.current?.matches(':disabled')) return;
    close();
    trigger.current?.focus();
    if (option.value !== String(value)) onValueChange(option.value);
  };
  useEffect(() => {
    if (disabled) close();
  }, [disabled]);

  useLayoutEffect(() => {
    const popup = list.current;
    const button = trigger.current;
    if (!open || disabled || !popup || !button) {
      if (popup?.matches(':popover-open')) popup.hidePopover();
      return;
    }
    const position = () => {
      const box = button.getBoundingClientRect();
      const gap = 4;
      const availableBelow = window.innerHeight - box.bottom - gap - 8;
      const availableAbove = box.top - gap - 8;
      const above = availableBelow < 180 && availableAbove > availableBelow;
      const height = Math.max(40, Math.min(280, above ? availableAbove : availableBelow));
      popup.style.maxHeight = `${height}px`;
      popup.style.minWidth = `${Math.min(box.width, window.innerWidth - 16)}px`;
      popup.style.maxWidth = `${window.innerWidth - 16}px`;
      const width = Math.min(window.innerWidth - 16, Math.max(box.width, popup.scrollWidth));
      popup.style.left = `${Math.max(8, Math.min(box.left, window.innerWidth - width - 8))}px`;
      popup.style.top = `${above ? Math.max(8, box.top - Math.min(height, popup.scrollHeight) - gap) : box.bottom + gap}px`;
    };
    popup.showPopover();
    position();
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !popup.contains(event.target) &&
        !button.contains(event.target)
      )
        close();
    };
    const scroll = (event: Event) => {
      if (!(event.target instanceof Node) || !popup.contains(event.target)) position();
    };
    document.addEventListener('pointerdown', outside);
    window.addEventListener('resize', position);
    document.addEventListener('scroll', scroll, true);
    return () => {
      if (popup.matches(':popover-open')) popup.hidePopover();
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('resize', position);
      document.removeEventListener('scroll', scroll, true);
    };
  }, [open, disabled]);
  useEffect(() => {
    if (open)
      list.current
        ?.querySelector(`#${CSS.escape(`${id}-${active}`)}`)
        ?.scrollIntoView({ block: 'nearest' });
  }, [open, active, id]);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.nativeEvent.isComposing || event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === 'Tab') {
      close();
      return;
    }
    if (event.key === 'Escape') {
      if (!open) return;
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (['Enter', ' '].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (open) choose(active);
      else show();
      return;
    }
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (!open) show();
      const from = open ? active : selected;
      const next =
        event.key === 'Home'
          ? enabled[0]
          : event.key === 'End'
            ? enabled.at(-1)
            : enabled[
                (enabled.indexOf(from) + (event.key === 'ArrowDown' ? 1 : -1) + enabled.length) %
                  enabled.length
              ];
      if (next !== undefined) setActive(next);
      return;
    }
    if (event.key.length === 1) {
      event.preventDefault();
      event.stopPropagation();
      if (!open) show();
      const now = Date.now();
      const character = event.key.toLocaleLowerCase();
      const previous = now - search.current.at < 700 ? search.current.text : '';
      const query = previous === character ? character : previous + character;
      search.current = { text: query, at: now };
      const ordered = [...enabled.filter((i) => i > active), ...enabled.filter((i) => i <= active)];
      const match = ordered.find((i) => options[i]?.label.toLocaleLowerCase().startsWith(query));
      if (match !== undefined) setActive(match);
    }
  };

  return (
    <span className="custom-select">
      <button
        ref={trigger}
        type="button"
        role="combobox"
        className={`custom-select-trigger ${className ?? ''}`}
        disabled={disabled}
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open && !disabled}
        aria-controls={id}
        aria-activedescendant={open && !disabled ? `${id}-${active}` : undefined}
        onClick={() => (open ? close() : show())}
        onKeyDown={onKeyDown}
        onBlur={close}
      >
        <span>{options[selected]?.label ?? 'Выберите…'}</span>
        <CaretDownIcon size={14} aria-hidden="true" />
      </button>
      <div
        ref={list}
        id={id}
        role="listbox"
        aria-label={label}
        popover="manual"
        className="custom-select-list"
      >
        {open &&
          options.map((option, index) => (
            <div
              key={option.value}
              id={`${id}-${index}`}
              role="option"
              data-value={option.value}
              aria-selected={option.value === String(value)}
              aria-disabled={option.disabled || undefined}
              className={index === active ? 'is-active' : undefined}
              onPointerDown={(event) => event.preventDefault()}
              onPointerMove={(event) => {
                if (event.pointerType === 'mouse' && !option.disabled) setActive(index);
              }}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                choose(index);
              }}
            >
              <span>{option.label}</span>
              {option.value === String(value) && <CheckIcon size={14} aria-hidden="true" />}
            </div>
          ))}
      </div>
    </span>
  );
}
