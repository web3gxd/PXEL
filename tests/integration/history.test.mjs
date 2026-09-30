import assert from 'node:assert/strict';
import { beforeEach, test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import {
  api,
  login,
  createDocument,
  connect,
  getDocument,
  setCommand,
  formulaCommand,
  undoCellCommand,
  uuid7,
  subscribeDocument,
  receiveFrames,
} from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
beforeEach(async () => delay(1100));
const uuid = (value) =>
  Buffer.from(value)
    .toString('hex')
    .replace(/^(\w{8})(\w{4})(\w{4})(\w{4})(\w{12})$/, '$1-$2-$3-$4-$5');
const target = (command, version) => ({
  operation_id: uuid(command.operationId),
  committed_version: String(version),
});

async function history(cookie, book) {
  const response = await api(base, cookie, `/api/documents/${book.document_id}/undo-history`);
  assert.equal(response.status, 200);
  const text = await response.text();
  assert.ok(Buffer.byteLength(text) < 65536);
  const decoded = JSON.parse(text);
  assert.equal(decoded.schema_version, 1);
  assert.equal(decoded.document_id, book.document_id);
  assert.deepEqual(Object.keys(decoded).sort(), [
    'actor_id',
    'document_id',
    'earliest_scanned_version',
    'redo',
    'schema_version',
    'through_version',
    'truncated',
    'truncation_reason',
    'undo',
  ]);
  for (const ref of [...decoded.undo, ...decoded.redo]) {
    assert.deepEqual(Object.keys(ref).sort(), ['committed_version', 'operation_id']);
  }
  return { decoded, text };
}

test('durable history DTO contains only own verified references and exact retry adds no target', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const first = setCommand(client, book, 'PXEL_history_content_canary');
    assert.equal((await client.send(first)).case, 'committed');
    book = await getDocument(base, cookie, book.document_id);
    const second = formulaCommand(client, book, '=A1+1', { column: 1, sequence: 2 });
    assert.equal((await client.send(second)).case, 'committed');
    book = await getDocument(base, cookie, book.document_id);
    const before = await history(cookie, book);
    assert.equal(before.decoded.actor_id, uuid(client.actorId));
    assert.equal(before.decoded.through_version, book.version);
    assert.equal(before.decoded.earliest_scanned_version, '1');
    assert.equal(before.decoded.truncated, false);
    assert.equal(before.decoded.truncation_reason, null);
    assert.deepEqual(before.decoded.undo, [target(first, 2), target(second, 3)]);
    assert.deepEqual(before.decoded.redo, []);
    assert.equal(before.text.includes('PXEL_history_content_canary'), false);
    assert.equal(before.text.includes('=A1+1'), false);
    assert.equal((await client.send(second)).case, 'receipt');
    assert.deepEqual((await history(cookie, book)).decoded, before.decoded);
    const wrongActorQuery = await api(
      base,
      cookie,
      `/api/documents/${book.document_id}/undo-history?actor_id=${uuid7()}`,
    );
    assert.equal(wrongActorQuery.status, 400);
  } finally {
    client.socket.terminate();
  }
});

test('nonlinear author history preserves independent targets across undo redo and branching', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  async function edit(command) {
    const result = await client.send(command);
    assert.equal(result.case, 'committed');
    book = await getDocument(base, cookie, book.document_id);
  }
  try {
    const a = setCommand(client, book, 7);
    await edit(a);
    const b = setCommand(client, book, 9, { column: 1, sequence: 2 });
    await edit(b);
    const ua = undoCellCommand(client, book, a.operationId, { sequence: 3 });
    await edit(ua);
    let result = (await history(cookie, book)).decoded;
    assert.deepEqual(result.undo, [target(b, 3)]);
    assert.deepEqual(result.redo, [target(ua, 4)]);
    const ub = undoCellCommand(client, book, b.operationId, { sequence: 4 });
    await edit(ub);
    result = (await history(cookie, book)).decoded;
    assert.deepEqual(result.undo, []);
    assert.deepEqual(result.redo, [target(ua, 4), target(ub, 5)]);
    const ra = undoCellCommand(client, book, ua.operationId, { sequence: 5 });
    await edit(ra);
    result = (await history(cookie, book)).decoded;
    assert.deepEqual(result.undo, [target(ra, 6)]);
    assert.deepEqual(result.redo, [target(ub, 5)]);
    const branch = setCommand(client, book, 11, { column: 2, sequence: 6 });
    await edit(branch);
    result = (await history(cookie, book)).decoded;
    assert.deepEqual(result.undo, [target(ra, 6), target(branch, 7)]);
    assert.deepEqual(result.redo, []);
  } finally {
    client.socket.terminate();
  }
});

test('history is editor-scoped and never exposes another actors targets', async () => {
  const suffix = uuid7().replaceAll('-', '').slice(-16);
  const owner = await login(base, 'history_owner_' + suffix);
  const guestName = 'history_guest_' + suffix;
  const guestHome = await login(base, guestName);
  const ownerIdentity = await (await api(base, owner, '/api/dev/identity')).json();
  const guestIdentity = await (await api(base, guestHome, '/api/dev/identity')).json();
  let { snapshot: book } = await createDocument(base, owner);
  const path = `/api/documents/${book.document_id}`;
  assert.ok([403, 404].includes((await api(base, guestHome, path + '/undo-history')).status));
  const grant = async (role) => {
    assert.equal(
      (
        await api(base, owner, path + '/permissions', 'POST', {
          action: 'grant',
          change_id: uuid7(),
          target_user_id: guestIdentity.actor_id,
          role,
        })
      ).status,
      200,
    );
  };
  await grant('viewer');
  const guest = await login(base, guestName, ownerIdentity.workspace_id);
  assert.equal((await api(base, guest, path + '/undo-history')).status, 403);
  await grant('editor');
  const writer = await connect(base, owner);
  const other = await connect(base, guest);
  try {
    const original = setCommand(writer, book, 7);
    assert.equal((await writer.send(original)).case, 'committed');
    book = await getDocument(base, owner, book.document_id);
    const foreign = setCommand(other, book, 7);
    assert.equal((await other.send(foreign)).case, 'committed');
    book = await getDocument(base, owner, book.document_id);
    assert.deepEqual((await history(owner, book)).decoded.undo, [target(original, 2)]);
    const guestHistory = (await history(guest, book)).decoded;
    assert.equal(guestHistory.actor_id, guestIdentity.actor_id);
    assert.deepEqual(guestHistory.undo, [target(foreign, 3)]);
    const inverse = await writer.send(
      undoCellCommand(writer, book, original.operationId, { sequence: 2 }),
    );
    assert.equal(inverse.case, 'rejected');
    assert.equal((await getDocument(base, owner, book.document_id)).state_hash, book.state_hash);
    assert.deepEqual((await history(owner, book)).decoded.undo, [target(original, 2)]);
    assert.equal(
      (
        await api(base, owner, path + '/permissions', 'POST', {
          action: 'revoke',
          change_id: uuid7(),
          target_user_id: guestIdentity.actor_id,
        })
      ).status,
      200,
    );
    assert.ok([403, 404].includes((await api(base, guest, path + '/undo-history')).status));
  } finally {
    writer.socket.terminate();
    other.socket.terminate();
  }
});

test('system head hints suppress author discovery without becoming history barriers', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  const observer = await connect(base, cookie);
  try {
    const original = setCommand(writer, book, 4);
    assert.equal((await writer.send(original)).case, 'committed');
    book = await getDocument(base, cookie, book.document_id);
    const [replay] = await receiveFrames(observer, 1, () =>
      subscribeDocument(observer, book.document_id, book.version),
    );
    assert.equal(replay.case, 'replayComplete');
    const notification = receiveFrames(observer, 1, () => {});
    const reply = await api(
      base,
      cookie,
      `/api/documents/${book.document_id}/recalculate`,
      'POST',
      { operation_id: uuid7() },
    );
    assert.equal(reply.status, 200);
    const [frame] = await notification;
    assert.equal(frame.case, 'documentHeadChanged');
    assert.equal(frame.value.headVersion, BigInt(book.version) + 1n);
    assert.equal(frame.value.headEventKind, 2);
    assert.equal(frame.value.headActorId.length, 0);
    book = await getDocument(base, cookie, book.document_id);
    const result = (await history(cookie, book)).decoded;
    assert.equal(result.through_version, book.version);
    assert.deepEqual(result.undo, [target(original, 2)]);
  } finally {
    writer.socket.terminate();
    observer.socket.terminate();
  }
});
