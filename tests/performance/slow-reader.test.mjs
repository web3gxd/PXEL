import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  connect,
  createDocument,
  getDocument,
  login,
  receiveFrames,
  setCommand,
  subscribeDocument,
} from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const commands = Number(process.env.PXEL_SLOW_READER_COMMANDS ?? 512);
if (!Number.isInteger(commands) || commands < 1 || commands > 1000)
  throw new Error('PXEL_SLOW_READER_COMMANDS must be in 1..1000');

test(
  'paused subscriber does not block durable writer or canonical head',
  { timeout: 60000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const writer = await connect(base, cookie);
    const slow = await connect(base, cookie);
    try {
      const [barrier] = await receiveFrames(slow, 1, () =>
        subscribeDocument(slow, snapshot.document_id, 1),
      );
      assert.equal(barrier.case, 'replayComplete');
      slow.socket._socket.pause();
      for (let index = 0; index < commands; index++) {
        const text = `${index.toString().padStart(4, '0')}:${'x'.repeat(30000)}`;
        const result = await writer.send(
          setCommand(writer, snapshot, text, { sequence: index + 1 }),
        );
        assert.equal(result.case, 'committed');
        assert.equal(result.value.committedVersion, BigInt(index + 2));
        // Keep below the global 64 commands/s development limiter.
        await delay(22);
      }
      const final = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(final.version, String(commands + 1));
      assert.equal(
        final.sheets[0].cells[0].value.value.slice(0, 4),
        String(commands - 1).padStart(4, '0'),
      );
      slow.socket._socket.resume();
      await delay(500);
      assert.equal(
        slow.socket.readyState,
        3,
        'Paused subscriber should be disconnected after overflow or send timeout',
      );
      process.stdout.write(
        `${JSON.stringify({
          commands,
          payload_bytes_each: 30005,
          durable_head: final.version,
          slow_socket_ready_state: slow.socket.readyState,
        })}\n`,
      );
    } finally {
      slow.socket.terminate();
      writer.socket.terminate();
    }
  },
);
