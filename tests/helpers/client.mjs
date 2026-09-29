import assert from 'node:assert/strict';
import { afterEach } from 'node:test';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import WebSocket from 'ws';

const require = createRequire(resolve('apps/web/package.json'));
const { create, createFileRegistry, fromBinary, toBinary } = require('@bufbuild/protobuf');
const { FileDescriptorSetSchema } = require('@bufbuild/protobuf/wkt');
const registry = createFileRegistry(
  fromBinary(FileDescriptorSetSchema, readFileSync('proto/wire-descriptor.bin')),
);
const clientSchema = registry.getMessage('pxel.wire.v1.ClientEnvelope');
const serverSchema = registry.getMessage('pxel.wire.v1.ServerEnvelope');
export const origin = 'http://127.0.0.1:5173';
const issuedSessions = new Map();
export async function releaseSessions(base) {
  for (const [cookie, server] of issuedSessions) {
    if (base !== undefined && server !== base) continue;
    const response = await fetch(server + '/api/dev/session', {
      method: 'DELETE',
      headers: { Origin: origin, Cookie: cookie },
      signal: AbortSignal.timeout(5000),
    });
    assert.equal(response.status, 204);
    issuedSessions.delete(cookie);
  }
}
afterEach(() => releaseSessions());

export function uuid7() {
  const id = randomBytes(16);
  id.writeUIntBE(Date.now(), 0, 6);
  id[6] = (id[6] & 15) | 112;
  id[8] = (id[8] & 63) | 128;
  const hex = id.toString('hex');
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join('-');
}
export const bytes = (id) => Buffer.from(id.replaceAll('-', ''), 'hex');
export async function login(base, account = 'local', workspace) {
  const response = await fetch(base + '/api/dev/session', {
    method: 'POST',
    headers: {
      Origin: origin,
      'X-PXEL-DEV-ACCOUNT': account,
      ...(workspace ? { 'X-PXEL-DEV-WORKSPACE': workspace } : {}),
    },
    signal: AbortSignal.timeout(5000),
  });
  assert.equal(response.status, 204);
  const cookie = response.headers.get('set-cookie').split(';')[0];
  issuedSessions.set(cookie, base);
  return cookie;
}
export function api(base, cookie, path, method = 'GET', body) {
  return fetch(base + path, {
    method,
    headers: { Origin: origin, Cookie: cookie, 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  });
}
export async function createDocument(base, cookie) {
  const intent = { document_id: uuid7(), operation_id: uuid7() };
  const response = await api(base, cookie, '/api/documents', 'POST', intent);
  assert.equal(response.status, 201);
  const snapshot = await getDocument(base, cookie, intent.document_id);
  return { intent, snapshot };
}
export async function getDocument(base, cookie, id) {
  const response = await api(base, cookie, '/api/documents/' + id);
  assert.equal(response.status, 200);
  return response.json();
}
function receive(socket) {
  return once(socket, 'message', { signal: AbortSignal.timeout(15000) }).then(([data, binary]) => {
    assert.equal(binary, true);
    const message = fromBinary(serverSchema, data);
    assert.equal(message.protocolVersion, 1);
    return message.payload;
  });
}
export const nextFrame = (client) => receive(client.socket);
export function decodeFrame(data) {
  const envelope = fromBinary(serverSchema, data);
  assert.equal(envelope.protocolVersion, 1);
  return envelope.payload;
}
export function receiveFrames(client, count, action) {
  return new Promise((resolve, reject) => {
    const frames = [];
    const timer = setTimeout(() => finish(new Error('Document frames timed out')), 15000);
    const onMessage = (data, binary) => {
      try {
        assert.equal(binary, true);
        frames.push(decodeFrame(data));
        if (frames.length === count) finish();
      } catch (error) {
        finish(error);
      }
    };
    function finish(error) {
      clearTimeout(timer);
      client.socket.off('message', onMessage);
      if (error) reject(error);
      else resolve(frames);
    }
    client.socket.on('message', onMessage);
    try {
      action();
    } catch (error) {
      finish(error);
    }
  });
}
export function subscribeDocument(client, documentId, lastSeenVersion) {
  client.socket.send(
    toBinary(
      clientSchema,
      create(clientSchema, {
        protocolVersion: 1,
        payload: {
          case: 'subscribeDocument',
          value: { documentId: bytes(documentId), lastSeenVersion: BigInt(lastSeenVersion) },
        },
      }),
    ),
  );
}
export function sendPresence(client, snapshot, row = 0, column = 0) {
  const sheet = snapshot.sheets[0];
  const address = {
    sheetId: bytes(sheet.id),
    rowId: bytes(sheet.rows[row]),
    columnId: bytes(sheet.columns[column]),
  };
  client.socket.send(
    toBinary(
      clientSchema,
      create(clientSchema, {
        protocolVersion: 1,
        payload: {
          case: 'setPresence',
          value: { documentId: bytes(snapshot.document_id), anchor: address, focus: address },
        },
      }),
    ),
  );
}
export async function connect(base, cookie) {
  const socket = new WebSocket(base.replace(/^http/, 'ws') + '/api/ws', {
    headers: { Origin: origin, Cookie: cookie },
    handshakeTimeout: 5000,
  });
  await once(socket, 'open', { signal: AbortSignal.timeout(5000) });
  const pending = receive(socket);
  socket.send(
    toBinary(
      clientSchema,
      create(clientSchema, {
        protocolVersion: 1,
        payload: {
          case: 'hello',
          value: { clientInstanceId: bytes(uuid7()), clientName: 'pxel-test' },
        },
      }),
    ),
  );
  const welcome = await pending;
  assert.equal(welcome.case, 'welcome');
  return {
    socket,
    actorId: welcome.value.actorId,
    async send(command) {
      const response = receive(socket);
      socket.send(
        toBinary(
          clientSchema,
          create(clientSchema, {
            protocolVersion: 1,
            payload: { case: 'command', value: command },
          }),
        ),
      );
      return response;
    },
  };
}
export function setCommand(
  client,
  snapshot,
  value,
  { row = 0, column = 0, sequence = 1, operationId = uuid7() } = {},
) {
  const sheet = snapshot.sheets[0];
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'setCellValue',
        value: {
          address: {
            sheetId: bytes(sheet.id),
            rowId: bytes(sheet.rows[row]),
            columnId: bytes(sheet.columns[column]),
          },
          value: {
            kind:
              typeof value === 'number'
                ? { case: 'number', value }
                : typeof value === 'boolean'
                  ? { case: 'boolean', value }
                  : { case: 'text', value },
          },
        },
      },
    },
  };
}

export function undoCellCommand(
  client,
  snapshot,
  targetOperationId,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    undoCell: { targetOperationId },
  };
}

export function copyRangeCommand(
  client,
  snapshot,
  { sourceRow = 0, sourceColumn = 0, destinationRow, destinationColumn, rows, columns },
  { sequence = 1, operationId = uuid7() } = {},
) {
  const sheet = snapshot.sheets[0];
  const cell = (row, column) => ({
    sheetId: bytes(sheet.id),
    rowId: bytes(sheet.rows[row]),
    columnId: bytes(sheet.columns[column]),
  });
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    copyRange: {
      source: cell(sourceRow, sourceColumn),
      destination: cell(destinationRow, destinationColumn),
      rows,
      columns,
    },
  };
}

export function batchValuesCommand(
  client,
  snapshot,
  writes,
  { sequence = 1, operationId = uuid7() } = {},
) {
  const sheet = snapshot.sheets[0];
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'setCellValues',
        value: {
          writes: writes.map(({ row, column, value }) => ({
            address: {
              sheetId: bytes(sheet.id),
              rowId: bytes(sheet.rows[row]),
              columnId: bytes(sheet.columns[column]),
            },
            value: {
              kind:
                typeof value === 'number'
                  ? { case: 'number', value }
                  : typeof value === 'boolean'
                    ? { case: 'boolean', value }
                    : { case: 'text', value },
            },
          })),
        },
      },
    },
  };
}

export function extendRowsCommand(
  client,
  snapshot,
  rowIds,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'extendRows',
        value: { sheetId: bytes(snapshot.sheets[0].id), rows: rowIds.map(bytes) },
      },
    },
  };
}

export function insertRowsCommand(
  client,
  snapshot,
  beforeRowId,
  rowIds,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'insertRows',
        value: {
          sheetId: bytes(snapshot.sheets[0].id),
          beforeRowId: bytes(beforeRowId),
          rows: rowIds.map(bytes),
        },
      },
    },
  };
}

export function deleteRowsCommand(
  client,
  snapshot,
  rowIds,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'deleteRows',
        value: { sheetId: bytes(snapshot.sheets[0].id), rows: rowIds.map(bytes) },
      },
    },
  };
}

export function insertColumnsCommand(
  client,
  snapshot,
  beforeColumnId,
  columnIds,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'insertColumns',
        value: {
          sheetId: bytes(snapshot.sheets[0].id),
          beforeColumnId: bytes(beforeColumnId),
          columns: columnIds.map(bytes),
        },
      },
    },
  };
}

export function deleteColumnsCommand(
  client,
  snapshot,
  columnIds,
  { sequence = 1, operationId = uuid7() } = {},
) {
  return {
    operationId: bytes(operationId),
    documentId: bytes(snapshot.document_id),
    actorId: client.actorId,
    baseVersion: BigInt(snapshot.version),
    clientSequence: BigInt(sequence),
    operation: {
      kind: {
        case: 'deleteColumns',
        value: { sheetId: bytes(snapshot.sheets[0].id), columns: columnIds.map(bytes) },
      },
    },
  };
}

export function formulaCommand(client, snapshot, source, options = {}) {
  const command = setCommand(client, snapshot, 0, options);
  const address = command.operation.kind.value.address;
  delete command.operation;
  command.formulaCommand = { address, source };
  return command;
}

export async function calculation(base, cookie, documentId, version) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const response = await api(base, cookie, `/api/documents/${documentId}/calculation`);
    assert.ok(
      response.status === 200 || response.status === 202,
      `Unexpected calculation status ${response.status}`,
    );
    const result = await response.json();
    if (response.status === 200 && result.input_version === String(version)) return result;
    await new Promise((resolve) => setTimeout(resolve, 30));
  }
  throw new Error('Calculation version was not published within deadline');
}

export function requestCalculation(client, documentId, version) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    const cleanup = () => {
      clearTimeout(timer);
      client.socket.off('message', message);
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error('Calculation stream timed out'));
    }, 20000);
    const message = (data, binary) => {
      try {
        assert.equal(binary, true);
        assert.ok(data.byteLength <= 256 * 1024);
        const envelope = fromBinary(serverSchema, data);
        assert.equal(envelope.protocolVersion, 1);
        if (envelope.payload.case === 'error') {
          cleanup();
          resolve({ error: envelope.payload.value });
          return;
        }
        assert.equal(envelope.payload.case, 'calculationCommitted');
        const chunk = envelope.payload.value;
        chunks.push(chunk);
        if (chunk.isFinal) {
          cleanup();
          resolve({ chunks });
        }
      } catch (error) {
        cleanup();
        reject(error);
      }
    };
    client.socket.on('message', message);
    client.socket.send(
      toBinary(
        clientSchema,
        create(clientSchema, {
          protocolVersion: 1,
          payload: {
            case: 'requestCalculation',
            value: { documentId: bytes(documentId), inputVersion: BigInt(version) },
          },
        }),
      ),
    );
  });
}
