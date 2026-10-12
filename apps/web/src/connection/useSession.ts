import { useEffect, useState } from 'react';
import { fromBinary } from '@bufbuild/protobuf';
import { ServerEnvelopeSchema } from '../generated/pxel/wire/v1/session_pb';
import type { DocumentStore } from '../document/store';
import { boundedJson, decodeGuest, guestPath, type GuestSession } from '../sharing/api';
import { decodeWelcome, encodeHello, reconnectDelay, SessionProtocolError } from './protocol';
import { refreshAccount } from '../auth/sessionEvents';

type ConnectionKind =
  'connecting' | 'online' | 'reconnecting' | 'offline' | 'unauthorized' | 'incompatible' | 'closed';
export type ConnectionState = {
  kind: ConnectionKind;
  label: string;
  latencyMs?: number | undefined;
};

const CONNECTING: ConnectionState = {
  kind: 'connecting',
  label: 'Подключение…',
};

export function sessionEndpoints(guest?: Pick<GuestSession, 'linkId'>) {
  return guest
    ? {
        session: `${guestPath(guest.linkId)}/session`,
        method: 'GET',
        websocket: `${guestPath(guest.linkId)}/ws`,
      }
    : { session: '/api/auth/session', method: 'POST', websocket: '/api/ws' };
}
export function useSession(document?: DocumentStore, guest = document?.transport.guest) {
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
    const endpoints = sessionEndpoints(guest);
    let accessClosed = false;
    const closeAccess = () => {
      if (!active || accessClosed) return;
      accessClosed = true;
      clearTimeout(retryTimer);
      clearTimeout(handshakeTimer);
      cancellation.abort();
      document?.closeGuestAccess();
      setState({ kind: 'closed', label: 'Доступ по ссылке закрыт' });
      socket?.close(4003, 'Access closed');
    };
    const stopAccessListener = document?.onGuestAccessClosed(closeAccess);
    let expiryTimer: ReturnType<typeof setTimeout> | undefined;
    if (guest)
      expiryTimer = setTimeout(
        closeAccess,
        Math.max(0, Math.min(2147483647, guest.expiresAt * 1000 - Date.now())),
      );
    // The instance identity is transient; cookies are HttpOnly and never read here.
    const instanceId = crypto.getRandomValues(new Uint8Array(16));
    instanceId[6] = ((instanceId[6] ?? 0) & 0x0f) | 0x40;
    instanceId[8] = ((instanceId[8] ?? 0) & 0x3f) | 0x80;

    const retry = () => {
      if (!active || accessClosed) return;
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
        if (accessClosed) return;
        response = await fetch(endpoints.session, {
          method: endpoints.method,
          credentials: 'same-origin',
          signal: AbortSignal.any([cancellation.signal, AbortSignal.timeout(5_000)]),
        });
      } catch {
        retry();
        return;
      }
      if (!active) return;
      if (response.status === 401 || response.status === 403 || response.status === 404) {
        if (guest) {
          closeAccess();
          return;
        }
        setState({ kind: 'unauthorized', label: 'Требуется авторизация' });
        if (response.status === 401) refreshAccount();
        return;
      }
      if (!response.ok) {
        retry();
        return;
      }
      if (guest) {
        try {
          const restored = decodeGuest(await boundedJson(response, 4096), guest.linkId);
          if (
            restored.documentId !== guest.documentId ||
            restored.actorId !== guest.actorId ||
            restored.permission !== guest.permission ||
            restored.admittedHeadVersion !== guest.admittedHeadVersion
          ) {
            closeAccess();
            return;
          }
        } catch {
          closeAccess();
          return;
        }
      }
      if (!active || accessClosed) return;

      const url = new URL(endpoints.websocket, window.location.href);
      url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const ws = new WebSocket(url);
      socket = ws;
      ws.binaryType = 'arraybuffer';
      let terminal = false;
      let welcomed = false;
      let helloSentAt: number | undefined;
      let documentMessages = Promise.resolve();
      const started = performance.now();
      handshakeTimer = setTimeout(() => ws.close(1000, 'Handshake timeout'), 5_000);
      ws.onopen = () => {
        if (!active) {
          ws.close();
          return;
        }
        const hello = encodeHello(instanceId);
        helloSentAt = performance.now();
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
                'replayComplete',
                'presenceSnapshot',
              ].includes(envelope.payload.case ?? '')
            )
              throw new SessionProtocolError('invalid_message');
            documentMessages = documentMessages
              .then(async () => {
                if (active) await document?.receive(envelope);
              })
              .catch(() => {
                terminal = true;
                if (guest) {
                  closeAccess();
                  return;
                }
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
          setState({
            kind: 'online',
            label: 'Подключено',
            latencyMs:
              helloSentAt === undefined ? undefined : Math.round(performance.now() - helloSentAt),
          });
        } catch (error) {
          if (error instanceof SessionProtocolError && error.code === 'server_busy') {
            ws.close(1000, 'Retry later');
            return;
          }
          terminal = true;
          const unauthorized =
            error instanceof SessionProtocolError && error.code === 'auth_required';
          if (guest && unauthorized) {
            closeAccess();
            return;
          }
          if (unauthorized) refreshAccount();
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
      ws.onclose = (event) => {
        if (guest && event.code === 4003) {
          terminal = true;
          closeAccess();
          return;
        }
        document?.disconnected();
        clearTimeout(handshakeTimer);
        if (!active || terminal || accessClosed) return;
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
      clearTimeout(expiryTimer);
      stopAccessListener?.();
      socket?.close(1000, 'Client closed');
    };
  }, [generation, document, guest]);

  const reconnect = () => {
    if (document?.guestAccessClosed) return;
    setState(CONNECTING);
    setGeneration((previous) => previous + 1);
  };
  return { state, reconnect };
}
