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
  moveRangeCommand,
  undoCellCommand,
  calculation,
  releaseSessions,
} from '../helpers/client.mjs';

test(
  'move compensation and redo recover exact contents, calculation and late receipts',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18086';
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
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18086' },
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
        if (child.exitCode !== null) throw new Error('Move history server exited before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('Move history server readiness timeout');
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
      await edit(setCommand(client, book, 4));
      await edit(formulaCommand(client, book, '=A1*2', { column: 1, sequence: 2 }));
      await edit(formulaCommand(client, book, '=B1+1', { column: 2, sequence: 3 }));
      const before = book;
      const move = moveRangeCommand(
        client,
        book,
        { destinationRow: 1, destinationColumn: 0, rows: 1, columns: 2 },
        { sequence: 4 },
      );
      await edit(move);
      const delayed = formulaCommand(client, book, '=A1', { column: 3, sequence: 5 });
      const undo = undoCellCommand(client, book, move.operationId, { sequence: 6 });
      const committedUndo = await edit(undo);
      assert.deepEqual(book.sheets, before.sheets);
      const expectedUndo = book;
      const undoCalculation = await calculation(base, cookie, book.document_id, book.version);
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
      const redo = undoCellCommand(client, book, undo.operationId, { sequence: 7 });
      const committedRedo = await edit(redo);
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
      client = await connect(base, cookie);
      const redoRetry = await client.send(redo);
      assert.equal(redoRetry.case, 'receipt');
      assert.equal(redoRetry.value.committedVersion, committedRedo.value.committedVersion);
      const oldUndo = await client.send(undo);
      assert.equal(oldUndo.case, 'receipt');
      assert.equal(oldUndo.value.committedVersion, committedUndo.value.committedVersion);
      assert.equal(oldUndo.value.currentHeadVersion, committedRedo.value.committedVersion);
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
