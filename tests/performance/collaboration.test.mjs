import assert from 'node:assert/strict';
import { cpus, platform, release, totalmem } from 'node:os';
import { performance } from 'node:perf_hooks';
import { test } from 'node:test';
import {
  connect,
  createDocument,
  login,
  nextFrame,
  setCommand,
  subscribeDocument,
  receiveFrames,
} from '../helpers/client.mjs';

const base = process.env.PXEL_BENCH_SERVER ?? 'http://127.0.0.1:8080';
const warmup = Number(process.env.PXEL_BENCH_WARMUP ?? 100);
const samples = Number(process.env.PXEL_BENCH_SAMPLES ?? 1_000);
const runs = Number(process.env.PXEL_BENCH_RUNS ?? 5);
const database = process.env.DATABASE_URL ? new URL(process.env.DATABASE_URL) : undefined;
if (![warmup, samples, runs].every((value) => Number.isInteger(value) && value > 0))
  throw new Error('Benchmark sizes must be positive integers');

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.ceil(sorted.length * fraction) - 1];
}

test(
  'durable ACK and second-client visibility on real PostgreSQL',
  { timeout: 600_000 },
  async () => {
    const cookie = await login(base);
    const results = [];
    for (let run = 1; run <= runs; run++) {
      const { snapshot } = await createDocument(base, cookie);
      const writer = await connect(base, cookie);
      const reader = await connect(base, cookie);
      try {
        const [barrier] = await receiveFrames(reader, 1, () =>
          subscribeDocument(reader, snapshot.document_id, 1),
        );
        assert.equal(barrier.case, 'replayComplete');
        const ackLatencies = [];
        const visibilityLatencies = [];
        for (let index = 0; index < warmup + samples; index++) {
          const sequence = index + 1;
          const update = nextFrame(reader).then((frame) => ({ frame, at: performance.now() }));
          const command = setCommand(writer, snapshot, sequence, { sequence });
          const started = performance.now();
          const ack = await writer.send(command);
          const acknowledgedAt = performance.now();
          const delivered = await update;
          assert.equal(ack.case, 'committed');
          assert.equal(delivered.frame.case, 'documentHeadChanged');
          assert.equal(ack.value.committedVersion, BigInt(sequence + 1));
          assert.equal(delivered.frame.value.headVersion, BigInt(sequence + 1));
          if (index >= warmup) {
            ackLatencies.push(acknowledgedAt - started);
            visibilityLatencies.push(delivered.at - started);
          }
        }
        results.push({
          run,
          ack_ms: {
            p50: percentile(ackLatencies, 0.5),
            p95: percentile(ackLatencies, 0.95),
            p99: percentile(ackLatencies, 0.99),
          },
          second_client_ms: {
            p50: percentile(visibilityLatencies, 0.5),
            p95: percentile(visibilityLatencies, 0.95),
            p99: percentile(visibilityLatencies, 0.99),
          },
        });
      } finally {
        writer.socket.terminate();
        reader.socket.terminate();
      }
    }
    const report = {
      kind: 'PXEL collaboration local benchmark',
      topology: {
        client_server: 'loopback',
        database_host: database?.hostname ?? 'unrecorded',
        database_port: database?.port ?? 'unrecorded',
        server_build: process.env.PXEL_BENCH_SERVER_PROFILE ?? 'unverified',
      },
      process: {
        node: process.version,
        platform: platform(),
        release: release(),
        cpu: cpus()[0]?.model,
        logical_cpus: cpus().length,
        ram_gib: +(totalmem() / 1024 ** 3).toFixed(2),
      },
      protocol: {
        warmup_per_run: warmup,
        samples_per_run: samples,
        runs,
        one_document_per_run: true,
        one_cell_rewritten_sequentially: true,
      },
      results,
    };
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  },
);
