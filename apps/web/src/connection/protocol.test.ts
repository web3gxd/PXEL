import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import { ClientEnvelopeSchema, ServerEnvelopeSchema } from '../generated/pxel/wire/v1/session_pb';
import { decodeWelcome, encodeHello, MAX_SERVER_MESSAGE_BYTES, reconnectDelay } from './protocol';

const id = Uint8Array.from({ length: 16 }, (_, index) => index + 1);

function welcomeBytes(protocolVersion = 1, actorId = id): Uint8Array {
  return toBinary(
    ServerEnvelopeSchema,
    create(ServerEnvelopeSchema, {
      protocolVersion,
      payload: {
        case: 'welcome',
        value: {
          sessionId: id,
          actorId,
          heartbeatIntervalMs: 15_000,
          maxMessageBytes: 1_048_576,
        },
      },
    }),
  );
}

describe('versioned session boundary', () => {
  it('matches the shared Rust Hello v1 golden bytes', () => {
    expect(Array.from(encodeHello(new Uint8Array(16).fill(1)))).toEqual([
      8, 1, 18, 28, 10, 16, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 18, 8, 112, 120, 101,
      108, 45, 119, 101, 98,
    ]);
  });

  it('encodes a binary hello with its exact identity and name', () => {
    const decoded = fromBinary(ClientEnvelopeSchema, encodeHello(id));
    expect(decoded.protocolVersion).toBe(1);
    expect(decoded.payload.case).toBe('hello');
    if (decoded.payload.case !== 'hello') throw new Error('Hello expected');
    expect(decoded.payload.value.clientInstanceId).toEqual(id);
    expect(decoded.payload.value.clientName).toBe('pxel-web');
  });

  it('validates a welcome rather than treating WS open as authenticated', () => {
    const welcome = decodeWelcome(welcomeBytes());
    expect(welcome.actorId).toEqual(id);
    expect(welcome.heartbeatIntervalMs).toBe(15_000);
  });

  it('rejects unknown versions, missing payloads, malformed bytes and invalid identities', () => {
    expect(() => decodeWelcome(welcomeBytes(2))).toThrow('unsupported_protocol');
    expect(() => decodeWelcome(welcomeBytes(1, new Uint8Array(15)))).toThrow('invalid_message');
    expect(() => encodeHello(new Uint8Array(17))).toThrow('invalid_message');
    expect(() => decodeWelcome(new Uint8Array([8, 1]))).toThrow('invalid_message');
    expect(() => decodeWelcome(new Uint8Array([255]))).toThrow('invalid_message');
    expect(() => decodeWelcome(new Uint8Array(MAX_SERVER_MESSAGE_BYTES + 1))).toThrow(
      'invalid_message',
    );
  });

  it('preserves auth rejection as a controlled error', () => {
    const message = toBinary(
      ServerEnvelopeSchema,
      create(ServerEnvelopeSchema, {
        protocolVersion: 1,
        payload: {
          case: 'error',
          value: { code: 3, supportedProtocolVersion: 1 },
        },
      }),
    );
    expect(() => decodeWelcome(message)).toThrow('auth_required');
  });
});

it('bounds exponential reconnect delay', () => {
  expect([0, 1, 2, 5, 10].map(reconnectDelay)).toEqual([1_000, 2_000, 4_000, 30_000, 30_000]);
});
