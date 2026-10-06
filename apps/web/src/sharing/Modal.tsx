import { useEffect, useRef, type ReactNode, type RefObject } from 'react';
import {
  X,
  SlidersHorizontal,
  Table,
  Funnel,
  SortAscending,
  ShieldCheck,
  UserCircle,
  ChartLine,
  ClockCounterClockwise,
  FileLock,
  ListNumbers,
  ShareNetwork,
  PencilSimple,
  type Icon,
} from '@phosphor-icons/react';

const icons: Record<string, Icon> = {
  'Размеры ячеек': SlidersHorizontal,
  'Вид листа': Funnel,
  'Сортировка диапазона': SortAscending,
  'Выдача и аудит': ShieldCheck,
  'Персональные данные': UserCircle,
  'Профайлер формул': ChartLine,
  'Журнал и версии': ClockCounterClockwise,
  'Файл PXEL': FileLock,
  'Проверка констант': ListNumbers,
  'Поделиться таблицей': ShareNetwork,
  'Название книги': PencilSimple,
};
export function Modal({
  title,
  onClose,
  children,
  initialFocus,
  clipboard,
  clipboardError,
  closeDisabled = false,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  initialFocus?: RefObject<HTMLElement | null>;
  clipboard?: 'credentials';
  clipboardError?: string;
  closeDisabled?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const TitleIcon = icons[title] ?? Table;
  useEffect(() => {
    const dialog = ref.current;
    const opener =
      document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
    dialog?.showModal();
    initialFocus?.current?.focus();
    return () => {
      dialog?.close();
      opener?.focus();
    };
  }, [initialFocus]);
  return (
    <dialog
      ref={ref}
      className="sharing-dialog"
      data-disclosure-clipboard={clipboard}
      aria-labelledby="sharing-dialog-title"
      onKeyDown={(event) => {
        if (event.key !== 'Tab') return;
        const controls = Array.from(
          event.currentTarget.querySelectorAll<HTMLElement>(
            'button,input,select,textarea,a[href],[tabindex]',
          ),
        ).filter(
          (element) =>
            !element.matches(':disabled') &&
            element.tabIndex >= 0 &&
            element.getClientRects().length > 0,
        );
        const first = controls[0],
          last = controls.at(-1);
        if (!first || !last) {
          event.preventDefault();
          return;
        }
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }}
      onCancel={(event) => {
        event.preventDefault();
        if (!closeDisabled) onClose();
      }}
    >
      <header className="sharing-dialog-header">
        <span className="modal-title-icon" aria-hidden="true">
          <TitleIcon size={22} />
        </span>
        <h2 id="sharing-dialog-title">{title}</h2>
        <button
          type="button"
          className="sharing-close"
          aria-label="Закрыть окно"
          disabled={closeDisabled}
          onClick={onClose}
        >
          <X size={18} aria-hidden="true" />
        </button>
      </header>
      <div className="modal-body">
        {children}
        {clipboardError && <p role="alert">{clipboardError}</p>}
      </div>
    </dialog>
  );
}
