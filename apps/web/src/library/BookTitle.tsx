import { useEffect, useRef, useState } from 'react';
import { PencilSimple } from '@phosphor-icons/react';
import type { DocumentStore } from '../document/store';
import { Modal } from '../sharing/Modal';
import { readMetadata, rename, opened, type Metadata } from './api';
import { writeTextClipboard } from '../clipboard/outbound';
export function BookTitle({ document }: { document: DocumentStore }) {
  const id = document.projection?.document_id;
  const titleInput = useRef<HTMLInputElement>(null);
  const [loaded, setLoaded] = useState<{ id: string; metadata: Metadata }>(),
    [editing, setEditing] = useState(false),
    [title, setTitle] = useState(''),
    [error, setError] = useState(''),
    [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!id || document.isGuest) return;
    const abort = new AbortController();
    void readMetadata(id, abort.signal)
      .then((metadata) => {
        if (!abort.signal.aborted) {
          setLoaded({ id, metadata });
          setError('');
        }
      })
      .catch(() => {
        if (!abort.signal.aborted) setError('Не удалось загрузить название.');
      });
    void opened(id).catch(() => {
      if (!abort.signal.aborted) setError('Не удалось записать недавнее открытие.');
    });
    return () => abort.abort();
  }, [document, id]);
  const metadata = loaded && loaded.id === id ? loaded.metadata : undefined;
  const save = async () => {
    if (!id || !metadata) return;
    setBusy(true);
    setError('');
    try {
      setLoaded({ id, metadata: await rename(id, metadata, title) });
      setEditing(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Не удалось переименовать книгу.');
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <h1>{document.isGuest ? 'Таблица' : (metadata?.title ?? 'Новая таблица')}</h1>
      {document.canShare && metadata && (
        <button
          type="button"
          className="library-rename"
          aria-label="Переименовать книгу"
          onClick={() => {
            setTitle(metadata.title);
            setEditing(true);
            setError('');
          }}
        >
          <PencilSimple size={16} aria-hidden="true" />
        </button>
      )}
      {!editing && error && (
        <span role="status" className="library-title-error">
          {error}
        </span>
      )}
      {editing && (
        <Modal
          title="Название книги"
          initialFocus={titleInput}
          clipboardError={document.error}
          onClose={() => {
            if (!busy) setEditing(false);
          }}
        >
          <label>
            Название
            <input
              ref={titleInput}
              data-disclosure-clipboard="owned"
              value={title}
              disabled={busy}
              onChange={(e) => setTitle(e.target.value)}
              onCopy={(e) => {
                e.preventDefault();
                const input = e.currentTarget;
                void writeTextClipboard(
                  document,
                  input.value.slice(input.selectionStart ?? 0, input.selectionEnd ?? 0),
                  'copy',
                );
              }}
              onCut={(e) => {
                e.preventDefault();
                const input = e.currentTarget,
                  start = input.selectionStart ?? 0,
                  end = input.selectionEnd ?? start,
                  original = input.value;
                void writeTextClipboard(document, original.slice(start, end), 'cut', () =>
                  setTitle((current) =>
                    current === original ? current.slice(0, start) + current.slice(end) : current,
                  ),
                );
              }}
            />
          </label>
          <button
            type="button"
            disabled={busy || !title.trim() || [...title].length > 256}
            onClick={() => void save()}
          >
            Сохранить название
          </button>
          {error && <p role="alert">{error}</p>}
        </Modal>
      )}
    </>
  );
}
