import assert from 'node:assert/strict';
import { beforeEach, test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import {
  login,
  createDocument,
  connect,
  getDocument,
  setCommand,
  formulaCommand,
  moveRangeCommand,
  undoCellCommand,
  calculation,
  subscribeDocument,
  receiveFrames,
  deleteRowsCommand,
} from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
beforeEach(async () => delay(1100));
function cell(book, row, column) {
  const sheet = book.sheets[0];
  return sheet.cells.find(
    (c) => c.row_id === sheet.rows[row] && c.column_id === sheet.columns[column],
  );
}
async function edited(client, cookie, book, command) {
  const response = await client.send(command);
  assert.equal(response.case, 'committed');
  return getDocument(base, cookie, book.document_id);
}

test('move freezes overlapping cells, rewrites inbound formulas, persists and deduplicates', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  const observer = await connect(base, cookie);
  try {
    book = await edited(client, cookie, book, setCommand(client, book, 7));
    book = await edited(
      client,
      cookie,
      book,
      formulaCommand(client, book, '=A1', { column: 1, sequence: 2 }),
    );
    book = await edited(
      client,
      cookie,
      book,
      formulaCommand(client, book, '=$A$1+$B$1', { row: 1, sequence: 3 }),
    );
    await receiveFrames(observer, 1, () =>
      subscribeDocument(observer, book.document_id, book.version),
    );
    const command = moveRangeCommand(
      client,
      book,
      { destinationRow: 0, destinationColumn: 1, rows: 1, columns: 2 },
      { sequence: 4 },
    );
    const head = receiveFrames(observer, 1, () => {});
    const ack = await client.send(command);
    assert.equal(ack.case, 'committed');
    assert.equal(ack.value.canonicalOperation, undefined);
    const [notification] = await head;
    assert.equal(notification.case, 'documentHeadChanged');
    assert.equal(notification.value.headVersion, ack.value.committedVersion);
    const moved = await getDocument(base, cookie, book.document_id);
    assert.equal(moved.state_hash, Buffer.from(ack.value.confirmedStateHash).toString('hex'));
    assert.equal(cell(moved, 0, 0), undefined);
    assert.deepEqual(cell(moved, 0, 1)?.value, { type: 'number', value: 7 });
    assert.equal(cell(moved, 0, 2)?.value.type, 'formula');
    assert.equal(cell(moved, 0, 2)?.value.value.current_text, '=B1');
    assert.equal(cell(moved, 1, 0)?.value.value.current_text, '=($B$1+$C$1)');
    const calculated = await calculation(base, cookie, book.document_id, moved.version);
    assert.equal(calculated.status, 'complete');
    assert.equal(calculated.values.length, 2);
    assert.equal(
      calculated.values.find((v) => v.column_id === moved.sheets[0].columns[2])?.value.value,
      7,
    );
    assert.equal(
      calculated.values.find((v) => v.row_id === moved.sheets[0].rows[1])?.value.value,
      14,
    );
    const retry = await client.send(command);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, ack.value.committedVersion);
    assert.equal((await getDocument(base, cookie, book.document_id)).state_hash, moved.state_hash);
    const inverse = await client.send(
      undoCellCommand(client, moved, command.operationId, { sequence: 5 }),
    );
    assert.equal(inverse.case, 'rejected');
    assert.equal(inverse.value.code, 14);
  } finally {
    client.socket.terminate();
    observer.socket.terminate();
  }
});

test('stale source and partial range dependencies reject without a committed version', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const stale = moveRangeCommand(client, book, {
      destinationRow: 2,
      destinationColumn: 0,
      rows: 1,
      columns: 1,
    });
    book = await edited(
      client,
      cookie,
      book,
      formulaCommand(client, book, '=SUM(A1:C1)', { row: 1, column: 3, sequence: 2 }),
    );
    const before = book.state_hash;
    const denied = await client.send(stale);
    assert.equal(denied.case, 'rejected');
    assert.equal(denied.value.code, 17);
    const partial = await client.send(
      moveRangeCommand(
        client,
        book,
        { destinationRow: 2, destinationColumn: 0, rows: 1, columns: 1 },
        { sequence: 3 },
      ),
    );
    assert.equal(partial.case, 'rejected');
    assert.equal(partial.value.code, 18);
    const after = await getDocument(base, cookie, book.document_id);
    assert.equal(after.version, book.version);
    assert.equal(after.state_hash, before);
  } finally {
    client.socket.terminate();
  }
});

test('moves invalidate delayed formula binding and historical formula restoration', async () => {
  const cookie = await login(base);
  let { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    book = await edited(client, cookie, book, setCommand(client, book, 5));
    book = await edited(
      client,
      cookie,
      book,
      formulaCommand(client, book, '=A1', { row: 1, column: 1, sequence: 2 }),
    );
    const overwritten = setCommand(client, book, 11, { row: 1, column: 1, sequence: 3 });
    book = await edited(client, cookie, book, overwritten);
    const delayed = formulaCommand(client, book, '=A1', { row: 2, column: 1, sequence: 4 });
    book = await edited(
      client,
      cookie,
      book,
      moveRangeCommand(
        client,
        book,
        { destinationRow: 0, destinationColumn: 2, rows: 1, columns: 1 },
        { sequence: 5 },
      ),
    );
    const before = book.state_hash;
    const formula = await client.send(delayed);
    assert.equal(formula.case, 'rejected');
    assert.equal(formula.value.code, 17);
    const undo = await client.send(
      undoCellCommand(client, book, overwritten.operationId, { sequence: 6 }),
    );
    assert.equal(undo.case, 'rejected');
    assert.equal(undo.value.code, 14);
    assert.equal((await getDocument(base, cookie, book.document_id)).state_hash, before);
  } finally {
    client.socket.terminate();
  }
});

test('move command never partially applies when a second payload is supplied', async () => {
  const cookie = await login(base);
  const { snapshot: book } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const command = moveRangeCommand(client, book, {
      destinationRow: 0,
      destinationColumn: 1,
      rows: 1,
      columns: 1,
    });
    command.operation = setCommand(client, book, 99).operation;
    const response = await client.send(command);
    assert.equal(response.case, 'rejected');
    assert.equal(response.value.code, 3);
    assert.equal((await getDocument(base, cookie, book.document_id)).state_hash, book.state_hash);
    for (const rows of [0, 1001, 0xffffffff]) {
      const malformed = moveRangeCommand(client, book, {
        destinationRow: 0,
        destinationColumn: 1,
        rows,
        columns: 1,
      });
      const rejected = await client.send(malformed);
      assert.equal(rejected.case, 'rejected');
      assert.equal(rejected.value.code, 3);
    }
  } finally {
    client.socket.terminate();
  }
});

test('a move with a deleted stable source address receives a typed target rejection', async () => {
  const cookie = await login(base);
  const { snapshot: original } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const deleted = await edited(
      client,
      cookie,
      original,
      deleteRowsCommand(client, original, [original.sheets[0].rows[0]]),
    );
    const withOldAxes = { ...original, version: deleted.version };
    const move = moveRangeCommand(
      client,
      withOldAxes,
      { destinationRow: 1, destinationColumn: 0, rows: 1, columns: 1 },
      { sequence: 2 },
    );
    const rejected = await client.send(move);
    assert.equal(rejected.case, 'rejected');
    assert.equal(rejected.value.code, 9);
    assert.equal(
      (await getDocument(base, cookie, original.document_id)).state_hash,
      deleted.state_hash,
    );
  } finally {
    client.socket.terminate();
  }
});
