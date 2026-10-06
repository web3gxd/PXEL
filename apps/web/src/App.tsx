import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { DocumentStore, displayValue } from './document/store';
import { columnLabel } from './grid/viewport';
import { CanvasGrid } from './grid/CanvasGrid';
import { StyleToolbar } from './document/StyleToolbar';
import { useSession } from './connection/useSession';
import { createSpreadsheetRegistry, type GridNavigation } from './commands/spreadsheet';
import { ShortcutPanel } from './commands/ShortcutPanel';
import { parseMarkdownInWorker } from './clipboard/markdown-worker-client';
import { MarkdownImport, type MarkdownImportRequest } from './clipboard/MarkdownImport';
import { AxisSizePanel } from './grid/AxisSizePanel';
import type { Position } from './document/store';
import { SharingDialog } from './sharing/SharingDialog';
import { SharedEntry, shareRoute } from './sharing/SharedEntry';
import { EncryptionBadge } from './sharing/EncryptionBadge';
import { OwnerNotifications } from './sharing/OwnerNotifications';
import { CommentsPanel } from './sharing/CommentsPanel';
import type { GuestSession } from './sharing/api';
import { DisclosurePanel } from './sharing/DisclosurePanel';
import { writeTextClipboard } from './clipboard/outbound';
import { ViewPanel } from './document/ViewPanel';
import { SortPanel } from './document/SortPanel';
import type { SortBounds } from './document/sort-range';
import { FormulaSyntax } from './document/FormulaSyntax';
import { HardcodePanel } from './document/HardcodePanel';
import { HardcodeMonitor } from './document/HardcodeMonitor';
import { ProfilePanel } from './document/ProfilePanel';
import { PrivacyPanel } from './document/PrivacyPanel';
import { PrivacyMonitor } from './document/PrivacyMonitor';
import { HistoryPanel } from './document/HistoryPanel';
import { PxelPanel } from './document/PxelPanel';
import { Home } from './library/Home';
import { BookTitle } from './library/BookTitle';
import { LibraryNavigation } from './library/LibraryNavigation';
import './sharing/sharing.css';
import { EditorMenus } from './ui/EditorMenus';
import { cellsMenu } from './ui/cells-menu';
import { ConnectionIndicator } from './connection/ConnectionIndicator';
import {
  Books,
  Plus,
  ArrowCounterClockwise,
  ArrowClockwise,
  ArrowsMerge,
  ArrowsSplit,
  ChartLine,
  ChatCircle,
  ClockCounterClockwise,
  DownloadSimple,
  Eye,
  FileArrowUp,
  FileLock,
  Funnel,
  Keyboard,
  ListNumbers,
  ShieldCheck,
  SortAscending,
  Table,
  UserCircle,
} from '@phosphor-icons/react';

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
  const [route] = useState(() => shareRoute(location.pathname, location.hash));
  const [initialMarkdown, setInitialMarkdown] = useState<string>();
  const [documentRoute, setDocumentRoute] = useState(() =>
    new URL(location.href).searchParams.has('document'),
  );
  return route ? (
    <SharedEntry route={route}>{(guest) => <SpreadsheetApp guest={guest} />}</SharedEntry>
  ) : documentRoute ? (
    <SpreadsheetApp initialMarkdown={initialMarkdown} />
  ) : (
    <Home
      onOpen={(id, markdown) => {
        if (markdown !== undefined) {
          history.replaceState(null, '', '/?document=' + encodeURIComponent(id));
          setInitialMarkdown(markdown);
          setDocumentRoute(true);
        } else location.assign('/?document=' + encodeURIComponent(id));
      }}
    />
  );
}

function SpreadsheetApp({
  guest,
  initialMarkdown,
}: {
  guest?: GuestSession;
  initialMarkdown?: string | undefined;
}) {
  const [document] = useState(() => new DocumentStore(guest ? { guest } : {}));
  const [navigation] = useState<GridNavigation>(() => ({}));
  const [commands] = useState(() => createSpreadsheetRegistry(document, navigation));
  const [markdownRequest, setMarkdownRequest] = useState<MarkdownImportRequest>();
  const [axisSizePanel, setAxisSizePanel] = useState<'row' | 'column'>();
  const [sharing, setSharing] = useState(false);
  const [disclosure, setDisclosure] = useState(false);
  const [viewPanel, setViewPanel] = useState(false);
  const [sortPanel, setSortPanel] = useState(false);
  const [hardcodePanel, setHardcodePanel] = useState(false);
  const [profilePanel, setProfilePanel] = useState(false);
  const [privacyPanel, setPrivacyPanel] = useState(false);
  const [historyPanel, setHistoryPanel] = useState(false);
  const [pxelPanel, setPxelPanel] = useState(false);
  const previewSort = useCallback(
    (bounds?: SortBounds) => document.previewSortBounds(bounds),
    [document],
  );
  const [comments, setComments] = useState(false);
  const [importPreview, setImportPreview] = useState<{
    origin: Position;
    rows: number;
    columns: number;
  }>();
  const markdownFile = useRef<HTMLInputElement>(null);
  useSyncExternalStore(document.subscribe, document.snapshot);
  const { state, reconnect } = useSession(document, guest);
  const inspectMarkdown = useCallback(
    async (text: string, file: boolean): Promise<boolean> => {
      if (!document.canEdit) return true;
      let parsed;
      try {
        parsed = await parseMarkdownInWorker(text);
      } catch (error) {
        document.error = error instanceof Error ? error.message : 'Не удалось разобрать Markdown.';
        document.notify();
        return file;
      }
      if (parsed.tables.length || file) {
        setMarkdownRequest({ text, markdown: parsed, origin: document.pasteOrigin, force: file });
        return true;
      }
      if (parsed.recognized) {
        if (!document.pasteMatrix([[text]], document.pasteOrigin, true)) return true;
        return true;
      }
      return false;
    },
    [document],
  );
  const updateImportPreview = useCallback(
    (rect: { origin: Position; rows: number; columns: number } | undefined) =>
      setImportPreview(rect),
    [],
  );
  const markdownStarted = useRef(false);
  useEffect(() => {
    if (initialMarkdown === undefined || !document.sheet || markdownStarted.current) return;
    markdownStarted.current = true;
    void inspectMarkdown(initialMarkdown, true);
  }, [document.sheet, initialMarkdown, inspectMarkdown]);

  if (guest && (state.kind === 'closed' || document.guestAccessClosed))
    return (
      <main className="shared-unlock shared-closed">
        <div className="shared-unlock-card">
          <span className="shared-wordmark">PXEL</span>
          <h1>Доступ по ссылке закрыт</h1>
          <p>Таблица больше недоступна. Обратитесь к владельцу за новой ссылкой.</p>
        </div>
      </main>
    );

  return (
    <main
      className="workspace"
      onCopyCapture={(event) => {
        const target = event.target as HTMLElement;
        if (!document.sheet || target.closest('[data-disclosure-clipboard],.sharing-comments'))
          return;
        if (target.closest('.grid-scroll') && !document.editing) return;
        event.preventDefault();
        const text =
          target instanceof HTMLTextAreaElement || target instanceof HTMLInputElement
            ? target.value.slice(target.selectionStart ?? 0, target.selectionEnd ?? 0)
            : (window.getSelection()?.toString() ?? '');
        void writeTextClipboard(document, text, 'copy', undefined, 'unmapped');
      }}
      onCutCapture={(event) => {
        const target = event.target as HTMLElement;
        if (!document.sheet || target.closest('[data-disclosure-clipboard],.sharing-comments'))
          return;
        if (target.closest('.grid-scroll') && !document.editing) return;
        event.preventDefault();
        if (target.closest('.sharing-dialog')) {
          const input =
            target instanceof HTMLTextAreaElement || target instanceof HTMLInputElement
              ? target
              : undefined;
          const original = input?.value,
            start = input?.selectionStart,
            end = input?.selectionEnd;
          const text =
            original !== undefined && start != null && end != null
              ? original.slice(start, end)
              : (window.getSelection()?.toString() ?? '');
          void writeTextClipboard(
            document,
            text,
            'cut',
            () => {
              if (
                input &&
                start != null &&
                end != null &&
                input.value === original &&
                input.selectionStart === start &&
                input.selectionEnd === end
              ) {
                input.setRangeText('', start, end, 'start');
                input.dispatchEvent(new Event('input', { bubbles: true }));
              }
            },
            'unmapped',
          );
          return;
        }
        if (!(target instanceof HTMLTextAreaElement) || !document.canEdit) return;
        const value = target.value,
          start = target.selectionStart,
          end = target.selectionEnd;
        void writeTextClipboard(
          document,
          value.slice(start, end),
          'cut',
          () => {
            if (
              target.value === value &&
              target.selectionStart === start &&
              target.selectionEnd === end
            )
              document.draft(value.slice(0, start) + value.slice(end));
          },
          'unmapped',
        );
      }}
      onKeyDown={(event) => {
        const target = event.target as HTMLElement;
        const context = target.closest('dialog,[role="dialog"]')
          ? 'dialog'
          : target.matches('input,textarea,[contenteditable="true"]')
            ? 'editor'
            : target.closest('.grid-scroll')
              ? 'grid'
              : 'chrome';
        commands.handle(event.nativeEvent, context);
      }}
    >
      <ShortcutPanel registry={commands} document={document} navigation={navigation} />
      <input
        ref={markdownFile}
        className="sr-only"
        type="file"
        accept=".md,text/markdown,text/plain"
        aria-label="Выбрать Markdown-файл"
        tabIndex={-1}
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.currentTarget.value = '';
          if (!file) return;
          if (file.size > 16 * 1024 * 1024) {
            document.error = 'Markdown-файл превышает лимит 16 МиБ.';
            document.notify();
            return;
          }
          void file.text().then((text) => inspectMarkdown(text, true));
        }}
      />
      <header className="app-header">
        <div className="document-heading">
          <div className="document-title-row">
            <span className="editor-brand" aria-hidden="true">
              <Table size={24} weight="duotone" />
            </span>
            <BookTitle document={document} />
            {document.sheet && (
              <span className="preview-badge" data-testid="save-status">
                {document.loading
                  ? 'Открываем…'
                  : document.pending.length
                    ? 'Сохраняем…'
                    : 'Сохранено'}
              </span>
            )}
            {document.sheet && <EncryptionBadge encryption={document.projection?.encryption} />}
            {guest && (
              <span className="preview-badge">
                {
                  (
                    {
                      view: 'Просмотр',
                      comment: 'Комментирование',
                      edit: 'Редактирование',
                    } as const
                  )[guest.permission]
                }
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
          <LibraryNavigation document={document} online={state.kind === 'online'}>
            {(library) => (
              <EditorMenus
                groups={[
                  {
                    label: 'Файл',
                    actions: [
                      ...(!guest
                        ? [
                            { label: 'Библиотека', icon: Books, run: library.home },
                            {
                              label: 'Создать таблицу',
                              icon: Plus,
                              disabled: library.disabled,
                              run: library.create,
                            },
                          ]
                        : []),
                      {
                        label: 'Импорт Markdown',
                        icon: FileArrowUp,
                        disabled: !document.canEdit,
                        run: () => markdownFile.current?.click(),
                      },
                      ...(!document.isGuest || document.projection?.capabilities?.can_export_pxel
                        ? [
                            {
                              label: 'Файл PXEL',
                              icon: FileLock,
                              disabled:
                                !document.sheet ||
                                state.kind !== 'online' ||
                                !!document.pending.length,
                              run: () => setPxelPanel(true),
                            },
                          ]
                        : []),
                      ...(document.canExport
                        ? [
                            {
                              label: document.exporting ? 'Готовим XLSX…' : 'Скачать XLSX',
                              icon: DownloadSimple,
                              testId: 'export-xlsx',
                              disabled:
                                !document.sheet ||
                                state.kind !== 'online' ||
                                document.exporting ||
                                !!document.pending.length,
                              run: () => void document.exportXlsx(),
                            },
                          ]
                        : []),
                      ...(document.canShare
                        ? [
                            {
                              label: 'Журнал и версии',
                              icon: ClockCounterClockwise,
                              disabled:
                                !document.sheet || !!document.pending.length || !!document.editing,
                              run: () => setHistoryPanel(true),
                            },
                          ]
                        : []),
                    ],
                  },
                  {
                    label: 'Правка',
                    actions: [
                      {
                        label: 'Отменить последнюю правку',
                        icon: ArrowCounterClockwise,
                        shortcut: 'Ctrl+Z',
                        disabled: !document.canUndo || state.kind !== 'online',
                        run: () => document.undo(),
                      },
                      {
                        label: 'Повторить отменённую правку',
                        icon: ArrowClockwise,
                        shortcut: 'Ctrl+Y',
                        disabled: !document.canRedo || state.kind !== 'online',
                        run: () => document.redo(),
                      },
                    ],
                  },
                  {
                    label: 'Вид',
                    actions: [
                      {
                        label: 'Вид листа',
                        icon: Funnel,
                        disabled: !document.sheet || document.viewLoading,
                        run: () => setViewPanel(true),
                      },
                      {
                        label: 'Комментарии',
                        icon: ChatCircle,
                        disabled: !document.sheet,
                        run: () => setComments((previous) => !previous),
                      },
                      ...(document.privacyMask
                        ? [
                            {
                              label: 'Показать оригиналы',
                              icon: Eye,
                              run: () => document.setPrivacyMask(undefined),
                            },
                          ]
                        : []),
                    ],
                  },
                  cellsMenu(document, state.kind === 'online', setAxisSizePanel),
                  {
                    label: 'Формат',
                    actions: [
                      {
                        label: 'Объединить ячейки',
                        icon: ArrowsMerge,
                        disabled: state.kind !== 'online' || !document.canMergeSelection,
                        run: () => document.setMergeSelected(true),
                      },
                      {
                        label: 'Разъединить ячейки',
                        icon: ArrowsSplit,
                        disabled: state.kind !== 'online' || !document.canUnmergeSelection,
                        run: () => document.setMergeSelected(false),
                      },
                    ],
                  },
                  {
                    label: 'Данные',
                    actions: [
                      {
                        label: 'Сортировка',
                        icon: SortAscending,
                        disabled:
                          !document.sheet ||
                          !document.canAuthor ||
                          !!document.pending.length ||
                          !!document.editing,
                        run: () => setSortPanel(true),
                      },
                      {
                        label: document.recalculating ? 'Пересчитываем…' : 'Пересчитать',
                        icon: ArrowClockwise,
                        testId: 'recalculate',
                        disabled:
                          !document.sheet ||
                          state.kind !== 'online' ||
                          document.recalculating ||
                          !!document.pending.length ||
                          !document.canEdit,
                        run: () => void document.recalculate(),
                      },
                      ...(document.canShare
                        ? [
                            {
                              label: 'Персональные данные',
                              icon: UserCircle,
                              disabled:
                                !document.sheet || !!document.pending.length || !!document.editing,
                              run: () => setPrivacyPanel(true),
                            },
                            {
                              label: 'Выдача и аудит',
                              icon: ShieldCheck,
                              run: () => setDisclosure(true),
                            },
                          ]
                        : []),
                    ],
                  },
                  {
                    label: 'Инструменты',
                    actions: [
                      ...(!document.isGuest
                        ? [
                            {
                              label: 'Проверка констант',
                              icon: ListNumbers,
                              disabled: !document.sheet || !!document.pending.length,
                              run: () => setHardcodePanel(true),
                            },
                          ]
                        : []),
                      ...(document.canShare
                        ? [
                            {
                              label: 'Профайлер',
                              icon: ChartLine,
                              disabled:
                                !document.sheet || !!document.pending.length || !!document.editing,
                              run: () => setProfilePanel(true),
                            },
                          ]
                        : []),
                      {
                        label: 'Настройки',
                        icon: Keyboard,
                        run: () => commands.open('settings'),
                      },
                      {
                        label: 'Горячие клавиши',
                        icon: Keyboard,
                        run: () => commands.open('help'),
                      },
                    ],
                  },
                ]}
              />
            )}
          </LibraryNavigation>
          {document.privacyAnalysisPending && document.privacyPolicy?.rules.enabled && (
            <span className="editor-status" role="status">
              Обновляем фильтр ПД…
            </span>
          )}
          {document.privacyAnalysisError && (
            <span className="editor-status" role="alert">
              {document.privacyAnalysisError}
            </span>
          )}
          {document.projection?.representation === 'anonymized' && (
            <span className="editor-status" role="status">
              Обезличенное представление · только значения
            </span>
          )}
        </div>
        <div className="sharing-header-actions">
          {!guest && document.canShare && (
            <>
              <OwnerNotifications />
              <button
                type="button"
                className="sharing-share-button"
                disabled={state.kind !== 'online'}
                onClick={() => setSharing(true)}
              >
                Поделиться
              </button>
            </>
          )}
          <ConnectionIndicator state={state} document={document} reconnect={reconnect} />
        </div>
      </header>

      <div className="toolbar" role="toolbar" aria-label="Форматирование">
        <div className="tool-group">
          <button
            type="button"
            className="tool"
            aria-label="Отменить последнюю правку"
            title="Отменить последнюю сохраненную правку"
            disabled={!document.canUndo || state.kind !== 'online'}
            onClick={() => document.undo()}
          >
            <Icon>
              <path d="M9 7 5 11l4 4M5 11h9a5 5 0 0 1 5 5" />
            </Icon>
          </button>
          <button
            type="button"
            className="tool"
            aria-label="Повторить отменённую правку"
            title="Повторить отменённую сохранённую правку"
            disabled={!document.canRedo || state.kind !== 'online'}
            onClick={() => document.redo()}
          >
            <Icon>
              <path d="m15 7 4 4-4 4m4-4h-9a5 5 0 0 0-5 5" />
            </Icon>
          </button>
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
        <StyleToolbar document={document} online={state.kind === 'online' && document.canEdit} />
        <button
          type="button"
          className="tool"
          aria-label="Объединить ячейки"
          title="Объединить выделенные ячейки"
          disabled={state.kind !== 'online' || !document.canMergeSelection}
          onClick={() => document.setMergeSelected(true)}
        >
          <ArrowsMerge size={18} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="tool"
          aria-label="Разъединить ячейки"
          title="Разъединить выделенные объединённые ячейки"
          disabled={state.kind !== 'online' || !document.canUnmergeSelection}
          onClick={() => document.setMergeSelected(false)}
        >
          <ArrowsSplit size={18} aria-hidden="true" />
        </button>
        <span className="view-mode">
          <Icon size={15}>
            <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6Z" />
            <circle cx="12" cy="12" r="2.5" />
          </Icon>
          {document.sheet && document.canEdit ? 'Редактирование' : 'Просмотр'}
        </span>
      </div>

      <div className="formula-bar" aria-label="Строка ввода">
        <HardcodeMonitor document={document} />
        <span className="cell-address">
          {document.sheet && !document.allRowsHidden
            ? `${columnLabel(document.selected.column)}${document.selected.row + 1}`
            : '—'}
        </span>
        <span className="formula-symbol" aria-hidden="true">
          ƒ<span>x</span>
        </span>
        {document.selectedHardcodeWarning && (
          <button
            type="button"
            aria-label="Константы отличаются от общего шаблона: открыть объяснение"
            title="Эвристика: константы отличаются от общего шаблона серии"
            onClick={() => setHardcodePanel(true)}
          >
            ⚠
          </button>
        )}
        {document.sheet ? (
          <FormulaSyntax
            text={
              document.allRowsHidden
                ? ''
                : (document.editing?.text ?? displayValue(document.value(document.selected)))
            }
            row={document.selected.row}
            column={document.selected.column}
          >
            <textarea
              className="formula-input"
              rows={1}
              wrap="off"
              aria-label="Значение ячейки"
              value={
                document.allRowsHidden
                  ? ''
                  : (document.editing?.text ?? displayValue(document.value(document.selected)))
              }
              disabled={document.allRowsHidden}
              readOnly={!document.canEdit}
              onFocus={() => {
                if (!document.editing) document.begin(undefined, 'bar');
              }}
              onChange={(event) => document.draft(event.target.value)}
              onBlur={() => {
                if (!document.navigationGuardActive) document.commit();
              }}
              onKeyDown={(event) => {
                if (event.nativeEvent.isComposing) return;
                if (event.key === 'Enter' && event.altKey) return;
                if (event.key === 'Enter') {
                  document.commit();
                  event.currentTarget.blur();
                } else if (event.key === 'Escape') {
                  document.cancel();
                  event.currentTarget.blur();
                }
              }}
            />
          </FormulaSyntax>
        ) : (
          <span className="formula-placeholder">Создайте таблицу, чтобы начать</span>
        )}
      </div>
      {(document.error || document.calculationError) && (
        <div className="document-error" role="alert">
          {document.error || document.calculationError}
        </div>
      )}
      <PrivacyMonitor document={document} />
      <CanvasGrid
        document={document}
        commands={commands}
        navigation={navigation}
        onMarkdownPaste={inspectMarkdown}
        importPreview={importPreview}
      />
      {sharing && document.canShare && document.projection && (
        <SharingDialog
          documentId={document.projection.document_id}
          onClose={() => setSharing(false)}
        />
      )}
      {comments && document.sheet && (
        <CommentsPanel document={document} onClose={() => setComments(false)} />
      )}
      {disclosure && document.canShare && document.projection && (
        <DisclosurePanel document={document} onClose={() => setDisclosure(false)} />
      )}
      {viewPanel && document.sheet && (
        <ViewPanel document={document} onClose={() => setViewPanel(false)} />
      )}
      {hardcodePanel && document.sheet && !document.isGuest && (
        <HardcodePanel document={document} onClose={() => setHardcodePanel(false)} />
      )}
      {profilePanel && document.sheet && document.canShare && (
        <ProfilePanel document={document} onClose={() => setProfilePanel(false)} />
      )}
      {privacyPanel && document.sheet && document.canShare && (
        <PrivacyPanel document={document} onClose={() => setPrivacyPanel(false)} />
      )}
      {historyPanel && document.sheet && document.canShare && (
        <HistoryPanel document={document} onClose={() => setHistoryPanel(false)} />
      )}
      {pxelPanel && <PxelPanel document={document} onClose={() => setPxelPanel(false)} />}
      {sortPanel && document.sheet && (
        <SortPanel
          document={document}
          onClose={() => setSortPanel(false)}
          onPreview={previewSort}
          onSort={(bounds, headers, keys, version) =>
            document.sortRange(bounds, headers, keys, version)
          }
        />
      )}
      {markdownRequest && (
        <MarkdownImport
          request={markdownRequest}
          document={document}
          onClose={() => setMarkdownRequest(undefined)}
          onPreview={updateImportPreview}
        />
      )}
      {axisSizePanel && (
        <AxisSizePanel
          document={document}
          axis={axisSizePanel}
          onClose={() => setAxisSizePanel(undefined)}
        />
      )}

      <footer className="sheet-footer">
        <div className="sheet-navigation">
          <button
            type="button"
            className="tool"
            aria-label="Добавить лист"
            title="Добавить лист"
            disabled={
              !document.sheet ||
              document.projection?.sheets.length === 64 ||
              state.kind !== 'online' ||
              !document.canEdit
            }
            onClick={() => document.addSheet()}
          >
            <Icon size={19}>
              <path d="M12 5v14M5 12h14" />
            </Icon>
          </button>
          <span className="sheet-list-icon" aria-hidden="true">
            <Icon size={18}>
              <path d="M7 6h13M7 12h13M7 18h13M3 6h.01M3 12h.01M3 18h.01" />
            </Icon>
          </span>
          {document.projection?.sheets.map((sheet) => (
            <button
              className={`sheet-tab${document.activeSheetId === sheet.id ? ' active' : ''}`}
              type="button"
              key={sheet.id}
              aria-label={`Перейти на лист ${document.privacyMask?.sheets.find((s) => s.id === sheet.id)?.name ?? sheet.name}`}
              aria-current={document.activeSheetId === sheet.id ? 'page' : undefined}
              onClick={() => document.selectSheet(sheet.id)}
            >
              <Icon size={15}>
                <rect x="4" y="3" width="16" height="18" rx="2" />
                <path d="M4 9h16M10 9v12" />
              </Icon>
              {document.privacyMask?.sheets.find((s) => s.id === sheet.id)?.name ?? sheet.name}
            </button>
          ))}
        </div>
        <span className="sheet-summary">
          {document.displaySheetName()}
          <span className="summary-separator">·</span>
          {document.sheet?.rows.length ?? 1_000} строк × {document.sheet?.columns.length ?? 26}{' '}
          столбцов
        </span>
        <span className="footer-wordmark" aria-hidden="true">
          PXEL
        </span>
      </footer>
    </main>
  );
}
