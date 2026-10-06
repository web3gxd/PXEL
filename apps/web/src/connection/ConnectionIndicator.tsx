import { useState } from 'react';
import type { ConnectionState } from './useSession';
import type { DocumentStore } from '../document/store';

export function ConnectionIndicator({
  state,
  document,
  reconnect,
}: {
  state: ConnectionState;
  document: DocumentStore;
  reconnect: () => void;
}) {
  const [open, setOpen] = useState(false);
  const retry = ['offline', 'unauthorized', 'incompatible'].includes(state.kind);
  return (
    <div
      className="connection-area connection-dashboard"
      onPointerEnter={() => setOpen(true)}
      onPointerLeave={(event) => {
        if (!event.currentTarget.contains(event.currentTarget.ownerDocument.activeElement))
          setOpen(false);
      }}
      onFocus={() => setOpen(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className={`connection-square connection-${state.kind}`}
        aria-label={`Соединение: ${state.label}`}
        aria-expanded={open}
        aria-controls="connection-dashboard"
        onClick={() => setOpen(true)}
      >
        <span className="connection-dot" aria-hidden="true" />
        <span className="sr-only" role="status" data-testid="connection-status">
          {state.label}
        </span>
      </button>
      {open && (
        <div
          className="connection-popover"
          id="connection-dashboard"
          role="region"
          aria-label="Состояние соединения"
        >
          <strong>{state.label}</strong>
          <p>Синхронизация с сервером PXEL</p>
          <dl>
            <div>
              <dt>Отклик WebSocket</dt>
              <dd>
                {state.kind === 'online' && state.latencyMs !== undefined
                  ? `${state.latencyMs} мс`
                  : '—'}
              </dd>
            </div>
            <div>
              <dt>Версия книги</dt>
              <dd>{document.projection?.version.toString() ?? '—'}</dd>
            </div>
            <div>
              <dt>Ожидают сохранения</dt>
              <dd>{document.pending.length}</dd>
            </div>
            <div>
              <dt>Расчёт</dt>
              <dd>
                {document.recalculating
                  ? 'Выполняется'
                  : document.calculatedVersion === document.projection?.version
                    ? 'Актуален'
                    : 'Ожидаем'}
              </dd>
            </div>
          </dl>
          <small>Отклик измерен при последнем подключении.</small>
          {retry && (
            <button type="button" onClick={reconnect}>
              Повторить подключение
            </button>
          )}
        </div>
      )}
    </div>
  );
}
