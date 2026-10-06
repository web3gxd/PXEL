import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  api,
  connect,
  createDocument,
  login,
  nextFrame,
  setCommand,
  subscribeDocument,
  receiveFrames,
} from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const writes = Number(process.env.PXEL_COMBINED_STRESS_WRITES ?? 600);
if (!Number.isInteger(writes) || writes < 100 || writes > 1_000)
  throw new Error('PXEL_COMBINED_STRESS_WRITES must be in 100..1000');

function percentiles(values) {
  const sorted = [...values].sort((left, right) => left - right);
  return {
    p50_ms: sorted[Math.ceil(sorted.length * 0.5) - 1],
    p95_ms: sorted[Math.ceil(sorted.length * 0.95) - 1],
    max_ms: sorted.at(-1),
  };
}

test(
  'durable writes remain available during concurrent database reads and a stalled subscriber',
  { timeout: 120_000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const writer = await connect(base, cookie);
    const observer = await connect(base, cookie);
    const stalled = await connect(base, cookie);
    const observedVersions = [];
    const observedHeads = [];
    const readLatencies = [];
    const ackLatencies = [];
    const visibilityLatencies = [];

    try {
      const [barrier] = await receiveFrames(observer, 1, () =>
        subscribeDocument(observer, snapshot.document_id, 1),
      );
      assert.equal(barrier.case, 'replayComplete');
      await receiveFrames(stalled, 1, () => subscribeDocument(stalled, snapshot.document_id, 1));
      stalled.socket._socket.pause();

      const databaseReaders = Array.from({ length: 2 }, async () => {
        let previousVersion = 0;
        for (let index = 0; index < 60; index++) {
          const started = performance.now();
          const response = await api(base, cookie, `/api/documents/${snapshot.document_id}`);
          assert.equal(response.status, 200);
          const current = await response.json();
          const version = Number(current.version);
          assert.ok(
            version >= previousVersion,
            'HTTP projection must not move behind its own read',
          );
          previousVersion = version;
          observedVersions.push(version);
          readLatencies.push(performance.now() - started);
          await delay(100);
        }
      });

      for (let index = 0; index < writes; index++) {
        const sequence = index + 1;
        const update = nextFrame(observer);
        const value = `${String(sequence).padStart(4, '0')}:${'x'.repeat(30_000)}`;
        const started = performance.now();
        const result = await writer.send(setCommand(writer, snapshot, value, { sequence }));
        const acknowledged = performance.now();
        assert.equal(result.case, 'committed');
        assert.equal(result.value.committedVersion, BigInt(sequence + 1));
        const head = await update;
        const delivered = performance.now();
        assert.equal(head.case, 'documentHeadChanged');
        assert.equal(head.value.headVersion, BigInt(sequence + 1));
        observedHeads.push(Number(head.value.headVersion));
        ackLatencies.push(acknowledged - started);
        visibilityLatencies.push(delivered - started);
        await delay(22);
      }

      await Promise.all(databaseReaders);
      const final = await api(base, cookie, `/api/documents/${snapshot.document_id}`);
      assert.equal(final.status, 200);
      const projection = await final.json();
      assert.equal(projection.version, String(writes + 1));
      assert.equal(
        projection.sheets[0].cells[0].value.value.slice(0, 4),
        String(writes).padStart(4, '0'),
      );
      assert.equal(observedHeads.length, writes);
      assert.ok(observedVersions.length >= 100);

      stalled.socket._socket.resume();
      await delay(500);
      assert.equal(
        stalled.socket.readyState,
        3,
        'stalled subscriber should be disconnected on overflow',
      );
      process.stdout.write(
        `${JSON.stringify({
          writes,
          concurrent_http_reads: observedVersions.length,
          second_client_notifications: observedHeads.length,
          http_read_latency: percentiles(readLatencies),
          durable_ack_latency: percentiles(ackLatencies),
          second_client_visibility_latency: percentiles(visibilityLatencies),
          final_version: projection.version,
          stalled_subscriber_closed: stalled.socket.readyState === 3,
        })}\n`,
      );
    } finally {
      writer.socket.terminate();
      observer.socket.terminate();
      stalled.socket.terminate();
    }
  },
);
