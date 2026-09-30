import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  api,
  bytes,
  login,
  createDocument,
  connect,
  getDocument,
  setCommand,
  undoCellCommand,
  releaseSessions,
} from '../helpers/client.mjs';

const require = createRequire(resolve('apps/web/package.json'));
const { create, createFileRegistry, fromBinary, toBinary } = require('@bufbuild/protobuf');
const { FileDescriptorSetSchema } = require('@bufbuild/protobuf/wkt');
const registry = createFileRegistry(
  fromBinary(FileDescriptorSetSchema, readFileSync('proto/wire-descriptor.bin')),
);
const clientSchema = registry.getMessage('pxel.wire.v1.ClientEnvelope');
function idString(id) {
  const hex = Buffer.from(id).toString('hex');
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join('-');
}
const target = (command, version) => ({
  operation_id: idString(command.operationId),
  committed_version: String(version),
});

test(
  'durable non-top undo discovery survives hard restarts and an unobserved acknowledgement',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18089';
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
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18089' },
          stdio: 'ignore',
          windowsHide: true,
        },
      );
      let failure;
      child.on('error', (error) => {
        failure = error;
      });
      for (let attempt = 0; attempt < 100; attempt++) {
        if (failure) throw failure;
        if (child.exitCode !== null || child.signalCode !== null)
          throw new Error('History recovery server stopped before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('History recovery server readiness timeout');
    }
    async function kill() {
      if (child && child.exitCode === null && child.signalCode === null) {
        const exited = once(child, 'exit');
        child.kill('SIGKILL');
        await exited;
      }
    }
    try {
      await start();
      const cookie = await login(base, `history-restart-${Date.now().toString(36)}`);
      let { snapshot: book } = await createDocument(base, cookie);
      client = await connect(base, cookie);
      const actorId = idString(client.actorId);
      async function history() {
        const response = await api(base, cookie, `/api/documents/${book.document_id}/undo-history`);
        assert.equal(response.status, 200);
        return response.json();
      }
      function expected(version, undo, redo) {
        return {
          schema_version: 1,
          document_id: book.document_id,
          actor_id: actorId,
          through_version: String(version),
          earliest_scanned_version: '1',
          undo,
          redo,
          truncated: false,
          truncation_reason: null,
        };
      }
      async function edit(command, version) {
        const response = await client.send(command);
        assert.equal(response.case, 'committed');
        assert.equal(response.value.committedVersion, BigInt(version));
        book = await getDocument(base, cookie, book.document_id);
        assert.equal(book.version, String(version));
      }
      const a = setCommand(client, book, 11);
      await edit(a, 2);
      const b = setCommand(client, book, 22, { column: 1, sequence: 2 });
      await edit(b, 3);
      const authored = book;
      assert.deepEqual(await history(), expected(3, [target(a, 2), target(b, 3)], []));
      const undoA = undoCellCommand(client, book, a.operationId, { sequence: 3 });
      // Send a genuine wire command without installing an ACK receiver. The test's
      // independent HTTP observer proves durability before the hard kill.
      client.socket.send(
        toBinary(
          clientSchema,
          create(clientSchema, {
            protocolVersion: 1,
            payload: { case: 'command', value: undoA },
          }),
        ),
      );
      let observed;
      const deadline = Date.now() + 10000;
      do {
        observed = await history();
        if (observed.through_version === '4') break;
        await delay(20);
      } while (Date.now() < deadline);
      const nonTop = expected(4, [target(b, 3)], [target(undoA, 4)]);
      assert.deepEqual(observed, nonTop);
      book = await getDocument(base, cookie, book.document_id);
      assert.equal(book.sheets[0].cells.length, 1);
      assert.equal(book.sheets[0].cells[0].value.value, 22);
      const beforeRestart = book;
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.equal(book.state_hash, beforeRestart.state_hash);
      assert.deepEqual(book.sheets, beforeRestart.sheets);
      assert.deepEqual(await history(), nonTop);
      client = await connect(base, cookie);
      assert.equal(idString(client.actorId), actorId);
      const lostAckRetry = await client.send(undoA);
      assert.equal(lostAckRetry.case, 'receipt');
      assert.equal(lostAckRetry.value.committedVersion, 4n);
      assert.equal(lostAckRetry.value.currentHeadVersion, 4n);
      assert.deepEqual(await history(), nonTop);
      const redoA = undoCellCommand(client, book, bytes(nonTop.redo[0].operation_id), {
        sequence: 4,
      });
      await edit(redoA, 5);
      assert.deepEqual(book.sheets, authored.sheets);
      assert.deepEqual(await history(), expected(5, [target(b, 3), target(redoA, 5)], []));
      const undoB = undoCellCommand(client, book, bytes((await history()).undo[0].operation_id), {
        sequence: 5,
      });
      await edit(undoB, 6);
      const pendingRedo = expected(6, [target(redoA, 5)], [target(undoB, 6)]);
      assert.deepEqual(await history(), pendingRedo);
      const secondRestart = book;
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.equal(book.state_hash, secondRestart.state_hash);
      assert.deepEqual(book.sheets, secondRestart.sheets);
      assert.deepEqual(await history(), pendingRedo);
      client = await connect(base, cookie);
      const redoB = undoCellCommand(client, book, bytes((await history()).redo[0].operation_id), {
        sequence: 6,
      });
      await edit(redoB, 7);
      assert.deepEqual(book.sheets, authored.sheets);
      const finalHistory = expected(7, [target(redoA, 5), target(redoB, 7)], []);
      assert.deepEqual(await history(), finalHistory);
      for (const [command, committedVersion] of [
        [undoA, 4n],
        [undoB, 6n],
        [redoB, 7n],
        [a, 2n],
      ]) {
        const receipt = await client.send(command);
        assert.equal(receipt.case, 'receipt');
        assert.equal(receipt.value.committedVersion, committedVersion);
        assert.equal(receipt.value.currentHeadVersion, 7n);
        assert.deepEqual(await history(), finalHistory);
      }
      assert.equal((await getDocument(base, cookie, book.document_id)).state_hash, book.state_hash);
    } finally {
      client?.socket.terminate();
      try {
        if (child?.exitCode === null && child?.signalCode === null) await releaseSessions(base);
      } finally {
        await kill();
      }
    }
  },
);
