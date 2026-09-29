import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import {
  connect,
  createDocument,
  decodeFrame,
  getDocument,
  login,
  receiveFrames,
  setCommand,
  subscribeDocument,
} from '../helpers/client.mjs';

const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';
const seeds = Number(process.env.PXEL_NETWORK_SEEDS ?? 3);
const commands = Number(process.env.PXEL_NETWORK_COMMANDS ?? 100);
if (!Number.isInteger(seeds) || seeds < 1 || seeds > 100) throw new Error('Invalid seed count');
if (!Number.isInteger(commands) || commands < 2 || commands > 1_000 || commands % 2)
  throw new Error('Command count must be even and within 2..1000');

function generator(seed) {
  let state = seed;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
}

test(
  'seeded live WebSocket writers, receipts and replay converge with PostgreSQL',
  { timeout: 3_600_000 },
  async () => {
    const cookie = await login(base, `net-${Date.now().toString(36)}`);
    for (let scenario = 0; scenario < seeds; scenario++) {
      const random = generator(0x6247a900 ^ scenario);
      const { snapshot } = await createDocument(base, cookie);
      let left = await connect(base, cookie);
      const right = await connect(base, cookie);
      let watcher = await connect(base, cookie);
      const observed = [];
      const expected = new Map();
      let observerError;
      let version = 1n;
      let leftSequence = 0;
      let rightSequence = 0;
      try {
        const [initial] = await receiveFrames(watcher, 1, () =>
          subscribeDocument(watcher, snapshot.document_id, 1),
        );
        assert.equal(initial.case, 'replayComplete');
        assert.equal(initial.value.barrierVersion, 1n);
        const onMessage = (data, binary) => {
          try {
            assert.equal(binary, true);
            const frame = decodeFrame(data);
            if (frame.case !== 'documentHeadChanged') return;
            assert.ok(
              frame.value.latestOperation,
              'value write must have a canonical replay event',
            );
            observed.push(frame);
          } catch (error) {
            observerError = error;
          }
        };
        const waitObserved = async (count) => {
          const deadline = Date.now() + 30_000;
          while (observed.length < count && !observerError && Date.now() < deadline)
            await delay(10);
          if (observerError) throw observerError;
          assert.equal(observed.length, count, 'subscriber missed or duplicated events');
        };
        const disconnectAt = commands >= 40 ? Math.floor(commands / 4) * 2 : 0;
        const reconnectAt = disconnectAt + 20;
        const writerReconnectAt = commands >= 40 ? Math.floor(commands / 6) * 2 : 0;
        let observerLastSeen = 1n;
        watcher.socket.on('message', onMessage);
        try {
          for (let step = 0; step < commands; step += 2) {
            const sameCell = step % 20 === 0;
            const leftAddress = {
              row: sameCell ? 0 : random() % 6,
              column: sameCell ? 0 : random() % 6,
            };
            const rightAddress = {
              row: sameCell ? 0 : random() % 6,
              column: sameCell ? 0 : random() % 6,
            };
            const leftValue = scenario * 10_000 + step + 1;
            const rightValue = scenario * 10_000 + step + 2;
            const baseSnapshot = { ...snapshot, version: String(version) };
            const leftCommand = setCommand(left, baseSnapshot, leftValue, {
              ...leftAddress,
              sequence: ++leftSequence,
            });
            const rightCommand = setCommand(right, baseSnapshot, rightValue, {
              ...rightAddress,
              sequence: ++rightSequence,
            });
            const [leftAck, rightAck] = await Promise.all([
              delay(random() % 3).then(() => left.send(leftCommand)),
              delay(random() % 3).then(() => right.send(rightCommand)),
            ]);
            assert.equal(leftAck.case, 'committed');
            assert.equal(rightAck.case, 'committed');
            assert.deepEqual(
              [leftAck.value.committedVersion, rightAck.value.committedVersion].sort((a, b) =>
                Number(a - b),
              ),
              [version + 1n, version + 2n],
            );
            for (const [address, value, ack] of [
              [leftAddress, leftValue, leftAck],
              [rightAddress, rightValue, rightAck],
            ]) {
              const key = `${address.row}:${address.column}`;
              const previous = expected.get(key);
              if (!previous || ack.value.committedVersion > previous.version)
                expected.set(key, { version: ack.value.committedVersion, value, ...address });
            }
            version += 2n;
            if (step % 30 === 0) {
              const retry = await left.send(leftCommand);
              assert.equal(retry.case, 'receipt');
              assert.equal(retry.value.committedVersion, leftAck.value.committedVersion);
            }
            if (observerError) throw observerError;
            if (step + 2 === writerReconnectAt) {
              left.socket.terminate();
              left = await connect(base, cookie);
            }
            if (step + 2 === disconnectAt) {
              await waitObserved(step + 2);
              observerLastSeen = version;
              watcher.socket.off('message', onMessage);
              watcher.socket.terminate();
            }
            if (step + 2 === reconnectAt) {
              watcher = await connect(base, cookie);
              const count = Number(version - observerLastSeen);
              const replay = await receiveFrames(watcher, count + 1, () =>
                subscribeDocument(watcher, snapshot.document_id, observerLastSeen),
              );
              assert.deepEqual(
                replay.slice(0, -1).map((frame) => frame.value.headVersion),
                Array.from({ length: count }, (_, index) => observerLastSeen + BigInt(index + 1)),
              );
              assert.equal(replay.at(-1).case, 'replayComplete');
              assert.equal(replay.at(-1).value.resyncRequired, false);
              observed.push(...replay.slice(0, -1));
              watcher.socket.on('message', onMessage);
            }
          }
          await waitObserved(commands);
          assert.deepEqual(
            observed.map((frame) => frame.value.headVersion),
            Array.from({ length: commands }, (_, index) => BigInt(index + 2)),
          );
        } finally {
          watcher.socket.off('message', onMessage);
        }

        const canonical = await getDocument(base, cookie, snapshot.document_id);
        assert.equal(canonical.version, String(version));
        assert.equal(
          Buffer.from(observed.at(-1).value.latestOperation.confirmedStateHash).toString('hex'),
          canonical.state_hash,
        );
        for (const cell of expected.values()) {
          const projected = canonical.sheets[0].cells.find(
            (entry) =>
              entry.row_id === snapshot.sheets[0].rows[cell.row] &&
              entry.column_id === snapshot.sheets[0].columns[cell.column],
          );
          assert.equal(projected?.value?.value, cell.value, 'last server-ordered write wins');
        }

        const lastSeen = version > 101n ? version - 100n : 1n;
        const returning = await connect(base, cookie);
        try {
          const expectedFrames = Number(version - lastSeen) + 1;
          const frames = await receiveFrames(returning, expectedFrames, () =>
            subscribeDocument(returning, snapshot.document_id, lastSeen),
          );
          const events = frames.slice(0, -1);
          assert.deepEqual(
            events.map((frame) => frame.value.headVersion),
            Array.from({ length: expectedFrames - 1 }, (_, index) => lastSeen + BigInt(index + 1)),
          );
          assert.equal(frames.at(-1).case, 'replayComplete');
          assert.equal(frames.at(-1).value.barrierVersion, version);
          assert.equal(frames.at(-1).value.resyncRequired, false);
          assert.equal(
            Buffer.from(events.at(-1).value.latestOperation.confirmedStateHash).toString('hex'),
            canonical.state_hash,
          );
        } finally {
          returning.socket.terminate();
        }
      } finally {
        left.socket.terminate();
        right.socket.terminate();
        watcher.socket.terminate();
      }
      if ((scenario + 1) % 10 === 0 || scenario + 1 === seeds)
        process.stdout.write(`Network scenarios verified: ${scenario + 1}/${seeds}.\n`);
    }
    process.stdout.write(`Verified ${seeds} network scenarios × ${commands} committed writes.\n`);
  },
);
