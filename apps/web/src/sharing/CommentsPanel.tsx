import { useEffect, useRef, useState } from 'react';
import { type DocumentStore, uuidV7 } from '../document/store';
import { writeTextClipboard } from '../clipboard/outbound';
import { commentsRequest, SharingError, type Comments } from './api';
export function CommentsPanel({
  document,
  onClose,
}: {
  document: DocumentStore;
  onClose: () => void;
}) {
  const sheet = document.sheet;
  const documentId = document.projection?.document_id;
  const rowId = sheet?.rows[document.selected.row];
  const columnId = sheet?.columns[document.selected.column];
  const [result, setResult] = useState<Comments>({ revision: '0', comments: [] });
  const [text, setText] = useState('');
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const generation = useRef(0);
  const pending = useRef<
    | { request_id: string; sheet_id: string; row_id: string; column_id: string; text: string }
    | undefined
  >(undefined);
  const path = documentId ? document.documentUrl(documentId, '/comments') : undefined;
  const selectionKey = `${sheet?.id}/${rowId}/${columnId}`;
  const selectedPath = (cursor?: string) => {
    const query = new URLSearchParams({
      sheet_id: sheet?.id ?? '',
      row_id: rowId ?? '',
      column_id: columnId ?? '',
    });
    if (cursor) query.set('cursor', cursor);
    return `${path}?${query}`;
  };
  const unavailable = (reason: unknown) => {
    if (document.isGuest && reason instanceof SharingError && reason.code === 'unavailable')
      document.closeGuestAccess();
    setError(reason instanceof Error ? reason.message : 'Не удалось загрузить комментарии.');
  };
  useEffect(() => {
    const current = ++generation.current;
    const abort = new AbortController();
    setResult({ revision: '0', comments: [] });
    setError('');
    setText('');
    pending.current = undefined;
    if (path && sheet?.id && rowId && columnId) {
      setLoading(true);
      const query = new URLSearchParams({ sheet_id: sheet.id, row_id: rowId, column_id: columnId });
      void commentsRequest(`${path}?${query}`, undefined, 'GET', abort.signal)
        .then((value) => {
          if (current === generation.current) setResult(value);
        })
        .catch((reason) => {
          if (!abort.signal.aborted && current === generation.current) {
            if (document.isGuest && reason instanceof SharingError && reason.code === 'unavailable')
              document.closeGuestAccess();
            setError('Не удалось загрузить комментарии.');
          }
        })
        .finally(() => {
          if (current === generation.current) setLoading(false);
        });
    }
    return () => {
      generation.current++;
      abort.abort();
    };
  }, [document, path, selectionKey, sheet?.id, rowId, columnId]);
  const post = async () => {
    if (
      !document.canComment ||
      !path ||
      !sheet ||
      !rowId ||
      !columnId ||
      busy ||
      !text ||
      new TextEncoder().encode(text).length > 4096
    )
      return;
    const current = generation.current;
    pending.current ??= {
      request_id: uuidV7(),
      sheet_id: sheet.id,
      row_id: rowId,
      column_id: columnId,
      text,
    };
    setBusy(true);
    setError('');
    try {
      const value = await commentsRequest(path, pending.current);
      if (current === generation.current) {
        setResult(value);
        setText('');
        pending.current = undefined;
      }
    } catch (reason) {
      if (current === generation.current) unavailable(reason);
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const remove = async (id: string) => {
    if (!document.canComment || !path || busy) return;
    const current = generation.current;
    setBusy(true);
    setError('');
    try {
      const value = await commentsRequest(
        `${path}/${encodeURIComponent(id)}`,
        { request_id: uuidV7() },
        'DELETE',
      );
      if (current === generation.current) setResult(value);
    } catch (reason) {
      if (current === generation.current) unavailable(reason);
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const more = async () => {
    if (!result.next_cursor || busy) return;
    const current = generation.current;
    setBusy(true);
    try {
      const value = await commentsRequest(selectedPath(result.next_cursor));
      if (current === generation.current)
        setResult((previous) => ({
          ...value,
          comments: [...previous.comments, ...value.comments]
            .filter(
              (item, index, all) =>
                all.findIndex((other) => other.comment_id === item.comment_id) === index,
            )
            .slice(-100),
        }));
    } catch (reason) {
      if (current === generation.current) unavailable(reason);
    } finally {
      if (current === generation.current) setBusy(false);
    }
  };
  const selected = result.comments.filter(
    (comment) =>
      comment.sheet_id === sheet?.id && comment.row_id === rowId && comment.column_id === columnId,
  );
  return (
    <aside
      className="sharing-comments"
      aria-labelledby="sharing-comments-title"
      onCopy={(event) => {
        event.preventDefault();
        const target = event.target;
        const value =
          target instanceof HTMLTextAreaElement
            ? target.value.slice(target.selectionStart, target.selectionEnd)
            : (window.getSelection()?.toString() ?? '');
        void writeTextClipboard(document, value, 'copy');
      }}
      onCut={(event) => {
        event.preventDefault();
        const target = event.target;
        if (!(target instanceof HTMLTextAreaElement)) return;
        const start = target.selectionStart,
          end = target.selectionEnd,
          original = target.value;
        void writeTextClipboard(document, original.slice(start, end), 'cut', () => {
          if (target.selectionStart === start && target.selectionEnd === end) {
            setText((value) =>
              value === original ? value.slice(0, start) + value.slice(end) : value,
            );
            pending.current = undefined;
          }
        });
      }}
    >
      <header>
        <h2 id="sharing-comments-title">Комментарии к ячейке</h2>
        <button
          type="button"
          className="sharing-close"
          aria-label="Закрыть комментарии"
          onClick={onClose}
        >
          ×
        </button>
      </header>
      {loading ? (
        <p role="status">Загружаем комментарии…</p>
      ) : !rowId || !columnId ? (
        <p>Выберите существующую ячейку.</p>
      ) : selected.length ? (
        <ul>
          {selected.map((comment) => (
            <li key={comment.comment_id}>
              <p>{comment.text}</p>
              {comment.can_delete && document.canComment && (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void remove(comment.comment_id)}
                >
                  Удалить комментарий
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <p>Комментариев пока нет.</p>
      )}
      {result.next_cursor && (
        <button type="button" disabled={busy} onClick={() => void more()}>
          Ещё комментарии
        </button>
      )}
      {document.canComment && rowId && columnId && (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void post();
          }}
        >
          <label className="sharing-label">
            Ваш комментарий
            <textarea
              value={text}
              onChange={(event) => {
                setText(event.target.value);
                pending.current = undefined;
              }}
              disabled={busy}
              rows={3}
            />
          </label>
          <p className="sharing-help">
            До 4096 байт. Комментарий привязан к этой ячейке и не перемещается вместе с её
            содержимым.
          </p>
          <button
            className="sharing-primary"
            disabled={busy || !text || new TextEncoder().encode(text).length > 4096}
          >
            Добавить комментарий
          </button>
        </form>
      )}
      {error && (
        <p className="sharing-error" role="alert">
          {error}
        </p>
      )}
    </aside>
  );
}
