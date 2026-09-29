import assert from 'node:assert/strict';
import { once } from 'node:events';
import { test } from 'node:test';
import {
  api,
  addSheetCommand,
  batchValuesCommand,
  bytes,
  decodeFrame,
  connect,
  copyRangeCommand,
  createDocument,
  deleteColumnsCommand,
  deleteRowsCommand,
  extendRowsCommand,
  insertColumnsCommand,
  insertRowsCommand,
  formulaCommand,
  calculation,
  requestCalculation,
  getDocument,
  login,
  nextFrame,
  receiveFrames,
  setCommand,
  undoCellCommand,
  sendPresence,
  subscribeDocument,
  uuid7,
} from '../helpers/client.mjs';
const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';

test('ABS and MOD commit and recalculate after a referenced value changes', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal((await client.send(setCommand(client, snapshot, -3))).case, 'committed');
    const formula = formulaCommand(client, { ...snapshot, version: '2' }, '=ABS(MOD(A1,2))', {
      column: 1,
      sequence: 2,
    });
    assert.equal((await client.send(formula)).case, 'committed');
    assert.deepEqual((await calculation(base, cookie, snapshot.document_id, 3)).values[0].value, {
      type: 'number',
      value: 1,
    });
    assert.equal(
      (await client.send(setCommand(client, { ...snapshot, version: '3' }, 4, { sequence: 3 })))
        .case,
      'committed',
    );
    const calculated = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(calculated.values[0].value, { type: 'number', value: 0 });
    assert.equal(calculated.engine_version, 'pxel-formula-0.3.0');
    assert.equal((await client.send(formula)).case, 'receipt');
  } finally {
    client.socket.terminate();
  }
});

test('AddSheet is durable, idempotent and rejects a duplicate sheet name', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const command = addSheetCommand(client, snapshot);
    const committed = await client.send(command);
    assert.equal(committed.case, 'committed');
    assert.equal(committed.value.committedVersion, 2n);
    assert.equal(committed.value.canonicalOperation.kind.case, 'addSheet');
    const after = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(after.version, '2');
    assert.equal(after.sheets.length, 2);
    assert.equal(after.sheets[1].name, 'Лист 2');
    assert.equal(after.sheets[1].rows.length, 1);
    assert.equal(after.sheets[1].columns.length, 1);
    const receipt = await client.send(command);
    assert.equal(receipt.case, 'receipt');
    assert.equal(receipt.value.committedVersion, 2n);
    const duplicate = await client.send(addSheetCommand(client, after, { sequence: 2 }));
    assert.equal(duplicate.case, 'rejected');
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '2');
    const replay = await connect(base, cookie);
    try {
      const frames = await receiveFrames(replay, 2, () =>
        subscribeDocument(replay, snapshot.document_id, 1),
      );
      assert.deepEqual(
        frames.map((frame) => frame.case),
        ['documentHeadChanged', 'replayComplete'],
      );
      assert.equal(frames[0].value.latestOperation.canonicalOperation.kind.case, 'addSheet');
    } finally {
      replay.socket.terminate();
    }
    const strangerCookie = await login(base, `sheet-stranger-${uuid7().slice(0, 8)}`);
    const stranger = await connect(base, strangerCookie);
    try {
      const denied = await stranger.send(addSheetCommand(stranger, snapshot));
      assert.equal(denied.case, 'rejected');
      assert.equal(denied.value.code, 2);
    } finally {
      stranger.socket.terminate();
    }
  } finally {
    client.socket.terminate();
  }
});

test('historical HTTP read is exact, authorized and does not change head', async () => {
  const owner = await login(base);
  const stranger = await login(base, `history-stranger-${uuid7().slice(0, 8)}`);
  const { snapshot } = await createDocument(base, owner);
  const client = await connect(base, owner);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 17, { sequence: 1 }))).case,
      'committed',
    );
    const path = `/api/documents/${snapshot.document_id}/versions/`;
    const original = await api(base, owner, path + '1');
    assert.equal(original.status, 200);
    const atOne = await original.json();
    assert.equal(atOne.version, '1');
    assert.equal(atOne.state_hash, snapshot.state_hash);
    assert.deepEqual(atOne.sheets[0].cells, []);
    const current = await api(base, owner, path + '2');
    assert.equal(current.status, 200);
    const atTwo = await current.json();
    assert.equal(atTwo.version, '2');
    assert.equal(atTwo.sheets[0].cells[0].value.value, 17);
    assert.equal((await api(base, owner, path + '3')).status, 404);
    assert.equal((await api(base, owner, path + '0')).status, 404);
    assert.equal((await api(base, owner, path + 'invalid')).status, 400);
    // Cross-workspace reads conceal whether the document exists.
    assert.equal((await api(base, stranger, path + '1')).status, 404);
    assert.equal((await getDocument(base, owner, snapshot.document_id)).version, '2');
  } finally {
    client.socket.terminate();
  }
});

test('copy range freezes values and translated formulas in one durable version', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 3, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, { ...snapshot, version: '2' }, '=A1*2', {
            column: 1,
            sequence: 2,
          }),
        )
      ).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          setCommand(client, { ...snapshot, version: '3' }, 5, { row: 1, sequence: 3 }),
        )
      ).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, { ...snapshot, version: '4' }, '=A2+1', {
            row: 1,
            column: 1,
            sequence: 4,
          }),
        )
      ).case,
      'committed',
    );
    const atFive = await getDocument(base, cookie, snapshot.document_id);
    const copy = copyRangeCommand(
      client,
      atFive,
      {
        destinationRow: 2,
        destinationColumn: 2,
        rows: 2,
        columns: 2,
      },
      { sequence: 5 },
    );
    const commit = await client.send(copy);
    assert.equal(commit.case, 'committed');
    assert.equal(commit.value.committedVersion, 6n);
    assert.equal(commit.value.canonicalOperation, undefined);
    const loaded = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(loaded.version, '6');
    const returning = await connect(base, cookie);
    try {
      const [hint, complete] = await receiveFrames(returning, 2, () =>
        subscribeDocument(returning, snapshot.document_id, 5),
      );
      assert.equal(hint.case, 'documentHeadChanged');
      assert.equal(hint.value.headVersion, 6n);
      assert.equal(hint.value.latestOperation, undefined);
      assert.equal(complete.case, 'replayComplete');
      assert.equal(complete.value.resyncRequired, true);
    } finally {
      returning.socket.terminate();
    }
    const at = (row, column) =>
      loaded.sheets[0].cells.find(
        (cell) =>
          cell.row_id === snapshot.sheets[0].rows[row] &&
          cell.column_id === snapshot.sheets[0].columns[column],
      )?.value;
    assert.deepEqual(at(2, 2), { type: 'number', value: 3 });
    assert.equal(at(2, 3)?.value.current_text, '=(C3*2)');
    assert.deepEqual(at(3, 2), { type: 'number', value: 5 });
    assert.equal(at(3, 3)?.value.current_text, '=(C4+1)');
    const results = await calculation(base, cookie, snapshot.document_id, 6);
    assert.deepEqual(
      results.values.find(
        (cell) =>
          cell.row_id === snapshot.sheets[0].rows[2] &&
          cell.column_id === snapshot.sheets[0].columns[3],
      )?.value,
      { type: 'number', value: 6 },
    );
    assert.equal((await client.send(copy)).case, 'receipt');
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '6');
    const collision = structuredClone(copy);
    collision.copyRange.destination.rowId = bytes(snapshot.sheets[0].rows[4]);
    const conflict = await client.send(collision);
    assert.equal(conflict.case, 'rejected');
    assert.equal(conflict.value.code, 6);

    const stale = copyRangeCommand(
      client,
      atFive,
      {
        destinationRow: 4,
        destinationColumn: 4,
        rows: 1,
        columns: 1,
      },
      { sequence: 6 },
    );
    const rejected = await client.send(stale);
    assert.equal(rejected.case, 'rejected');
    assert.equal(rejected.value.code, 17);
    assert.equal(rejected.value.retryable, false);
    assert.equal(rejected.value.resyncRequired, true);
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '6');

    const overlap = copyRangeCommand(
      client,
      loaded,
      { destinationRow: 0, destinationColumn: 1, rows: 1, columns: 2 },
      { sequence: 7 },
    );
    assert.equal((await client.send(overlap)).case, 'committed');
    const overlapped = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(overlapped.version, '7');
    const sameRow = (column) =>
      overlapped.sheets[0].cells.find(
        (cell) =>
          cell.row_id === snapshot.sheets[0].rows[0] &&
          cell.column_id === snapshot.sheets[0].columns[column],
      )?.value;
    assert.deepEqual(sameRow(1), { type: 'number', value: 3 });
    assert.equal(sameRow(2)?.value.current_text, '=(B1*2)');
    const fill = setCommand(client, overlapped, 77, { row: 3, column: 4, sequence: 8 });
    assert.equal((await client.send(fill)).case, 'committed');
    const blank = copyRangeCommand(
      client,
      { ...overlapped, version: '8' },
      {
        sourceRow: 2,
        sourceColumn: 0,
        destinationRow: 3,
        destinationColumn: 4,
        rows: 1,
        columns: 1,
      },
      { sequence: 9 },
    );
    assert.equal((await client.send(blank)).case, 'committed');
    const cleared = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(cleared.version, '9');
    assert.equal(
      cleared.sheets[0].cells.some(
        (cell) =>
          cell.row_id === snapshot.sheets[0].rows[3] &&
          cell.column_id === snapshot.sheets[0].columns[4],
      ),
      false,
    );
  } finally {
    client.socket.close();
  }
});

test('value batch commits atomically in one version, replays and deduplicates', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const command = batchValuesCommand(client, snapshot, [
      { row: 0, column: 0, value: 4 },
      { row: 1, column: 1, value: 'batch' },
    ]);
    const result = await client.send(command);
    assert.equal(result.case, 'committed');
    assert.equal(result.value.committedVersion, 2n);
    assert.equal(result.value.canonicalOperation.kind.case, 'setCellValues');
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.version, '2');
    assert.equal(current.sheets[0].cells.length, 2);
    const retry = await client.send(command);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, 2n);
    const invalid = batchValuesCommand(
      client,
      current,
      [
        { row: 0, column: 0, value: 99 },
        { row: 0, column: 0, value: 100 },
      ],
      { sequence: 2 },
    );
    assert.equal((await client.send(invalid)).case, 'rejected');
    const after = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(after.version, '2');
    assert.equal(after.sheets[0].cells.length, 2);
  } finally {
    client.socket.close();
  }
});

test('value batch invalidates dependent formulas at its single committed version', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const initial = await client.send(
      batchValuesCommand(client, snapshot, [
        { row: 0, column: 0, value: 2 },
        { row: 0, column: 1, value: 3 },
      ]),
    );
    assert.equal(initial.case, 'committed');
    const formula = await client.send(
      formulaCommand(client, { ...snapshot, version: '2' }, '=A1+B1', { column: 2, sequence: 2 }),
    );
    assert.equal(formula.case, 'committed');
    const updated = await client.send(
      batchValuesCommand(
        client,
        { ...snapshot, version: '3' },
        [
          { row: 0, column: 0, value: 5 },
          { row: 0, column: 1, value: 8 },
        ],
        { sequence: 3 },
      ),
    );
    assert.equal(updated.case, 'committed');
    const result = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(result.values[0].value, { type: 'number', value: 13 });
  } finally {
    client.socket.close();
  }
});

test('presence reaches subscribers and disappears without advancing document version', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const first = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    assert.equal(
      (await receiveFrames(first, 1, () => subscribeDocument(first, snapshot.document_id, 1)))[0]
        .case,
      'replayComplete',
    );
    assert.equal(
      (await receiveFrames(second, 1, () => subscribeDocument(second, snapshot.document_id, 1)))[0]
        .case,
      'replayComplete',
    );
    const seenByFirst = nextFrame(first);
    const seenBySecond = nextFrame(second);
    sendPresence(first, snapshot);
    const [a, b] = await Promise.all([seenByFirst, seenBySecond]);
    assert.equal(a.case, 'presenceSnapshot');
    assert.equal(b.case, 'presenceSnapshot');
    assert.equal(a.value.peers.length, 2);
    assert.equal(b.value.peers.length, 2);
    assert.equal(
      Buffer.from(b.value.peers.find((peer) => peer.anchor)?.anchor.rowId).toString('hex'),
      snapshot.sheets[0].rows[0].replaceAll('-', ''),
    );
    const leave = nextFrame(second);
    first.socket.close();
    const after = await leave;
    assert.equal(after.case, 'presenceSnapshot');
    assert.equal(after.value.peers.length, 1);
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '1');
  } finally {
    first.socket.terminate();
    second.socket.terminate();
  }
});

test('another socket receives a canonical live event and late subscriber replays it', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const first = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    const [barrier] = await receiveFrames(second, 1, () =>
      subscribeDocument(second, snapshot.document_id, snapshot.version),
    );
    assert.equal(barrier.case, 'replayComplete');
    assert.equal(barrier.value.barrierVersion, 1n);
    const update = nextFrame(second);
    const committed = await first.send(setCommand(first, snapshot, 42));
    assert.equal(committed.case, 'committed');
    const hint = await update;
    assert.equal(hint.case, 'documentHeadChanged');
    assert.equal(hint.value.headVersion, 2n);
    assert.equal(hint.value.latestOperation.committedVersion, 2n);
    assert.equal(hint.value.latestOperation.canonicalOperation.kind.case, 'setCellValue');
    assert.equal(hint.value.latestOperation.canonicalOperation.kind.value.value.kind.value, 42);
    assert.equal(
      Buffer.from(hint.value.documentId).toString('hex'),
      snapshot.document_id.replaceAll('-', ''),
    );
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.version, '2');
    assert.equal(current.sheets[0].cells[0].value.value, 42);
    const returning = await connect(base, cookie);
    try {
      const [resumed, complete] = await receiveFrames(returning, 2, () =>
        subscribeDocument(returning, snapshot.document_id, 1),
      );
      assert.equal(resumed.case, 'documentHeadChanged');
      assert.equal(resumed.value.headVersion, 2n);
      assert.equal(complete.case, 'replayComplete');
      assert.equal(complete.value.barrierVersion, 2n);
      assert.equal(complete.value.resyncRequired, false);
    } finally {
      returning.socket.close();
    }
  } finally {
    first.socket.close();
    second.socket.close();
  }
});

test('concurrent writes to one cell follow server version order and replay the winner', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const first = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    const [left, right] = await Promise.all([
      first.send(setCommand(first, snapshot, 101, { sequence: 1 })),
      second.send(setCommand(second, snapshot, 202, { sequence: 1 })),
    ]);
    assert.equal(left.case, 'committed');
    assert.equal(right.case, 'committed');
    assert.deepEqual(
      [left.value.committedVersion, right.value.committedVersion].sort((a, b) => Number(a - b)),
      [2n, 3n],
    );
    const winner = left.value.committedVersion === 3n ? 101 : 202;
    const authoritative = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(authoritative.version, '3');
    assert.equal(authoritative.sheets[0].cells[0].value.value, winner);
    const returning = await connect(base, cookie);
    try {
      const [older, newer, complete] = await receiveFrames(returning, 3, () =>
        subscribeDocument(returning, snapshot.document_id, 1),
      );
      assert.equal(older.value.headVersion, 2n);
      assert.equal(newer.value.headVersion, 3n);
      assert.equal(complete.case, 'replayComplete');
      assert.equal(
        newer.value.latestOperation.canonicalOperation.kind.value.value.kind.value,
        winner,
      );
    } finally {
      returning.socket.close();
    }
  } finally {
    first.socket.close();
    second.socket.close();
  }
});

test('concurrent row extensions append stable IDs in server order and survive replay', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const first = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    const leftIds = [uuid7(), uuid7()];
    const rightIds = [uuid7(), uuid7()];
    const leftCommand = extendRowsCommand(first, snapshot, leftIds, { sequence: 1 });
    const rightCommand = extendRowsCommand(second, snapshot, rightIds, { sequence: 1 });
    const [left, right] = await Promise.all([first.send(leftCommand), second.send(rightCommand)]);
    assert.equal(left.case, 'committed');
    assert.equal(right.case, 'committed');
    assert.deepEqual(
      [left.value.committedVersion, right.value.committedVersion].sort((a, b) => Number(a - b)),
      [2n, 3n],
    );
    const expected = [
      ...snapshot.sheets[0].rows,
      ...(left.value.committedVersion === 2n ? leftIds : rightIds),
      ...(left.value.committedVersion === 3n ? leftIds : rightIds),
    ];
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.version, '3');
    assert.deepEqual(current.sheets[0].rows, expected);
    const retry = await first.send(leftCommand);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, left.value.committedVersion);
    const returning = await connect(base, cookie);
    try {
      const [older, newer, barrier] = await receiveFrames(returning, 3, () =>
        subscribeDocument(returning, snapshot.document_id, 1),
      );
      assert.equal(older.value.headVersion, 2n);
      assert.equal(newer.value.headVersion, 3n);
      assert.equal(older.value.latestOperation.canonicalOperation.kind.case, 'extendRows');
      assert.equal(newer.value.latestOperation.canonicalOperation.kind.case, 'extendRows');
      assert.equal(barrier.case, 'replayComplete');
      assert.equal(barrier.value.barrierVersion, 3n);
    } finally {
      returning.socket.close();
    }
    const delayedFormula = await first.send(
      formulaCommand(first, snapshot, '=A1', { sequence: 2 }),
    );
    assert.equal(delayedFormula.case, 'committed');
    assert.equal(delayedFormula.value.committedVersion, 4n);
    const calculated = await calculation(base, cookie, snapshot.document_id, 4);
    assert.equal(calculated.status, 'complete');
    assert.deepEqual(calculated.values[0].value, { type: 'error', value: '#CYCLE!' });
  } finally {
    first.socket.close();
    second.socket.close();
  }
});

test('delayed formula binding preserves original targets across insertion and range deletion', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 11, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (await client.send(setCommand(client, snapshot, 22, { row: 1, sequence: 2 }))).case,
      'committed',
    );
    const seen = { ...snapshot, version: '3' };
    const inserted = insertRowsCommand(client, seen, snapshot.sheets[0].rows[0], [uuid7()], {
      sequence: 3,
    });
    assert.equal((await client.send(inserted)).case, 'committed');
    const delayed = formulaCommand(client, seen, '=A1+1', { row: 1, column: 2, sequence: 4 });
    const committed = await client.send(delayed);
    assert.equal(committed.case, 'committed');
    assert.equal(committed.value.committedVersion, 5n);
    assert.equal((await client.send(delayed)).case, 'receipt');
    const result = async (version, row, column) => {
      const calculated = await calculation(base, cookie, snapshot.document_id, version);
      assert.equal(calculated.status, 'complete');
      return calculated.values.find(
        (cell) =>
          cell.row_id === snapshot.sheets[0].rows[row] &&
          cell.column_id === snapshot.sheets[0].columns[column],
      )?.value;
    };
    assert.deepEqual(await result(5, 1, 2), { type: 'number', value: 12 });
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(
      (
        await client.send(
          deleteRowsCommand(client, current, [snapshot.sheets[0].rows[0]], { sequence: 5 }),
        )
      ).case,
      'committed',
    );
    assert.deepEqual(await result(6, 1, 2), { type: 'error', value: '#REF!' });
    const delayedRange = formulaCommand(client, seen, '=SUM(A1:A2)', {
      row: 2,
      column: 3,
      sequence: 6,
    });
    assert.equal((await client.send(delayedRange)).case, 'committed');
    assert.deepEqual(await result(7, 2, 3), { type: 'number', value: 22 });
    const replayed = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(replayed.version, '7');
    const canonical = replayed.sheets[0].cells.find(
      (cell) =>
        cell.row_id === snapshot.sheets[0].rows[2] &&
        cell.column_id === snapshot.sheets[0].columns[3],
    );
    assert.equal(canonical.value.value.current_text, '=SUM(A2:A2)');
  } finally {
    client.socket.close();
  }
});

test('appending rows preserves an existing formula target and calculated value', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const value = await client.send(setCommand(client, snapshot, 17, { sequence: 1 }));
    assert.equal(value.case, 'committed');
    const formula = await client.send(
      formulaCommand(client, { ...snapshot, version: '2' }, '=A1', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    const before = await calculation(base, cookie, snapshot.document_id, 3);
    assert.deepEqual(before.values[0].value, { type: 'number', value: 17 });
    const extension = await client.send(
      extendRowsCommand(client, { ...snapshot, version: '3' }, [uuid7()], { sequence: 3 }),
    );
    assert.equal(extension.case, 'committed');
    const after = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(after.values[0].value, before.values[0].value);
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.sheets[0].rows.length, snapshot.sheets[0].rows.length + 1);
    assert.equal(
      current.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value.value.source,
      '=A1',
    );
  } finally {
    client.socket.close();
  }
});

test('row insertion uses stable anchors, server order and rebases formulas', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const left = await connect(base, cookie);
  const right = await connect(base, cookie);
  try {
    const anchor = snapshot.sheets[0].rows[1];
    assert.equal((await left.send(setCommand(left, snapshot, 99, { row: 1 }))).case, 'committed');
    const leftRow = uuid7();
    const rightRow = uuid7();
    const leftCommand = insertRowsCommand(left, snapshot, anchor, [leftRow], { sequence: 2 });
    const rightCommand = insertRowsCommand(right, snapshot, anchor, [rightRow], { sequence: 1 });
    const [first, second] = await Promise.all([left.send(leftCommand), right.send(rightCommand)]);
    assert.equal(first.case, 'committed');
    assert.equal(second.case, 'committed');
    assert.deepEqual(
      [first.value.committedVersion, second.value.committedVersion].sort((a, b) => Number(a - b)),
      [3n, 4n],
    );
    const mostRecent = first.value.committedVersion === 4n ? leftRow : rightRow;
    const earlier = first.value.committedVersion === 3n ? leftRow : rightRow;
    const afterInsert = await getDocument(base, cookie, snapshot.document_id);
    assert.deepEqual(afterInsert.sheets[0].rows.slice(0, 4), [
      snapshot.sheets[0].rows[0],
      earlier,
      mostRecent,
      anchor,
    ]);
    assert.equal(
      afterInsert.sheets[0].cells.find((cell) => cell.row_id === anchor)?.value.value,
      99,
    );
    const retry = await left.send(leftCommand);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, first.value.committedVersion);
    const staleCell = await left.send(setCommand(left, snapshot, 100, { row: 1, sequence: 3 }));
    assert.equal(staleCell.case, 'committed');
    assert.equal(staleCell.value.committedVersion, 5n);
    const formula = await left.send(
      formulaCommand(left, { ...snapshot, version: '5' }, '=A4', {
        row: 0,
        column: 1,
        sequence: 4,
      }),
    );
    assert.equal(formula.case, 'committed');
    assert.equal(formula.value.committedVersion, 6n);
    const transformed = await left.send(
      insertRowsCommand(left, { ...snapshot, version: '6' }, anchor, [uuid7()], { sequence: 5 }),
    );
    assert.equal(transformed.case, 'committed');
    assert.equal(transformed.value.committedVersion, 7n);
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.version, '7');
    assert.equal(current.sheets[0].cells.find((cell) => cell.row_id === anchor)?.value.value, 100);
    const rendered = current.sheets[0].cells.find(
      (cell) =>
        cell.row_id === snapshot.sheets[0].rows[0] &&
        cell.column_id === snapshot.sheets[0].columns[1],
    )?.value.value;
    assert.equal(rendered.source, '=A4');
    assert.equal(rendered.current_text, '=A5');
    const calculated = await calculation(base, cookie, snapshot.document_id, 7);
    assert.equal(calculated.status, 'complete');
    assert.deepEqual(calculated.values[0].value, { type: 'number', value: 100 });
  } finally {
    left.socket.close();
    right.socket.close();
  }
});

test('column insertion preserves bound formula targets and survives durable replay', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 17, { sequence: 1 }))).case,
      'committed',
    );
    const formula = await client.send(
      formulaCommand(client, { ...snapshot, version: '2' }, '=A1', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    const newColumn = uuid7();
    const command = insertColumnsCommand(
      client,
      { ...snapshot, version: '3' },
      snapshot.sheets[0].columns[0],
      [newColumn],
      { sequence: 3 },
    );
    const inserted = await client.send(command);
    assert.equal(inserted.case, 'committed');
    assert.equal(inserted.value.committedVersion, 4n);
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.deepEqual(current.sheets[0].columns.slice(0, 3), [
      newColumn,
      snapshot.sheets[0].columns[0],
      snapshot.sheets[0].columns[1],
    ]);
    assert.equal(
      current.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value.value
        .current_text,
      '=B1',
    );
    assert.equal(
      current.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.column_id,
      snapshot.sheets[0].columns[1],
    );
    const calculated = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(calculated.values[0].value, { type: 'number', value: 17 });
    const retry = await client.send(command);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, 4n);
  } finally {
    client.socket.close();
  }
});

test('concurrent column inserts before one stable anchor follow server order', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const left = await connect(base, cookie);
  const right = await connect(base, cookie);
  try {
    const anchor = snapshot.sheets[0].columns[0];
    const leftId = uuid7();
    const rightId = uuid7();
    const [first, second] = await Promise.all([
      left.send(insertColumnsCommand(left, snapshot, anchor, [leftId])),
      right.send(insertColumnsCommand(right, snapshot, anchor, [rightId])),
    ]);
    assert.equal(first.case, 'committed');
    assert.equal(second.case, 'committed');
    assert.deepEqual(
      [first.value.committedVersion, second.value.committedVersion].sort((a, b) => Number(a - b)),
      [2n, 3n],
    );
    const earlier = first.value.committedVersion === 2n ? leftId : rightId;
    const later = first.value.committedVersion === 3n ? leftId : rightId;
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.deepEqual(current.sheets[0].columns.slice(0, 3), [earlier, later, anchor]);
    assert.equal(current.version, '3');
  } finally {
    left.socket.close();
    right.socket.close();
  }
});

test('column deletion persists REF, rejects stale writes and does not reuse IDs', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  try {
    const removed = snapshot.sheets[0].columns[0];
    assert.equal(
      (await writer.send(setCommand(writer, snapshot, 17, { sequence: 1 }))).case,
      'committed',
    );
    const formula = await writer.send(
      formulaCommand(writer, { ...snapshot, version: '2' }, '=A1', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    const command = deleteColumnsCommand(writer, { ...snapshot, version: '3' }, [removed], {
      sequence: 3,
    });
    const deletion = await writer.send(command);
    assert.equal(deletion.case, 'committed');
    assert.equal(deletion.value.committedVersion, 4n);
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.sheets[0].columns.includes(removed), false);
    assert.equal(
      current.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value.value
        .current_text,
      '=#REF!',
    );
    const calculated = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(calculated.values[0].value, { type: 'error', value: '#REF!' });
    const retry = await writer.send(command);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, 4n);
    const stale = await writer.send(setCommand(writer, snapshot, 99, { sequence: 4 }));
    assert.equal(stale.case, 'rejected');
    const reuse = await writer.send(
      insertColumnsCommand(
        writer,
        { ...snapshot, version: '4' },
        snapshot.sheets[0].columns[1],
        [removed],
        { sequence: 5 },
      ),
    );
    assert.equal(reuse.case, 'rejected');
  } finally {
    writer.socket.close();
  }
});

test('row deletion persists typed REF, refuses stale writes and never reuses a row ID', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    const removed = snapshot.sheets[0].rows[1];
    assert.equal(
      (await writer.send(setCommand(writer, snapshot, 7, { row: 1, sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (
        await writer.send(
          formulaCommand(writer, { ...snapshot, version: '2' }, '=A2', {
            row: 0,
            column: 1,
            sequence: 2,
          }),
        )
      ).case,
      'committed',
    );
    const deletion = deleteRowsCommand(writer, { ...snapshot, version: '3' }, [removed], {
      sequence: 3,
    });
    const committed = await writer.send(deletion);
    assert.equal(committed.case, 'committed');
    assert.equal(committed.value.committedVersion, 4n);
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.sheets[0].rows.length, 999);
    assert.equal(current.sheets[0].rows.includes(removed), false);
    const formula = current.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value
      .value;
    assert.equal(formula.source, '=A2');
    assert.equal(formula.current_text, '=#REF!');
    const calculated = await calculation(base, cookie, snapshot.document_id, 4);
    assert.deepEqual(calculated.values[0].value, { type: 'error', value: '#REF!' });
    const retry = await writer.send(deletion);
    assert.equal(retry.case, 'receipt');
    assert.equal(retry.value.committedVersion, 4n);
    const staleWrite = await second.send(setCommand(second, snapshot, 9, { row: 1 }));
    assert.equal(staleWrite.case, 'rejected');
    assert.equal(staleWrite.value.code, 9);
    const secondDelete = await second.send(
      deleteRowsCommand(second, { ...snapshot, version: '4' }, [removed], { sequence: 2 }),
    );
    assert.equal(secondDelete.case, 'rejected');
    assert.equal(secondDelete.value.code, 9);
    const reused = await writer.send(
      insertRowsCommand(
        writer,
        { ...snapshot, version: '4' },
        snapshot.sheets[0].rows[2],
        [removed],
        {
          sequence: 4,
        },
      ),
    );
    assert.equal(reused.case, 'rejected');
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '4');
  } finally {
    writer.socket.close();
    second.socket.close();
  }
});

test('seeded mixed-client writes, retry, extension and reconnect converge through ordered replay', async () => {
  let random = 0x50e1c0de;
  const next = () => {
    random ^= random << 13;
    random ^= random >>> 17;
    random ^= random << 5;
    return random >>> 0;
  };
  const cookie = await login(base);
  for (let scenario = 0; scenario < 5; scenario++) {
    const { snapshot } = await createDocument(base, cookie);
    let left = await connect(base, cookie);
    const right = await connect(base, cookie);
    try {
      const [a, b] = await Promise.all([
        left.send(setCommand(left, snapshot, next() % 1000, { row: 0, sequence: 1 })),
        right.send(setCommand(right, snapshot, next() % 1000, { row: 1, sequence: 1 })),
      ]);
      assert.deepEqual(
        [a.value.committedVersion, b.value.committedVersion].sort((x, y) => Number(x - y)),
        [2n, 3n],
      );
      let version = 3n;
      for (let index = 0; index < 12; index++) {
        if (index === 4) {
          const rowIds = [uuid7(), uuid7()];
          const extension = await right.send(
            extendRowsCommand(right, { ...snapshot, version: String(version) }, rowIds, {
              sequence: index + 2,
            }),
          );
          assert.equal(extension.case, 'committed');
          assert.equal(extension.value.committedVersion, ++version);
        } else {
          const writer = next() % 2 === 0 ? left : right;
          const command = setCommand(writer, snapshot, next() % 1000, {
            row: next() % 3,
            column: next() % 3,
            sequence: index + 2,
          });
          const committed = await writer.send(command);
          assert.equal(committed.case, 'committed');
          assert.equal(committed.value.committedVersion, ++version);
          if (index === 2) {
            const retry = await writer.send(command);
            assert.equal(retry.case, 'receipt');
            assert.equal(retry.value.committedVersion, version);
          }
        }
        if (index === 6) {
          left.socket.terminate();
          left = await connect(base, cookie);
        }
        await new Promise((resolve) => setTimeout(resolve, 30));
      }
      const canonical = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(canonical.version, String(version));
      assert.equal(canonical.sheets[0].rows.length, snapshot.sheets[0].rows.length + 2);
      const returning = await connect(base, cookie);
      try {
        const frames = await receiveFrames(returning, Number(version), () =>
          subscribeDocument(returning, snapshot.document_id, 1),
        );
        const events = frames.slice(0, -1);
        assert.deepEqual(
          events.map((frame) => frame.value.headVersion),
          Array.from({ length: Number(version - 1n) }, (_, index) => BigInt(index + 2)),
        );
        assert.equal(frames.at(-1).case, 'replayComplete');
        assert.equal(frames.at(-1).value.barrierVersion, version);
        assert.equal(frames.at(-1).value.resyncRequired, false);
        assert.equal(
          Buffer.from(events.at(-1).value.latestOperation.confirmedStateHash).toString('hex'),
          canonical.state_hash,
        );
      } finally {
        returning.socket.close();
      }
    } finally {
      left.socket.terminate();
      right.socket.terminate();
    }
  }
});

test('reconnect crosses structural events and a frozen copy without losing the barrier', async () => {
  const cookie = await login(base);
  for (let scenario = 0; scenario < 5; scenario++) {
    const { snapshot } = await createDocument(base, cookie);
    const left = await connect(base, cookie);
    const right = await connect(base, cookie);
    try {
      const first = await Promise.all([
        left.send(setCommand(left, snapshot, scenario + 10, { sequence: 1 })),
        right.send(setCommand(right, snapshot, scenario + 20, { column: 1, sequence: 1 })),
      ]);
      assert.ok(first.every((result) => result.case === 'committed'));
      assert.deepEqual(first.map((result) => result.value.committedVersion).sort(), [2n, 3n]);
      let current = await getDocument(base, cookie, snapshot.document_id);
      const insertRow = await left.send(
        insertRowsCommand(left, current, current.sheets[0].rows[1], [uuid7()], { sequence: 2 }),
      );
      assert.equal(insertRow.case, 'committed');
      assert.equal(insertRow.value.committedVersion, 4n);
      current = await getDocument(base, cookie, snapshot.document_id);
      const insertColumn = await right.send(
        insertColumnsCommand(right, current, current.sheets[0].columns[1], [uuid7()], {
          sequence: 2,
        }),
      );
      assert.equal(insertColumn.case, 'committed');
      assert.equal(insertColumn.value.committedVersion, 5n);
      current = await getDocument(base, cookie, snapshot.document_id);
      const inserted = await left.send(
        setCommand(left, current, scenario + 30, { row: 1, column: 1, sequence: 3 }),
      );
      assert.equal(inserted.case, 'committed');
      assert.equal(inserted.value.committedVersion, 6n);
      current = await getDocument(base, cookie, snapshot.document_id);
      const copy = copyRangeCommand(
        right,
        current,
        { destinationRow: 2, destinationColumn: 3, rows: 2, columns: 2 },
        { sequence: 3 },
      );
      const copied = await right.send(copy);
      assert.equal(copied.case, 'committed');
      assert.equal(copied.value.committedVersion, 7n);
      assert.equal((await right.send(copy)).case, 'receipt');
      current = await getDocument(base, cookie, snapshot.document_id);
      const deleteRow = await left.send(
        deleteRowsCommand(left, current, [snapshot.sheets[0].rows[1]], { sequence: 4 }),
      );
      assert.equal(deleteRow.case, 'committed');
      assert.equal(deleteRow.value.committedVersion, 8n);
      current = await getDocument(base, cookie, snapshot.document_id);
      const deleteColumn = await right.send(
        deleteColumnsCommand(right, current, [snapshot.sheets[0].columns[1]], { sequence: 4 }),
      );
      assert.equal(deleteColumn.case, 'committed');
      assert.equal(deleteColumn.value.committedVersion, 9n);
      const canonical = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(canonical.version, '9');
      assert.equal(
        canonical.state_hash,
        Buffer.from(deleteColumn.value.confirmedStateHash).toString('hex'),
      );

      const early = await connect(base, cookie);
      try {
        const frames = await receiveFrames(early, 7, () =>
          subscribeDocument(early, snapshot.document_id, 1),
        );
        assert.deepEqual(
          frames.slice(0, 5).map((frame) => frame.value.headVersion),
          [2n, 3n, 4n, 5n, 6n],
        );
        assert.equal(frames[5].case, 'documentHeadChanged');
        assert.equal(frames[5].value.headVersion, 9n);
        assert.equal(frames[5].value.latestOperation, undefined);
        assert.equal(frames[6].case, 'replayComplete');
        assert.equal(frames[6].value.barrierVersion, 9n);
        assert.equal(frames[6].value.resyncRequired, true);
      } finally {
        early.socket.terminate();
      }

      const late = await connect(base, cookie);
      try {
        const frames = await receiveFrames(late, 3, () =>
          subscribeDocument(late, snapshot.document_id, 7),
        );
        assert.deepEqual(
          frames.slice(0, 2).map((frame) => frame.value.headVersion),
          [8n, 9n],
        );
        assert.equal(frames[2].value.resyncRequired, false);
        assert.equal(
          Buffer.from(frames[1].value.latestOperation.confirmedStateHash).toString('hex'),
          canonical.state_hash,
        );
      } finally {
        late.socket.terminate();
      }
    } finally {
      left.socket.terminate();
      right.socket.terminate();
    }
  }
});

test('late subscriber receives every missed version in order before the replay barrier', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  const reader = await connect(base, cookie);
  try {
    for (let index = 1; index <= 3; index++) {
      const result = await writer.send(
        setCommand(writer, snapshot, index * 10, { sequence: index }),
      );
      assert.equal(result.case, 'committed');
      assert.equal(result.value.committedVersion, BigInt(index + 1));
    }
    const frames = await receiveFrames(reader, 4, () =>
      subscribeDocument(reader, snapshot.document_id, 1),
    );
    assert.deepEqual(
      frames.slice(0, 3).map((frame) => frame.value.headVersion),
      [2n, 3n, 4n],
    );
    assert.ok(frames.slice(0, 3).every((frame) => frame.case === 'documentHeadChanged'));
    assert.deepEqual(
      frames.slice(0, 3).map((frame) => frame.value.latestOperation.committedVersion),
      [2n, 3n, 4n],
    );
    assert.equal(frames[3].case, 'replayComplete');
    assert.equal(frames[3].value.barrierVersion, 4n);
    assert.equal(frames[3].value.resyncRequired, false);
    const live = nextFrame(reader);
    const final = await writer.send(setCommand(writer, snapshot, 40, { sequence: 4 }));
    assert.equal(final.case, 'committed');
    const update = await live;
    assert.equal(update.case, 'documentHeadChanged');
    assert.equal(update.value.headVersion, 5n);
    assert.equal(update.value.latestOperation.committedVersion, 5n);
  } finally {
    writer.socket.close();
    reader.socket.close();
  }
});

test('commit during replay is delivered after the captured barrier without a gap', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  const reader = await connect(base, cookie);
  try {
    for (let index = 1; index <= 50; index++) {
      const result = await writer.send(setCommand(writer, snapshot, index, { sequence: index }));
      assert.equal(result.case, 'committed');
    }
    let racingCommit;
    const frames = await receiveFrames(reader, 52, () => {
      reader.socket.once('message', () => {
        racingCommit = writer.send(setCommand(writer, snapshot, 51, { sequence: 51 }));
      });
      subscribeDocument(reader, snapshot.document_id, 1);
    });
    assert.equal((await racingCommit).case, 'committed');
    assert.deepEqual(
      frames.slice(0, 50).map((frame) => frame.value.headVersion),
      Array.from({ length: 50 }, (_, index) => BigInt(index + 2)),
    );
    assert.equal(frames[50].case, 'replayComplete');
    assert.equal(frames[50].value.barrierVersion, 51n);
    assert.equal(frames[51].case, 'documentHeadChanged');
    assert.equal(frames[51].value.headVersion, 52n);
  } finally {
    writer.socket.close();
    reader.socket.close();
  }
});

test('system calculation event requests a projection resync at the replay barrier', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const response = await api(
    base,
    cookie,
    `/api/documents/${snapshot.document_id}/recalculate`,
    'POST',
    { operation_id: uuid7() },
  );
  assert.equal(response.status, 200);
  const reader = await connect(base, cookie);
  try {
    const [hint, marker] = await receiveFrames(reader, 2, () =>
      subscribeDocument(reader, snapshot.document_id, 1),
    );
    assert.equal(hint.case, 'documentHeadChanged');
    assert.equal(hint.value.headVersion, 2n);
    assert.equal(hint.value.latestOperation, undefined);
    assert.equal(marker.case, 'replayComplete');
    assert.equal(marker.value.barrierVersion, 2n);
    assert.equal(marker.value.resyncRequired, true);
  } finally {
    reader.socket.close();
  }
});

test('replay above the event cap requests one bounded projection resync', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const writer = await connect(base, cookie);
  const reader = await connect(base, cookie);
  try {
    for (let index = 1; index <= 129; index++) {
      const result = await writer.send(setCommand(writer, snapshot, index, { sequence: index }));
      assert.equal(result.case, 'committed');
    }
    const [hint, marker] = await receiveFrames(reader, 2, () =>
      subscribeDocument(reader, snapshot.document_id, 1),
    );
    assert.equal(hint.case, 'documentHeadChanged');
    assert.equal(hint.value.headVersion, 130n);
    assert.equal(hint.value.latestOperation, undefined);
    assert.equal(marker.case, 'replayComplete');
    assert.equal(marker.value.barrierVersion, 130n);
    assert.equal(marker.value.resyncRequired, true);
  } finally {
    writer.socket.close();
    reader.socket.close();
  }
});

test('revoked development session receives no later document head hint', async () => {
  const revokedCookie = await login(base, 'local');
  const writerCookie = await login(base, 'local');
  const { snapshot } = await createDocument(base, writerCookie);
  const observer = await connect(base, revokedCookie);
  const writer = await connect(base, writerCookie);
  const messages = [];
  observer.socket.on('message', (data) => messages.push(data));
  try {
    const closed = once(observer.socket, 'close', { signal: AbortSignal.timeout(5000) });
    subscribeDocument(observer, snapshot.document_id, snapshot.version);
    const logout = await api(base, revokedCookie, '/api/dev/session', 'DELETE');
    assert.equal(logout.status, 204);
    const committed = await writer.send(setCommand(writer, snapshot, 88));
    assert.equal(committed.case, 'committed');
    await closed;
    assert.ok(messages.every((data) => decodeFrame(data).case === 'replayComplete'));
  } finally {
    observer.socket.close();
    writer.socket.close();
  }
});

test('explicit recalc persists one system epoch per operation identity and survives replay', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const formula = await client.send(formulaCommand(client, snapshot, '=2+3', { sequence: 1 }));
    assert.equal(formula.case, 'committed');
    const operationId = uuid7();
    const path = `/api/documents/${snapshot.document_id}/recalculate`;
    const first = await api(base, cookie, path, 'POST', { operation_id: operationId });
    assert.equal(first.status, 200);
    const receipt = await first.json();
    assert.equal(receipt.committed_version, '3');
    assert.equal(receipt.current_head_version, '3');
    const head = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(head.calculation_context.epoch, '1');
    assert.equal(head.version, '3');
    assert.equal(head.sheets[0].cells[0].value.value.source, '=2+3');
    const { chunks } = await requestCalculation(client, snapshot.document_id, 3);
    const final = chunks.at(-1);
    assert.equal(final.calculationEpoch, 1n);
    assert.equal(
      Buffer.from(final.contextId).toString('hex'),
      head.calculation_context.context_id.replaceAll('-', ''),
    );
    assert.equal(final.cells[0].value.kind.value, 5);
    const duplicate = await api(base, cookie, path, 'POST', { operation_id: operationId });
    assert.equal(duplicate.status, 200);
    assert.equal((await duplicate.json()).committed_version, '3');
    assert.equal(
      (await getDocument(base, cookie, snapshot.document_id)).state_hash,
      head.state_hash,
    );
    const later = await api(base, cookie, path, 'POST', { operation_id: uuid7() });
    assert.equal(later.status, 200);
    assert.equal((await later.json()).committed_version, '4');
    const advanced = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(advanced.calculation_context.epoch, '2');
    assert.notEqual(advanced.calculation_context.context_id, head.calculation_context.context_id);
    const outsider = await login(base, 'epoch_' + uuid7().replaceAll('-', '').slice(0, 18));
    assert.ok(
      [403, 404].includes(
        (await api(base, outsider, path, 'POST', { operation_id: uuid7() })).status,
      ),
    );
    const invalidOrigin = await fetch(base + path, {
      method: 'POST',
      headers: {
        Origin: 'https://other.example',
        Cookie: cookie,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ operation_id: uuid7() }),
    });
    assert.equal(invalidOrigin.status, 403);
  } finally {
    client.socket.terminate();
  }
});

test(
  'calculation WebSocket stream is chunked, complete, read-only and authorized',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      for (let row = 0; row < 24; row++) {
        const result = await client.send(
          formulaCommand(client, snapshot, '="' + 'x'.repeat(16384) + '"', {
            row,
            sequence: row + 1,
          }),
        );
        assert.equal(result.case, 'committed');
      }
      const before = await getDocument(base, cookie, snapshot.document_id);
      const { chunks } = await requestCalculation(client, snapshot.document_id, 25);
      assert.ok(chunks.length >= 2);
      let cells = 0;
      for (let sequence = 0; sequence < chunks.length; sequence++) {
        const chunk = chunks[sequence];
        assert.equal(chunk.patchSequence, sequence);
        assert.equal(chunk.inputVersion, 25n);
        assert.equal(chunk.fullSnapshot, true);
        assert.equal(chunk.errorCode, 0);
        assert.equal(chunk.isFinal, sequence === chunks.length - 1);
        assert.equal(chunk.calculatedStateHash.length, chunk.isFinal ? 32 : 0);
        for (const cell of chunk.cells) {
          assert.equal(cell.value.kind.case, 'text');
          assert.equal(cell.value.kind.value.length, 16384);
          cells++;
        }
      }
      assert.equal(cells, 24);
      const http = await calculation(base, cookie, snapshot.document_id, 25);
      assert.equal(
        Buffer.from(chunks.at(-1).calculatedStateHash).toString('hex'),
        http.calculated_state_hash,
      );
      const after = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(after.version, before.version);
      assert.equal(after.state_hash, before.state_hash);
      const stranger = await connect(
        base,
        await login(base, 'stream_' + uuid7().replaceAll('-', '').slice(0, 18)),
      );
      try {
        assert.equal((await requestCalculation(stranger, snapshot.document_id, 25)).error.code, 3);
      } finally {
        stranger.socket.terminate();
      }
    } finally {
      client.socket.terminate();
    }
  },
);

test(
  'server calculation is versioned, incremental and cycles remain typed values',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      assert.equal((await client.send(setCommand(client, snapshot, 2))).case, 'committed');
      assert.equal(
        (await client.send(setCommand(client, snapshot, 3, { column: 1, sequence: 2 }))).case,
        'committed',
      );
      const formula = await client.send(
        formulaCommand(client, snapshot, '=A1+B1', { column: 2, sequence: 3 }),
      );
      assert.equal(formula.case, 'committed');
      assert.equal(formula.value.committedVersion, 4n);
      let state = await calculation(base, cookie, snapshot.document_id, 4);
      assert.equal(state.status, 'complete');
      assert.match(state.calculated_state_hash, /^[0-9a-f]{64}$/);
      assert.deepEqual(state.values[0].value, { type: 'number', value: 5 });
      await client.send(
        formulaCommand(client, { ...snapshot, version: '4' }, '=10', { column: 3, sequence: 4 }),
      );
      state = await calculation(base, cookie, snapshot.document_id, 5);
      assert.equal(state.evaluated_cells, 1);
      await client.send(setCommand(client, { ...snapshot, version: '5' }, 8, { sequence: 5 }));
      state = await calculation(base, cookie, snapshot.document_id, 6);
      assert.equal(state.evaluated_cells, 1);
      assert.deepEqual(
        state.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[2]).value,
        { type: 'number', value: 11 },
      );
      await client.send(
        formulaCommand(client, { ...snapshot, version: '6' }, '=C1', { column: 2, sequence: 6 }),
      );
      state = await calculation(base, cookie, snapshot.document_id, 7);
      assert.equal(state.status, 'complete');
      assert.deepEqual(
        state.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[2]).value,
        { type: 'error', value: '#CYCLE!' },
      );
      const stranger = await login(base, 'calc_' + uuid7().replaceAll('-', '').slice(0, 20));
      assert.ok(
        [403, 404].includes(
          (await api(base, stranger, `/api/documents/${snapshot.document_id}/calculation`)).status,
        ),
      );
    } finally {
      client.socket.close();
    }
  },
);

test('COUNT and COUNTA bind as durable formulas and calculate distinct reference rules', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 4, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (await client.send(setCommand(client, snapshot, '5', { column: 1, sequence: 2 }))).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=COUNT(A1:B1)', { column: 2, sequence: 3 }),
        )
      ).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=COUNTA(A1:B1)', { column: 3, sequence: 4 }),
        )
      ).case,
      'committed',
    );
    const state = await calculation(base, cookie, snapshot.document_id, 5);
    assert.equal(state.status, 'complete');
    const result = new Map(state.values.map((cell) => [cell.column_id, cell.value]));
    assert.deepEqual(result.get(snapshot.sheets[0].columns[2]), { type: 'number', value: 1 });
    assert.deepEqual(result.get(snapshot.sheets[0].columns[3]), { type: 'number', value: 2 });
    const restored = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(restored.version, '5');
    assert.equal(
      restored.sheets[0].cells.filter((cell) => cell.value.type === 'formula').length,
      2,
    );
  } finally {
    client.socket.close();
  }
});

test('logical functions commit, recalculate and survive a persisted context advance', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 2, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (await client.send(setCommand(client, snapshot, 3, { column: 1, sequence: 2 }))).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=AND(A1>0,NOT(B1=0),OR(A1:A2))', {
            column: 2,
            sequence: 3,
          }),
        )
      ).case,
      'committed',
    );
    const valueAt = async (version) => {
      const state = await calculation(base, cookie, snapshot.document_id, version);
      assert.equal(state.status, 'complete');
      assert.equal(state.engine_version, 'pxel-formula-0.3.0');
      return state.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[2])?.value;
    };
    assert.deepEqual(await valueAt(4), { type: 'boolean', value: true });
    assert.equal(
      (await client.send(setCommand(client, snapshot, 0, { column: 1, sequence: 4 }))).case,
      'committed',
    );
    assert.deepEqual(await valueAt(5), { type: 'boolean', value: false });
    const response = await api(
      base,
      cookie,
      `/api/documents/${snapshot.document_id}/recalculate`,
      'POST',
      { operation_id: uuid7() },
    );
    assert.equal(response.status, 200);
    assert.deepEqual(await valueAt(6), { type: 'boolean', value: false });
    assert.equal(
      (await getDocument(base, cookie, snapshot.document_id)).calculation_context.epoch,
      '1',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=OR(TRUE,1/0)', { column: 2, sequence: 5 }),
        )
      ).case,
      'committed',
    );
    assert.deepEqual(await valueAt(7), { type: 'error', value: '#DIV/0!' });
  } finally {
    client.socket.close();
  }
});

test('IFS commits and recalculates the selected branch after input changes', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 3, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (await client.send(setCommand(client, snapshot, 5, { column: 1, sequence: 2 }))).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=IFS(A1>B1,A1,TRUE,B1)', { column: 2, sequence: 3 }),
        )
      ).case,
      'committed',
    );
    const column = snapshot.sheets[0].columns[2];
    const valueAt = async (version) => {
      const state = await calculation(base, cookie, snapshot.document_id, version);
      assert.equal(state.status, 'complete');
      return state.values.find((cell) => cell.column_id === column)?.value;
    };
    assert.deepEqual(await valueAt(4), { type: 'number', value: 5 });
    assert.equal(
      (await client.send(setCommand(client, snapshot, 7, { sequence: 4 }))).case,
      'committed',
    );
    assert.deepEqual(await valueAt(5), { type: 'number', value: 7 });
  } finally {
    client.socket.close();
  }
});

test('IFERROR replacement is durable and leaves a later valid result intact', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 1, { sequence: 1 }))).case,
      'committed',
    );
    assert.equal(
      (await client.send(setCommand(client, snapshot, 0, { column: 1, sequence: 2 }))).case,
      'committed',
    );
    assert.equal(
      (
        await client.send(
          formulaCommand(client, snapshot, '=IFERROR(A1/B1,9)', { column: 2, sequence: 3 }),
        )
      ).case,
      'committed',
    );
    const column = snapshot.sheets[0].columns[2];
    const valueAt = async (version) => {
      const state = await calculation(base, cookie, snapshot.document_id, version);
      assert.equal(state.status, 'complete');
      return state.values.find((cell) => cell.column_id === column)?.value;
    };
    assert.deepEqual(await valueAt(4), { type: 'number', value: 9 });
    assert.equal(
      (await client.send(setCommand(client, snapshot, 2, { column: 1, sequence: 4 }))).case,
      'committed',
    );
    assert.deepEqual(await valueAt(5), { type: 'number', value: 0.5 });
  } finally {
    client.socket.close();
  }
});

test(
  'formula text is server-bound, durable and deduplicated independently of AST',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      const command = formulaCommand(client, snapshot, '=A1+SUM($B$1:B2)', { column: 2 });
      const result = await client.send(command);
      assert.equal(result.case, 'committed');
      const canonical = result.value.canonicalOperation;
      assert.equal(canonical.kind.case, 'setCellFormula');
      const formula = canonical.kind.value.formula;
      assert.equal(formula.source, command.formulaCommand.source);
      assert.equal(formula.ast.schemaVersion, 1);
      assert.equal(formula.boundAtVersion, 1n);
      const reference = formula.ast.nodes.find((node) => node.kind.case === 'cell').kind.value;
      assert.deepEqual(reference.rowId, bytes(snapshot.sheets[0].rows[0]));
      assert.deepEqual(reference.columnId, bytes(snapshot.sheets[0].columns[0]));
      assert.equal((await client.send(command)).case, 'receipt');
      const altered = structuredClone(command);
      altered.formulaCommand.source = '=A2';
      assert.equal((await client.send(altered)).value.code, 6);
      const loaded = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(loaded.version, '2');
      assert.equal(loaded.sheets[0].cells[0].value.type, 'formula');
      assert.equal(loaded.sheets[0].cells[0].value.value.source, formula.source);

      const untrustedAst = setCommand(client, loaded, 0, { sequence: 2 });
      untrustedAst.operation = canonical;
      assert.equal((await client.send(untrustedAst)).value.code, 3);
      const invalid = formulaCommand(client, loaded, '=SUM(', { sequence: 3 });
      assert.equal((await client.send(invalid)).value.code, 11);
      const missingBase = formulaCommand(client, loaded, '=A1', { sequence: 4 });
      missingBase.baseVersion = 0n;
      assert.equal((await client.send(missingBase)).value.code, 10);
      assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '2');
    } finally {
      client.socket.close();
    }
  },
);

test('undo restores a formula preimage only while its axes are unchanged', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    assert.equal(
      (await client.send(setCommand(client, snapshot, 7, { sequence: 1 }))).case,
      'committed',
    );
    const formula = await client.send(
      formulaCommand(client, { ...snapshot, version: '2' }, '=A1+1', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    const overwrite = setCommand(client, { ...snapshot, version: '3' }, 99, {
      column: 1,
      sequence: 3,
    });
    assert.equal((await client.send(overwrite)).case, 'committed');
    const undo = await client.send(
      undoCellCommand(client, { ...snapshot, version: '4' }, overwrite.operationId, {
        sequence: 4,
      }),
    );
    assert.equal(undo.case, 'committed', `undo rejected with code ${undo.value?.code}`);
    assert.equal(undo.value.canonicalOperation.kind.case, 'setCellFormula');
    const restored = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(restored.version, '5');
    assert.equal(
      restored.sheets[0].cells.find((cell) => cell.column_id === snapshot.sheets[0].columns[1])
        ?.value.value.source,
      '=A1+1',
    );
    const calculated = await calculation(base, cookie, snapshot.document_id, 5);
    assert.equal(calculated.status, 'complete');
    assert.deepEqual(
      calculated.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[1])?.value,
      { type: 'number', value: 8 },
    );

    const later = setCommand(client, restored, 100, { column: 1, sequence: 5 });
    assert.equal((await client.send(later)).case, 'committed');
    const insertion = await client.send(
      insertRowsCommand(
        client,
        { ...restored, version: '6' },
        snapshot.sheets[0].rows[0],
        [uuid7()],
        {
          sequence: 6,
        },
      ),
    );
    assert.equal(insertion.case, 'committed');
    const rejected = await client.send(
      undoCellCommand(client, { ...restored, version: '7' }, later.operationId, {
        sequence: 7,
      }),
    );
    assert.equal(rejected.case, 'rejected');
    assert.equal(rejected.value.code, 14);
    assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '7');
  } finally {
    client.socket.close();
  }
});

test(
  'create, durable commit, exact retry, collision and stable-ID validation',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { intent, snapshot } = await createDocument(base, cookie);
    assert.equal(snapshot.version, '1');
    assert.equal(snapshot.sheets[0].rows.length, 1000);
    assert.equal(snapshot.sheets[0].columns.length, 26);
    assert.deepEqual(snapshot.sheets[0].cells, []);
    const repeated = await api(base, cookie, '/api/documents', 'POST', intent);
    assert.equal(repeated.status, 201);
    assert.deepEqual(await getDocument(base, cookie, intent.document_id), snapshot);
    const client = await connect(base, cookie);
    try {
      const command = setCommand(client, snapshot, 42);
      const committed = await client.send(command);
      assert.equal(committed.case, 'committed');
      assert.equal(committed.value.committedVersion, 2n);
      const duplicate = await client.send(command);
      assert.equal(duplicate.case, 'receipt');
      assert.equal(duplicate.value.committedVersion, 2n);
      const altered = structuredClone(command);
      altered.operation.kind.value.value.kind.value = 43;
      const collision = await client.send(altered);
      assert.equal(collision.case, 'rejected');
      assert.equal(collision.value.code, 6);
      const invalid = setCommand(client, snapshot, 'invalid target', { sequence: 2 });
      invalid.operation.kind.value.address.rowId = bytes(uuid7());
      const rejected = await client.send(invalid);
      assert.equal(rejected.case, 'rejected');
      assert.equal(rejected.value.code, 9);
      const impersonated = setCommand(client, snapshot, 'wrong actor', { sequence: 3 });
      impersonated.actorId = bytes(uuid7());
      const denied = await client.send(impersonated);
      assert.equal(denied.case, 'rejected');
      assert.equal(denied.value.code, 1);
      const current = await getDocument(base, cookie, intent.document_id);
      assert.equal(current.version, '2');
      assert.equal(
        current.state_hash,
        Buffer.from(committed.value.confirmedStateHash).toString('hex'),
      );
      assert.deepEqual(current.sheets[0].cells, [
        {
          row_id: snapshot.sheets[0].rows[0],
          column_id: snapshot.sheets[0].columns[0],
          value: { type: 'number', value: 42 },
        },
      ]);
    } finally {
      client.socket.terminate();
    }
  },
);

test(
  'two connections with stale base get ordered independent property commits',
  { timeout: 20000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const a = await connect(base, cookie),
      b = await connect(base, cookie);
    try {
      const results = await Promise.all([
        a.send(setCommand(a, snapshot, 'left')),
        b.send(setCommand(b, snapshot, 'right', { column: 1 })),
      ]);
      assert.ok(results.every((r) => r.case === 'committed'));
      assert.deepEqual(results.map((r) => r.value.committedVersion).sort(), [2n, 3n]);
      const state = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(state.version, '3');
      assert.equal(state.sheets[0].cells.length, 2);
      assert.deepEqual(state.sheets[0].cells.map((c) => c.value.value).sort(), ['left', 'right']);
    } finally {
      a.socket.terminate();
      b.socket.terminate();
    }
  },
);

test(
  'another authenticated tenant cannot read or write a document',
  { timeout: 20000 },
  async () => {
    const owner = await login(base);
    const stranger = await login(base, 'stranger_' + uuid7().replaceAll('-', '').slice(-16));
    const { snapshot } = await createDocument(base, owner);
    const response = await api(base, stranger, '/api/documents/' + snapshot.document_id);
    assert.ok([403, 404].includes(response.status));
    const other = await connect(base, stranger);
    try {
      const result = await other.send(setCommand(other, snapshot, 'not allowed'));
      assert.equal(result.case, 'rejected');
      assert.ok([1, 2].includes(result.value.code));
      assert.equal((await getDocument(base, owner, snapshot.document_id)).version, '1');
    } finally {
      other.socket.terminate();
    }
  },
);

test('owner grant and revoke fence a second user subscription at access epoch', async () => {
  const suffix = uuid7().replaceAll('-', '').slice(-16);
  const owner = await login(base, 'acl_owner_' + suffix);
  const readerAccount = 'acl_reader_' + suffix;
  const readerHome = await login(base, readerAccount);
  const ownerIdentity = await (await api(base, owner, '/api/dev/identity')).json();
  const readerIdentity = await (await api(base, readerHome, '/api/dev/identity')).json();
  const { snapshot } = await createDocument(base, owner);
  assert.ok(
    [403, 404].includes(
      (await api(base, readerHome, '/api/documents/' + snapshot.document_id)).status,
    ),
  );
  const path = `/api/documents/${snapshot.document_id}/permissions`;
  const grant = {
    action: 'grant',
    change_id: uuid7(),
    target_user_id: readerIdentity.actor_id,
    role: 'viewer',
  };
  const granted = await api(base, owner, path, 'POST', grant);
  assert.equal(granted.status, 200);
  assert.deepEqual(await granted.json(), {
    change_id: grant.change_id,
    access_epoch: 1,
    newly_committed: true,
  });
  const retry = await api(base, owner, path, 'POST', grant);
  assert.equal(retry.status, 200);
  assert.equal((await retry.json()).newly_committed, false);
  assert.equal((await api(base, owner, path, 'POST', { ...grant, role: 'editor' })).status, 409);
  const readerCookie = await login(base, readerAccount, ownerIdentity.workspace_id);
  assert.equal((await getDocument(base, readerCookie, snapshot.document_id)).version, '1');
  assert.equal(
    (await api(base, readerCookie, path, 'POST', { ...grant, change_id: uuid7() })).status,
    403,
  );
  const reader = await connect(base, readerCookie);
  const writer = await connect(base, owner);
  try {
    const [barrier] = await receiveFrames(reader, 1, () =>
      subscribeDocument(reader, snapshot.document_id, 1),
    );
    assert.equal(barrier.case, 'replayComplete');
    const denied = await reader.send(setCommand(reader, snapshot, 'viewer cannot write'));
    assert.equal(denied.case, 'rejected');
    const update = nextFrame(reader);
    assert.equal((await writer.send(setCommand(writer, snapshot, 11))).case, 'committed');
    assert.equal((await update).value.headVersion, 2n);
    const closed = once(reader.socket, 'close', { signal: AbortSignal.timeout(5000) });
    const revoke = {
      action: 'revoke',
      change_id: uuid7(),
      target_user_id: readerIdentity.actor_id,
    };
    const revoked = await api(base, owner, path, 'POST', revoke);
    assert.equal(revoked.status, 200);
    assert.equal((await revoked.json()).access_epoch, 2);
    await closed;
    assert.ok(
      [403, 404].includes(
        (await api(base, readerCookie, '/api/documents/' + snapshot.document_id)).status,
      ),
    );
    assert.equal(
      (await writer.send(setCommand(writer, snapshot, 12, { sequence: 2 }))).case,
      'committed',
    );
  } finally {
    reader.socket.terminate();
    writer.socket.terminate();
  }
});

test('formula calculation and live subscription respect ACL revoke across versions', async () => {
  const suffix = uuid7().replaceAll('-', '').slice(-16);
  const owner = await login(base, 'calc_owner_' + suffix);
  const readerAccount = 'calc_reader_' + suffix;
  const readerHome = await login(base, readerAccount);
  const ownerIdentity = await (await api(base, owner, '/api/dev/identity')).json();
  const readerIdentity = await (await api(base, readerHome, '/api/dev/identity')).json();
  const { snapshot } = await createDocument(base, owner);
  const writer = await connect(base, owner);
  let reader;
  let calculationClient;
  try {
    assert.equal((await writer.send(setCommand(writer, snapshot, 2))).case, 'committed');
    const formula = await writer.send(
      formulaCommand(writer, { ...snapshot, version: '2' }, '=A1+1', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    assert.equal(formula.value.committedVersion, 3n);
    const path = `/api/documents/${snapshot.document_id}/permissions`;
    const grant = await api(base, owner, path, 'POST', {
      action: 'grant',
      change_id: uuid7(),
      target_user_id: readerIdentity.actor_id,
      role: 'viewer',
    });
    assert.equal(grant.status, 200);
    const readerCookie = await login(base, readerAccount, ownerIdentity.workspace_id);
    reader = await connect(base, readerCookie);
    calculationClient = await connect(base, readerCookie);
    const [barrier] = await receiveFrames(reader, 1, () =>
      subscribeDocument(reader, snapshot.document_id, 3),
    );
    assert.equal(barrier.case, 'replayComplete');
    const before = await requestCalculation(calculationClient, snapshot.document_id, 3);
    assert.equal(before.chunks.at(-1).cells[0].value.kind.value, 3);
    const update = nextFrame(reader);
    assert.equal(
      (await writer.send(setCommand(writer, { ...snapshot, version: '3' }, 7, { sequence: 3 })))
        .case,
      'committed',
    );
    assert.equal((await update).value.headVersion, 4n);
    const recalculated = await requestCalculation(calculationClient, snapshot.document_id, 4);
    const last = recalculated.chunks.at(-1);
    assert.equal(last.inputVersion, 4n);
    assert.equal(last.cells[0].value.kind.value, 8);
    const closed = once(reader.socket, 'close', { signal: AbortSignal.timeout(5000) });
    const revoke = await api(base, owner, path, 'POST', {
      action: 'revoke',
      change_id: uuid7(),
      target_user_id: readerIdentity.actor_id,
    });
    assert.equal(revoke.status, 200);
    assert.equal((await revoke.json()).access_epoch, 2);
    await closed;
    assert.equal(
      (await writer.send(setCommand(writer, { ...snapshot, version: '4' }, 9, { sequence: 4 })))
        .case,
      'committed',
    );
    assert.ok(
      [403, 404].includes(
        (await api(base, readerCookie, `/api/documents/${snapshot.document_id}/calculation`))
          .status,
      ),
    );
    const denied = await requestCalculation(calculationClient, snapshot.document_id, 5);
    assert.equal(denied.error.code, 3);
    const ownerResult = await calculation(base, owner, snapshot.document_id, 5);
    assert.deepEqual(ownerResult.values[0].value, { type: 'number', value: 10 });
  } finally {
    reader?.socket.terminate();
    calculationClient?.socket.terminate();
    writer.socket.terminate();
  }
});

test('owner exports committed scalar XLSX while viewer cannot export', async () => {
  const suffix = uuid7().replaceAll('-', '').slice(-16);
  const owner = await login(base, 'exp_owner_' + suffix);
  const viewerAccount = 'exp_viewer_' + suffix;
  const viewerHome = await login(base, viewerAccount);
  const ownerIdentity = await (await api(base, owner, '/api/dev/identity')).json();
  const viewerIdentity = await (await api(base, viewerHome, '/api/dev/identity')).json();
  const { snapshot } = await createDocument(base, owner);
  const writer = await connect(base, owner);
  try {
    assert.equal((await writer.send(setCommand(writer, snapshot, 7))).case, 'committed');
    const formula = await writer.send(
      formulaCommand(writer, { ...snapshot, version: '2' }, '=A1*2', {
        column: 1,
        sequence: 2,
      }),
    );
    assert.equal(formula.case, 'committed');
    const path = `/api/documents/${snapshot.document_id}/export.xlsx`;
    const response = await api(base, owner, path);
    assert.equal(response.status, 200);
    assert.equal(
      response.headers.get('content-type'),
      'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    );
    assert.equal(response.headers.get('cache-control'), 'no-store');
    const archive = Buffer.from(await response.arrayBuffer());
    assert.equal(archive.subarray(0, 4).toString('hex'), '504b0304');
    assert.ok(archive.includes(Buffer.from('<v>7</v>')));
    assert.ok(archive.includes(Buffer.from('<f>(A1*2)</f>')));
    const grant = await api(
      base,
      owner,
      `/api/documents/${snapshot.document_id}/permissions`,
      'POST',
      {
        action: 'grant',
        change_id: uuid7(),
        target_user_id: viewerIdentity.actor_id,
        role: 'viewer',
      },
    );
    assert.equal(grant.status, 200);
    const viewer = await login(base, viewerAccount, ownerIdentity.workspace_id);
    assert.equal((await api(base, viewer, path)).status, 403);
    const stranger = await login(base, 'exp_other_' + suffix);
    assert.ok([403, 404].includes((await api(base, stranger, path)).status));
  } finally {
    writer.socket.terminate();
  }
});
