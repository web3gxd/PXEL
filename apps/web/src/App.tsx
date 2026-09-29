import { useState, useSyncExternalStore, type ReactNode } from 'react';
import { DocumentStore, displayValue } from './document/store';
import { columnLabel } from './grid/viewport';
import { CanvasGrid } from './grid/CanvasGrid';
import { useSession } from './connection/useSession';

function Icon({ children, size = 18 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

function Tool({
  label,
  children,
  className = '',
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <button
      type="button"
      className={`tool ${className}`}
      disabled
      aria-label={label}
      title={`${label} — пока недоступно`}
    >
      {children}
    </button>
  );
}

export function App() {
  const [document] = useState(() => new DocumentStore());
  useSyncExternalStore(document.subscribe, document.snapshot);
  const { state, reconnect } = useSession(document);
  const canRetry = ['offline', 'unauthorized', 'incompatible'].includes(state.kind);

  return (
    <main className="workspace">
      <header className="app-header">
        <div className="document-heading">
          <div className="document-title-row">
            <h1>Новая таблица</h1>
            {!document.sheet && (
              <button
                className="create-document"
                disabled={document.loading || state.kind !== 'online'}
                onClick={() => void document.createDocument()}
              >
                Создать таблицу
              </button>
            )}
            {document.sheet && (
              <span className="preview-badge" data-testid="save-status">
                {document.pending.length ? 'Сохраняем…' : 'Сохранено'}
              </span>
            )}
            {document.remotePresence.length > 0 && (
              <span className="preview-badge" data-testid="remote-presence">
                Вместе в таблице: {document.remotePresence.length + 1}
              </span>
            )}
            {document.previewVisible && (
              <span className="preview-badge" data-testid="calculation-preview">
                Предпросмотр • не подтверждено
              </span>
            )}
            {document.previewStatus === 'invalid' && (
              <span className="preview-badge" data-testid="preview-status">
                Незавершенная или неподдерживаемая формула
              </span>
            )}
            {document.previewStatus === 'failed' && (
              <span className="preview-badge" data-testid="preview-status">
                Предпросмотр недоступен
              </span>
            )}
          </div>
          <nav className="document-menu" aria-label="Меню таблицы">
            {['Файл', 'Правка', 'Вид', 'Вставка', 'Формат', 'Данные'].map((label) => (
              <button type="button" key={label} disabled title={`${label} — пока недоступно`}>
                {label}
              </button>
            ))}
          </nav>
        </div>
        <div className="connection-area">
          <span
            className={`connection connection-${state.kind}`}
            role="status"
            data-testid="connection-status"
          >
            <span className="connection-dot" aria-hidden="true" />
            {state.label}
          </span>
          {canRetry && (
            <button className="retry-button" type="button" onClick={reconnect}>
              Повторить
            </button>
          )}
        </div>
      </header>

      <div
        className="toolbar"
        role="toolbar"
        aria-label="Форматирование (недоступно в режиме просмотра)"
      >
        <div className="tool-group">
          <Tool label="Отменить">
            <Icon>
              <path d="M9 7 5 11l4 4M5 11h9a5 5 0 0 1 5 5" />
            </Icon>
          </Tool>
          <Tool label="Повторить">
            <Icon>
              <path d="m15 7 4 4-4 4m4-4h-9a5 5 0 0 0-5 5" />
            </Icon>
          </Tool>
          <Tool label="Печать">
            <Icon>
              <path d="M7 8V3h10v5M7 17H4V9h16v8h-3M7 14h10v7H7zM17 11h.01" />
            </Icon>
          </Tool>
        </div>
        <span className="tool-separator" />
        <span className="zoom-display">
          100%
          <span className="tiny-chevron" aria-hidden="true">
            ⌄
          </span>
        </span>
        <span className="tool-separator" />
        <div className="tool-group number-tools">
          <Tool label="Числовой формат">
            <span>123</span>
          </Tool>
          <Tool label="Процентный формат">
            <span>%</span>
          </Tool>
          <Tool label="Денежный формат">
            <span>₽</span>
          </Tool>
        </div>
        <span className="tool-separator number-tools" />
        <Tool label="Шрифт" className="font-tool">
          <span>По умолчанию</span>
          <span className="tiny-chevron" aria-hidden="true">
            ⌄
          </span>
        </Tool>
        <Tool label="Размер шрифта" className="size-tool">
          11
        </Tool>
        <span className="tool-separator" />
        <div className="tool-group">
          <Tool label="Полужирный">
            <strong>B</strong>
          </Tool>
          <Tool label="Курсив">
            <em>I</em>
          </Tool>
          <Tool label="Зачеркнутый">
            <span className="strikethrough">S</span>
          </Tool>
          <Tool label="Цвет текста">
            <span className="text-color">A</span>
          </Tool>
        </div>
        <span className="tool-separator extra-tools" />
        <div className="tool-group extra-tools">
          <Tool label="Границы">
            <Icon>
              <rect x="4" y="4" width="16" height="16" rx="1" />
              <path d="M12 4v16M4 12h16" />
            </Icon>
          </Tool>
          <Tool label="Выравнивание">
            <Icon>
              <path d="M4 5h16M4 10h10M4 15h16M4 20h10" />
            </Icon>
          </Tool>
          <Tool label="Перенос текста">
            <Icon>
              <path d="M4 6h16M4 11h12a3 3 0 0 1 0 6h-5m2-3-3 3 3 3M4 17h2" />
            </Icon>
          </Tool>
        </div>
        <span className="view-mode">
          <Icon size={15}>
            <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6Z" />
            <circle cx="12" cy="12" r="2.5" />
          </Icon>
          {document.sheet ? 'Редактирование' : 'Просмотр'}
        </span>
      </div>

      <div className="formula-bar" aria-label="Строка ввода">
        <span className="cell-address">
          {document.sheet
            ? `${columnLabel(document.selected.column)}${document.selected.row + 1}`
            : '—'}
        </span>
        <span className="formula-symbol" aria-hidden="true">
          ƒ<span>x</span>
        </span>
        {document.sheet ? (
          <input
            className="formula-input"
            aria-label="Значение ячейки"
            value={document.editing?.text ?? displayValue(document.value(document.selected))}
            onFocus={() => document.begin(undefined, 'bar')}
            onChange={(event) => document.draft(event.target.value)}
            onBlur={() => document.commit()}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'Enter') {
                document.commit();
                event.currentTarget.blur();
              } else if (event.key === 'Escape') {
                document.cancel();
                event.currentTarget.blur();
              }
            }}
          />
        ) : (
          <span className="formula-placeholder">Создайте таблицу, чтобы начать</span>
        )}
      </div>
      {(document.error || document.calculationError) && (
        <div className="document-error" role="alert">
          {document.error || document.calculationError}
        </div>
      )}
      <CanvasGrid document={document} />

      <footer className="sheet-footer">
        <div className="sheet-navigation">
          <Tool label="Добавить лист">
            <Icon size={19}>
              <path d="M12 5v14M5 12h14" />
            </Icon>
          </Tool>
          <span className="sheet-list-icon" aria-hidden="true">
            <Icon size={18}>
              <path d="M7 6h13M7 12h13M7 18h13M3 6h.01M3 12h.01M3 18h.01" />
            </Icon>
          </span>
          <span className="sheet-tab">
            <Icon size={15}>
              <rect x="4" y="3" width="16" height="18" rx="2" />
              <path d="M4 9h16M10 9v12" />
            </Icon>
            Лист 1
          </span>
        </div>
        <span className="sheet-summary">
          {document.sheet ? 'Лист 1' : 'Пустой лист'}
          <span className="summary-separator">·</span>1 000 строк × 26 столбцов
        </span>
        <span className="footer-wordmark" aria-hidden="true">
          PXEL
        </span>
      </footer>
    </main>
  );
}
