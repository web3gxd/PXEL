import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  connect,
  createDocument,
  getDocument,
  login,
  receiveFrames,
  releaseSessions,
  setCommand,
  subscribeDocument,
} from '../helpers/client.mjs';

const base = 'http://127.0.0.1:18083';

test(
  'two writers converge through hard restart, receipt retry and journal replay',
  { timeout: 120_000 },
  async () => {
    let server;
    async function start() {
      server = spawn(
        resolve(
          process.env.PXEL_RECOVERY_SERVER_BINARY ??
            'target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''),
        ),
        [],
        {
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18083' },
          stdio: 'ignore',
          windowsHide: true,
        },
      );
      let failure;
      server.on('error', (error) => {
        failure = error;
      });
      for (let attempt = 0; attempt < 100; attempt++) {
        if (failure) throw failure;
        if (server.exitCode !== null || server.signalCode !== null)
          throw new Error('Recovery server stopped before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('Recovery server readiness timeout');
    }
    async function stop() {
      if (server && server.exitCode === null && server.signalCode === null) {
        const exited = once(server, 'exit');
        server.kill('SIGKILL');
        await exited;
      }
    }

    let left;
    let right;
    try {
      await start();
      const cookie = await login(base, `restart-${Date.now().toString(36)}`);
      const { snapshot } = await createDocument(base, cookie);
      left = await connect(base, cookie);
      right = await connect(base, cookie);
      let version = 1n;
      let lastCommand;
      let lastAck;
      for (let round = 0; round < 25; round++) {
        const current = { ...snapshot, version: String(version) };
        const leftCommand = setCommand(left, current, round * 2 + 1, {
          row: 0,
          sequence: round + 1,
        });
        const rightCommand = setCommand(right, current, round * 2 + 2, {
          row: 0,
          sequence: round + 1,
        });
        const [a, b] = await Promise.all([left.send(leftCommand), right.send(rightCommand)]);
        assert.equal(a.case, 'committed');
        assert.equal(b.case, 'committed');
        assert.deepEqual(
          [a.value.committedVersion, b.value.committedVersion].sort((x, y) => Number(x - y)),
          [version + 1n, version + 2n],
        );
        version += 2n;
        lastCommand = rightCommand;
        lastAck = b;
      }
      const before = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(before.version, '51');
      left.socket.terminate();
      right.socket.terminate();
      await stop();
      await start();

      const restored = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(restored.version, before.version);
      assert.equal(restored.state_hash, before.state_hash);
      const replayClient = await connect(base, cookie);
      try {
        const frames = await receiveFrames(replayClient, 51, () =>
          subscribeDocument(replayClient, snapshot.document_id, 1),
        );
        assert.deepEqual(
          frames.slice(0, -1).map((frame) => frame.value.headVersion),
          Array.from({ length: 50 }, (_, index) => BigInt(index + 2)),
        );
        assert.equal(frames.at(-1).case, 'replayComplete');
        assert.equal(frames.at(-1).value.resyncRequired, false);
        assert.equal(
          Buffer.from(frames[49].value.latestOperation.confirmedStateHash).toString('hex'),
          restored.state_hash,
        );
      } finally {
        replayClient.socket.terminate();
      }
      left = await connect(base, cookie);
      right = await connect(base, cookie);
      const receipt = await right.send(lastCommand);
      assert.equal(receipt.case, 'receipt');
      assert.equal(receipt.value.committedVersion, lastAck.value.committedVersion);
      assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '51');
      const next = { ...snapshot, version: '51' };
      const [a, b] = await Promise.all([
        left.send(setCommand(left, next, 1001, { row: 1, sequence: 26 })),
        right.send(setCommand(right, next, 1002, { row: 1, sequence: 26 })),
      ]);
      assert.deepEqual(
        [a.value.committedVersion, b.value.committedVersion].sort((x, y) => Number(x - y)),
        [52n, 53n],
      );
      const final = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(final.version, '53');
      const winner = a.value.committedVersion > b.value.committedVersion ? 1001 : 1002;
      assert.equal(
        final.sheets[0].cells.find((cell) => cell.row_id === snapshot.sheets[0].rows[1])?.value
          .value,
        winner,
      );
      assert.equal(
        final.state_hash,
        Buffer.from(
          (a.value.committedVersion > b.value.committedVersion ? a : b).value.confirmedStateHash,
        ).toString('hex'),
      );
    } finally {
      left?.socket.terminate();
      right?.socket.terminate();
      try {
        if (server && server.exitCode === null && server.signalCode === null)
          await releaseSessions(base);
      } finally {
        await stop();
      }
    }
  },
);
