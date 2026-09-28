import { useEffect, useState } from 'react';
import { fromBinary } from '@bufbuild/protobuf';
import { ServerEnvelopeSchema } from '../generated/pxel/wire/v1/session_pb';
import type { DocumentStore } from '../document/store';
import { decodeWelcome, encodeHello, reconnectDelay, SessionProtocolError } from './protocol';

type ConnectionKind =
  'connecting' | 'online' | 'reconnecting' | 'offline' | 'unauthorized' | 'incompatible';
export type ConnectionState = { kind: ConnectionKind; label: string };

const CONNECTING: ConnectionState = {
  kind: 'connecting',
  label: 'Подключение…',
};

export function useSession(document?: DocumentStore) {
  const [state, setState] = useState<ConnectionState>(CONNECTING);
  const [generation, setGeneration] = useState(0);

  // The calculation worker belongs to the document, not an individual socket.
  useEffect(() => () => document?.dispose(), [document]);

  useEffect(() => {
    let active = true;
    let attempts = 0;
    let socket: WebSocket | undefined;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let handshakeTimer: ReturnType<typeof setTimeout> | undefined;
    const cancellation = new AbortController();
    // The instance identity is transient; cookies are HttpOnly and never read here.
    const instanceId = crypto.getRandomValues(new Uint8Array(16));
    instanceId[6] = ((instanceId[6] ?? 0) & 0x0f) | 0x40;
    instanceId[8] = ((instanceId[8] ?? 0) & 0x3f) | 0x80;

    const retry = () => {
      if (!active) return;
      if (attempts >= 6) {
        setState({ kind: 'offline', label: 'Нет подключения' });
        return;
      }
      setState({ kind: 'reconnecting', label: 'Восстанавливаем связь…' });
      retryTimer = setTimeout(() => {
        void connect();
      }, reconnectDelay(attempts++));
    };

    const connect = async () => {
      let response: Response;
      try {
        response = await fetch('/api/dev/session', {
          method: 'POST',
          credentials: 'same-origin',
          signal: AbortSignal.any([cancellation.signal, AbortSignal.timeout(5_000)]),
        });
      } catch {
        retry();
        return;
      }
      if (!active) return;
      if (response.status === 401 || response.status === 403 || response.status === 404) {
        setState({ kind: 'unauthorized', label: 'Требуется авторизация' });
        return;
      }
      if (!response.ok) {
        retry();
        return;
      }

      const url = new URL('/api/ws', window.location.href);
      url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const ws = new WebSocket(url);
      socket = ws;
      ws.binaryType = 'arraybuffer';
      let terminal = false;
      let welcomed = false;
      let documentMessages = Promise.resolve();
      const started = performance.now();
      handshakeTimer = setTimeout(() => ws.close(1000, 'Handshake timeout'), 5_000);
      ws.onopen = () => {
        if (!active) {
          ws.close();
          return;
        }
        const hello = encodeHello(instanceId);
        ws.send(new Uint8Array(hello).buffer);
      };
      ws.onmessage = (event: MessageEvent<unknown>) => {
        if (!active) return;
        try {
          if (!(event.data instanceof ArrayBuffer))
            throw new SessionProtocolError('invalid_message');
          if (welcomed) {
            if (event.data.byteLength > 1_048_576)
              throw new SessionProtocolError('invalid_message');
            const envelope = fromBinary(ServerEnvelopeSchema, new Uint8Array(event.data));
            if (envelope.protocolVersion !== 1)
              throw new SessionProtocolError('unsupported_protocol');
            if (
              ![
                'committed',
                'receipt',
                'rejected',
                'calculationCommitted',
                'documentHeadChanged',
              ].includes(envelope.payload.case ?? '')
            )
              throw new SessionProtocolError('invalid_message');
            documentMessages = documentMessages
              .then(async () => {
                if (active) await document?.receive(envelope);
              })
              .catch(() => {
                terminal = true;
                setState({ kind: 'incompatible', label: 'Не удалось согласовать соединение' });
                ws.close(4002, 'Invalid document message');
              });
            return;
          }
          const welcome = decodeWelcome(new Uint8Array(event.data));
          void document
            ?.connected(welcome.actorId, (bytes) => {
              if (ws.readyState !== WebSocket.OPEN || ws.bufferedAmount > 1_048_576) return false;
              ws.send(new Uint8Array(bytes).buffer);
              return true;
            })
            .then((synced) => {
              if (synced === false && active) ws.close(1000, 'Document sync failed');
            });
          welcomed = true;
          clearTimeout(handshakeTimer);
          setState({ kind: 'online', label: 'Подключено' });
        } catch (error) {
          if (error instanceof SessionProtocolError && error.code === 'server_busy') {
            ws.close(1000, 'Retry later');
            return;
          }
          terminal = true;
          const unauthorized =
            error instanceof SessionProtocolError && error.code === 'auth_required';
          setState(
            unauthorized
              ? { kind: 'unauthorized', label: 'Требуется авторизация' }
              : {
                  kind: 'incompatible',
                  label: 'Не удалось согласовать соединение',
                },
          );
          ws.close(4002, 'Protocol error');
        }
      };
      ws.onerror = () => {
        /* The close event owns reconnect; never log payloads. */
      };
      ws.onclose = () => {
        document?.disconnected();
        clearTimeout(handshakeTimer);
        if (!active || terminal) return;
        if (welcomed && performance.now() - started > 30_000) attempts = 0;
        retry();
      };
    };
    void connect();
    return () => {
      active = false;
      document?.disconnected();
      cancellation.abort();
      clearTimeout(retryTimer);
      clearTimeout(handshakeTimer);
      socket?.close(1000, 'Client closed');
    };
  }, [generation, document]);

  const reconnect = () => {
    setState(CONNECTING);
    setGeneration((previous) => previous + 1);
  };
  return { state, reconnect };
}
