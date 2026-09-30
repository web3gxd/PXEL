import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  login,
  createDocument,
  connect,
  getDocument,
  setCommand,
  formulaCommand,
  deleteRowsCommand,
  undoCellCommand,
  calculation,
  releaseSessions,
} from '../helpers/client.mjs';

test(
  'axis delete compensation and redo recover formulas, activation and late receipts',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18087';
    let child;
    let client;
    async function start() {
      child = spawn(
        resolve(
          process.env.PXEL_RECOVERY_SERVER_BINARY ??
            'target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''),
        ),
        [],
        {
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18087' },
          stdio: 'ignore',
          windowsHide: true,
        },
      );
      let error;
      child.on('error', (caught) => {
        error = caught;
      });
      for (let i = 0; i < 100; i++) {
        if (error) throw error;
        if (child.exitCode !== null) throw new Error('Axis history server exited before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('Axis history server readiness timeout');
    }
    async function kill() {
      if (child && child.exitCode === null) {
        const exited = once(child, 'exit');
        child.kill('SIGKILL');
        await exited;
      }
    }
    try {
      await start();
      const cookie = await login(base);
      let { snapshot: book } = await createDocument(base, cookie);
      client = await connect(base, cookie);
      async function edit(command) {
        const response = await client.send(command);
        assert.equal(response.case, 'committed');
        book = await getDocument(base, cookie, book.document_id);
        return response;
      }
      await edit(setCommand(client, book, 4, { row: 1 }));
      await edit(formulaCommand(client, book, '=A2*2', { column: 1, sequence: 2 }));
      await edit(formulaCommand(client, book, '=A2+3', { row: 1, column: 2, sequence: 3 }));
      const before = book;
      const delayed = formulaCommand(client, book, '=A2', { column: 3, sequence: 5 });
      const delayedValue = setCommand(client, book, 99, { row: 1, sequence: 8 });
      const deletion = deleteRowsCommand(client, book, [book.sheets[0].rows[1]], { sequence: 4 });
      await edit(deletion);
      assert.equal(book.sheets[0].rows.includes(before.sheets[0].rows[1]), false);
      assert.equal(
        book.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value.current_text,
        '=(#REF!*2)',
      );
      const deleted = book;
      const undo = undoCellCommand(client, book, deletion.operationId, { sequence: 6 });
      const committedUndo = await edit(undo);
      assert.deepEqual(book.sheets, before.sheets);
      const expectedUndo = book;
      const undoCalculation = await calculation(base, cookie, book.document_id, book.version);
      assert.deepEqual(
        undoCalculation.values.map((cell) => cell.value).sort((a, b) => a.value - b.value),
        [
          { type: 'number', value: 7 },
          { type: 'number', value: 8 },
        ],
      );
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.equal(book.state_hash, expectedUndo.state_hash);
      assert.deepEqual(book.sheets, expectedUndo.sheets);
      const recovered = await calculation(base, cookie, book.document_id, book.version);
      assert.equal(recovered.calculated_state_hash, undoCalculation.calculated_state_hash);
      assert.deepEqual(recovered.values, undoCalculation.values);
      client = await connect(base, cookie);
      const retry = await client.send(undo);
      assert.equal(retry.case, 'receipt');
      assert.equal(retry.value.committedVersion, committedUndo.value.committedVersion);
      const rejected = await client.send(delayed);
      assert.equal(rejected.case, 'rejected');
      assert.equal(rejected.value.code, 17);
      const rejectedValue = await client.send(delayedValue);
      assert.equal(rejectedValue.case, 'rejected');
      assert.equal(rejectedValue.value.code, 17);
      assert.equal(
        (await getDocument(base, cookie, book.document_id)).state_hash,
        expectedUndo.state_hash,
      );
      const redo = undoCellCommand(client, book, undo.operationId, { sequence: 7 });
      const committedRedo = await edit(redo);
      assert.deepEqual(book.sheets, deleted.sheets);
      const expectedRedo = book;
      const redoCalculation = await calculation(base, cookie, book.document_id, book.version);
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.equal(book.state_hash, expectedRedo.state_hash);
      assert.deepEqual(book.sheets, expectedRedo.sheets);
      assert.equal(
        (await calculation(base, cookie, book.document_id, book.version)).calculated_state_hash,
        redoCalculation.calculated_state_hash,
      );
      assert.deepEqual(
        (await calculation(base, cookie, book.document_id, book.version)).values,
        redoCalculation.values,
      );
      client = await connect(base, cookie);
      const redoRetry = await client.send(redo);
      assert.equal(redoRetry.case, 'receipt');
      assert.equal(redoRetry.value.committedVersion, committedRedo.value.committedVersion);
      const oldUndo = await client.send(undo);
      assert.equal(oldUndo.case, 'receipt');
      assert.equal(oldUndo.value.committedVersion, committedUndo.value.committedVersion);
      assert.equal(oldUndo.value.currentHeadVersion, committedRedo.value.committedVersion);
      const oldDelete = await client.send(deletion);
      assert.equal(oldDelete.case, 'receipt');
      assert.equal(oldDelete.value.currentHeadVersion, committedRedo.value.committedVersion);
      assert.equal(
        (await getDocument(base, cookie, book.document_id)).state_hash,
        expectedRedo.state_hash,
      );
    } finally {
      client?.socket.terminate();
      try {
        if (child?.exitCode === null) await releaseSessions(base);
      } finally {
        await kill();
      }
    }
  },
);
