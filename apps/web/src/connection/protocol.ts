import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { ClientEnvelopeSchema, ServerEnvelopeSchema } from '../generated/pxel/wire/v1/session_pb';
import type { ServerWelcome } from '../generated/pxel/wire/v1/session_pb';

export const PROTOCOL_VERSION = 1;
export const MAX_SERVER_MESSAGE_BYTES = 1_048_576;

export class SessionProtocolError extends Error {
  constructor(
    readonly code: 'invalid_message' | 'unsupported_protocol' | 'auth_required' | 'server_busy',
  ) {
    super(code);
    this.name = 'SessionProtocolError';
  }
}

export function encodeHello(clientInstanceId: Uint8Array): Uint8Array {
  if (clientInstanceId.length !== 16) throw new SessionProtocolError('invalid_message');
  return toBinary(
    ClientEnvelopeSchema,
    create(ClientEnvelopeSchema, {
      protocolVersion: PROTOCOL_VERSION,
      payload: {
        case: 'hello',
        value: { clientInstanceId, clientName: 'pxel-web' },
      },
    }),
  );
}

export function decodeWelcome(bytes: Uint8Array): ServerWelcome {
  if (bytes.length > MAX_SERVER_MESSAGE_BYTES) throw new SessionProtocolError('invalid_message');
  let envelope;
  try {
    envelope = fromBinary(ServerEnvelopeSchema, bytes);
  } catch {
    throw new SessionProtocolError('invalid_message');
  }
  if (envelope.protocolVersion !== PROTOCOL_VERSION)
    throw new SessionProtocolError('unsupported_protocol');
  if (envelope.payload.case === 'error') {
    const code = envelope.payload.value.code;
    if (code === 1) throw new SessionProtocolError('unsupported_protocol');
    if (code === 3) throw new SessionProtocolError('auth_required');
    if (code === 4) throw new SessionProtocolError('server_busy');
    throw new SessionProtocolError('invalid_message');
  }
  if (envelope.payload.case !== 'welcome') throw new SessionProtocolError('invalid_message');
  const welcome = envelope.payload.value;
  if (
    welcome.sessionId.length !== 16 ||
    welcome.actorId.length !== 16 ||
    welcome.heartbeatIntervalMs === 0 ||
    welcome.heartbeatIntervalMs > 120_000 ||
    welcome.maxMessageBytes === 0 ||
    welcome.maxMessageBytes > MAX_SERVER_MESSAGE_BYTES
  )
    throw new SessionProtocolError('invalid_message');
  return welcome;
}

export function reconnectDelay(attempt: number): number {
  return Math.min(30_000, 1_000 * 2 ** Math.max(0, attempt));
}
