import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { setTimeout } from 'node:timers/promises';
import { test } from 'node:test';
import {
  calculation,
  releaseSessions,
  formulaCommand,
  connect,
  createDocument,
  getDocument,
  login,
  setCommand,
} from '../helpers/client.mjs';

test(
  'hard server restart replays content and resolves a discarded ACK by operation identity',
  { timeout: 40000 },
  async () => {
    const base = 'http://127.0.0.1:18082';
    let child;
    async function start() {
      child = spawn(
        resolve('target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : '')),
        [],
        {
          env: { ...process.env, PXEL_DEV_AUTH: '1', PXEL_BIND: '127.0.0.1:18082' },
          stdio: 'ignore',
          windowsHide: true,
        },
      );
      let failure;
      child.on('error', (e) => {
        failure = e;
      });
      for (let i = 0; i < 100; i++) {
        if (failure) throw failure;
        if (child.exitCode !== null) throw new Error('Recovery server exited before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await setTimeout(100);
      }
      throw new Error('Recovery server readiness timeout');
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
      const { snapshot } = await createDocument(base, cookie);
      let client = await connect(base, cookie);
      const command = setCommand(client, snapshot, 'durable after crash');
      // Harness observes durability but deliberately does not advance client confirmed state.
      const ack = await client.send(command);
      assert.equal(ack.case, 'committed');
      client.socket.terminate();
      await kill();
      await start();
      const restored = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(restored.version, '2');
      assert.equal(restored.state_hash, Buffer.from(ack.value.confirmedStateHash).toString('hex'));
      assert.equal(restored.sheets[0].cells[0].value.value, 'durable after crash');
      assert.deepEqual(restored.sheets[0].rows, snapshot.sheets[0].rows);
      client = await connect(base, cookie);
      try {
        assert.deepEqual(client.actorId, command.actorId);
        const retry = await client.send(command);
        assert.equal(retry.case, 'receipt');
        assert.equal(retry.value.committedVersion, 2n);
        assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '2');
        const formula = formulaCommand(client, restored, '=SUM(2,3)', { column: 1, sequence: 2 });
        assert.equal((await client.send(formula)).case, 'committed');
        const before = await calculation(base, cookie, snapshot.document_id, 3);
        assert.equal(before.status, 'complete');
        assert.deepEqual(before.values[0].value, { type: 'number', value: 5 });
        client.socket.terminate();
        await kill();
        await start();
        const recovered = await getDocument(base, cookie, snapshot.document_id);
        assert.equal(recovered.version, '3');
        assert.equal(
          recovered.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          '=SUM(2,3)',
        );
        const after = await calculation(base, cookie, snapshot.document_id, 3);
        assert.equal(after.status, 'complete');
        assert.equal(after.calculated_state_hash, before.calculated_state_hash);
        assert.deepEqual(after.values, before.values);
      } finally {
        client.socket.terminate();
      }
    } finally {
      try {
        await releaseSessions(base);
      } finally {
        await kill();
      }
    }
  },
);
