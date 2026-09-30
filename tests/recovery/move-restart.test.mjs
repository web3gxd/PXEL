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
  calculation,
  releaseSessions,
} from '../helpers/client.mjs';

test(
  'frozen move survives hard restart with calculation hash, receipt and delayed-binding barrier',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18085';
    let child;
    async function start() {
      child = spawn(
        resolve(
          process.env.PXEL_RECOVERY_SERVER_BINARY ??
            'target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''),
        ),
        [],
        {
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18085' },
          stdio: 'ignore',
          windowsHide: true,
        },
      );
      let error;
      child.on('error', (e) => {
        error = e;
      });
      for (let i = 0; i < 100; i++) {
        if (error) throw error;
        if (child.exitCode !== null)
          throw new Error('Move recovery server exited before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('Move recovery readiness timeout');
    }
    async function kill() {
      if (child && child.exitCode === null) {
        const exited = once(child, 'exit');
        child.kill('SIGKILL');
        await exited;
      }
    }
    let client;
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
      const delayed = formulaCommand(client, book, '=A1', { column: 3, sequence: 4 });
      const move = moveRangeCommand(
        client,
        book,
        { destinationRow: 1, destinationColumn: 0, rows: 1, columns: 2 },
        { sequence: 5 },
      );
      const committed = await edit(move);
      const expected = book;
      const beforeCalculation = await calculation(base, cookie, book.document_id, book.version);
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, expected.document_id);
      assert.equal(book.version, '5');
      assert.equal(book.state_hash, expected.state_hash);
      assert.deepEqual(book.sheets, expected.sheets);
      const afterCalculation = await calculation(base, cookie, book.document_id, book.version);
      assert.equal(afterCalculation.calculated_state_hash, beforeCalculation.calculated_state_hash);
      assert.deepEqual(afterCalculation.values, beforeCalculation.values);
      client = await connect(base, cookie);
      const receipt = await client.send(move);
      assert.equal(receipt.case, 'receipt');
      assert.equal(receipt.value.committedVersion, committed.value.committedVersion);
      const denied = await client.send(delayed);
      assert.equal(denied.case, 'rejected');
      assert.equal(denied.value.code, 17);
      assert.equal(
        (await getDocument(base, cookie, book.document_id)).state_hash,
        expected.state_hash,
      );
      await edit(setCommand(client, book, 10, { row: 1, sequence: 6 }));
      const later = await client.send(move);
      assert.equal(later.case, 'receipt');
      assert.equal(later.value.committedVersion, 5n);
      assert.equal(later.value.currentHeadVersion, 6n);
      assert.deepEqual(later.value.canonicalEventHash, receipt.value.canonicalEventHash);
      client.socket.terminate();
      await kill();
      await start();
      const restored = await getDocument(base, cookie, book.document_id);
      assert.equal(restored.version, '6');
      assert.equal(restored.state_hash, book.state_hash);
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
