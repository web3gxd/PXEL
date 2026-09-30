import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { readFileSync, openSync, closeSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  bytes,
  login,
  createDocument,
  connect,
  getDocument,
  setCommand,
  formulaCommand,
  undoCellCommand,
  calculation,
  releaseSessions,
  uuid7,
} from '../helpers/client.mjs';

const require = createRequire(resolve('apps/web/package.json'));
const { create, createFileRegistry, fromBinary, toBinary } = require('@bufbuild/protobuf');
const { FileDescriptorSetSchema } = require('@bufbuild/protobuf/wkt');
const registry = createFileRegistry(
  fromBinary(FileDescriptorSetSchema, readFileSync('proto/wire-descriptor.bin')),
);
const clientSchema = registry.getMessage('pxel.wire.v1.ClientEnvelope');
const snapshotSchema = registry.getMessage('pxel.storage.v1.SnapshotEnvelope');

function query(sql) {
  const database = new URL(process.env.DATABASE_URL);
  const result = spawnSync(
    join(process.env.PXEL_PG_BIN ?? '', 'psql' + (process.platform === 'win32' ? '.exe' : '')),
    ['-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-c', sql],
    {
      env: {
        ...process.env,
        PGHOST: database.hostname,
        PGPORT: database.port || '5432',
        PGUSER: decodeURIComponent(database.username),
        PGPASSWORD: decodeURIComponent(database.password),
        PGDATABASE: decodeURIComponent(database.pathname.slice(1)),
      },
      encoding: 'utf8',
      windowsHide: true,
      timeout: 10000,
    },
  );
  // Never include database URLs, driver stderr or connection credentials in reports.
  assert.equal(result.status, 0, 'Private snapshot metadata query failed');
  return result.stdout.trim();
}
function allLeaves() {
  const set = (value) => ({ action: { case: 'set', value } });
  return {
    schemaVersion: 1,
    profileVersion: 1,
    changes: [
      ['fontFamily', 'Courier New'],
      ['fontSize', 1200],
      ['fontBold', true],
      ['fontItalic', true],
      ['fontUnderline', 3],
      ['fontStrike', true],
      ['fontColor', Uint8Array.from([255, 128, 0])],
      ['fill', Uint8Array.from([12, 34, 56])],
      ...['borderTop', 'borderRight', 'borderBottom', 'borderLeft'].map((edge) => [
        edge,
        { line: 6, color: Uint8Array.from([64, 96, 128]) },
      ]),
      ['horizontal', 3],
      ['vertical', 2],
      ['wrap', true],
      ['numberFormat', 3],
    ].map(([name, value]) => ({ kind: { case: name, value: set(value) } })),
  };
}
const expectedStyle = {
  font_family: 'Courier New',
  font_size_hundredth_points: 1200,
  font_bold: true,
  font_italic: true,
  font_underline: 'double',
  font_strike: true,
  font_color: [255, 128, 0],
  fill: [12, 34, 56],
  border_top: { line: 'double', color: [64, 96, 128] },
  border_right: { line: 'double', color: [64, 96, 128] },
  border_bottom: { line: 'double', color: [64, 96, 128] },
  border_left: { line: 'double', color: [64, 96, 128] },
  horizontal: 'center',
  vertical: 'center',
  wrap: true,
  number_format: 'decimal2',
};

test(
  'snapshot4 style range and journal tail survive hard kills with unobserved inverse ACK and late receipts',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18090';
    const probe = createServer();
    probe.listen(18090, '127.0.0.1');
    await once(probe, 'listening');
    probe.close();
    await once(probe, 'close');
    const blobRoot = resolve(`.local/styles-recovery-${uuid7()}`);
    let child, client;
    const logPath = resolve('.local/styles-recovery-server.log');
    let startCount = 0,
      logOffset = 0;
    async function start() {
      logOffset = existsSync(logPath) ? readFileSync(logPath).length : 0;
      const log = openSync(logPath, 'a');
      child = spawn(
        resolve(
          process.env.PXEL_RECOVERY_SERVER_BINARY ??
            'target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''),
        ),
        [],
        {
          env: {
            ...process.env,
            PXEL_DEV_AUTH: '1',
            PXEL_BIND: '127.0.0.1:18090',
            PXEL_BLOB_ROOT: blobRoot,
            PXEL_SNAPSHOT_INTERVAL: startCount++ === 0 ? '3' : '100000',
          },
          stdio: ['ignore', log, log],
          windowsHide: true,
        },
      );
      closeSync(log);
      let failure;
      child.on('error', (error) => {
        failure = error;
      });
      for (let attempt = 0; attempt < 100; attempt++) {
        if (failure) throw failure;
        if (child.exitCode !== null || child.signalCode !== null)
          throw new Error('Style recovery server exited before readiness');
        try {
          if ((await fetch(base + '/readyz', { signal: AbortSignal.timeout(300) })).ok) return;
        } catch {}
        await delay(100);
      }
      throw new Error('Style recovery server readiness timeout');
    }
    async function kill() {
      if (child && child.exitCode === null && child.signalCode === null) {
        const exited = once(child, 'exit');
        child.kill('SIGKILL');
        await exited;
      }
    }
    function recoveredFromSnapshot(replayedEvents) {
      const records = readFileSync(logPath)
        .subarray(logOffset)
        .toString('utf8')
        .trim()
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line));
      assert.ok(
        records.some(
          (record) =>
            record.fields?.event === 'document_recovery' &&
            record.fields.snapshot_version === 4 &&
            record.fields.replayed_events === replayedEvents,
        ),
        'Recovery must actually select snapshot4 and replay the expected journal tail',
      );
    }
    try {
      await start();
      const cookie = await login(base, `styles-restart-${Date.now().toString(36)}`);
      let { snapshot: book } = await createDocument(base, cookie);
      client = await connect(base, cookie);
      async function edit(command, version) {
        const result = await client.send(command);
        assert.equal(result.case, 'committed');
        assert.equal(result.value.committedVersion, BigInt(version));
        book = await getDocument(base, cookie, book.document_id);
        assert.equal(book.version, String(version));
        return result;
      }
      await edit(setCommand(client, book, 3), 2);
      await edit(formulaCommand(client, book, '=A1*2', { column: 1, sequence: 2 }), 3);
      const sheet = book.sheets[0];
      const style = {
        operationId: bytes(uuid7()),
        documentId: bytes(book.document_id),
        actorId: client.actorId,
        baseVersion: BigInt(book.version),
        clientSequence: 3n,
        setRangeStyle: {
          addresses: [0, 1, 2].map((column) => ({
            sheetId: bytes(sheet.id),
            rowId: bytes(sheet.rows[0]),
            columnId: bytes(sheet.columns[column]),
          })),
          patch: allLeaves(),
        },
      };
      await edit(style, 4);
      assert.equal(book.sheets[0].styles.length, 3);
      assert.equal(book.sheets[0].cells.length, 2);
      for (const cell of book.sheets[0].styles) assert.deepEqual(cell.style, expectedStyle);
      const atSnapshot = book;
      assert.match(
        book.document_id,
        /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      );
      const activeSql = `SELECT row_to_json(s) FROM (SELECT snapshot_version::text, snapshot_schema_version, object_key::text, byte_length::text, encode(payload_checksum,'hex') AS checksum, encode(state_hash,'hex') AS hash FROM document_snapshots WHERE document_id='${book.document_id}' AND state='ACTIVE' ORDER BY snapshot_version DESC LIMIT 1) s`;
      let metadata;
      const deadline = Date.now() + 10000;
      do {
        const row = query(activeSql);
        if (row) {
          metadata = JSON.parse(row);
          break;
        }
        await delay(50);
      } while (Date.now() < deadline);
      assert.ok(metadata, 'Scheduled snapshot did not activate');
      assert.equal(metadata.snapshot_version, '4');
      assert.equal(metadata.snapshot_schema_version, 4);
      assert.equal(metadata.hash, atSnapshot.state_hash);
      assert.match(metadata.object_key, /^[0-9a-f-]{36}$/);
      const file = readFileSync(join(blobRoot, metadata.object_key));
      assert.equal(file.subarray(0, 8).toString('hex'), '5058454c424c4201');
      const payload = file.subarray(48);
      assert.equal(BigInt(payload.length), BigInt(metadata.byte_length));
      assert.equal(file.readBigUInt64LE(8), BigInt(payload.length));
      const checksum = createHash('sha256').update(payload).digest('hex');
      assert.equal(checksum, metadata.checksum);
      assert.equal(file.subarray(16, 48).toString('hex'), checksum);
      const frozen = fromBinary(snapshotSchema, payload);
      assert.equal(frozen.snapshotSchemaVersion, 4);
      assert.equal(frozen.documentVersion, 4n);
      assert.equal(frozen.sheets[0].styles.length, 3);
      assert.equal(frozen.sheets[0].cells.length, 2);
      for (const cell of frozen.sheets[0].styles) assert.equal(cell.style.changes.length, 16);
      await edit(setCommand(client, book, 9, { row: 1, sequence: 4 }), 5);
      const styledWithTail = book;
      const lostUndo = undoCellCommand(client, book, style.operationId, { sequence: 5 });
      // Intentionally install no ACK receiver; independent HTTP reads prove the
      // compensation is durable before terminating the socket and owned process.
      client.socket.send(
        toBinary(
          clientSchema,
          create(clientSchema, {
            protocolVersion: 1,
            payload: { case: 'command', value: lostUndo },
          }),
        ),
      );
      const commitDeadline = Date.now() + 10000;
      do {
        book = await getDocument(base, cookie, book.document_id);
        if (book.version === '6') break;
        await delay(20);
      } while (Date.now() < commitDeadline);
      assert.equal(book.version, '6');
      assert.deepEqual(book.sheets[0].styles, []);
      assert.deepEqual(book.sheets[0].cells, styledWithTail.sheets[0].cells);
      const undone = book,
        undoneCalculation = await calculation(base, cookie, book.document_id, book.version);
      assert.deepEqual(
        undoneCalculation.values.map((cell) => cell.value),
        [{ type: 'number', value: 6 }],
      );
      assert.equal(
        query(
          `SELECT count(*) FROM document_operations WHERE document_id='${book.document_id}' AND committed_version>4`,
        ),
        '2',
      );
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.deepEqual(book, undone);
      recoveredFromSnapshot(2);
      const recoveredCalculation = await calculation(base, cookie, book.document_id, book.version);
      assert.equal(
        recoveredCalculation.calculated_state_hash,
        undoneCalculation.calculated_state_hash,
      );
      assert.deepEqual(recoveredCalculation.values, undoneCalculation.values);
      client = await connect(base, cookie);
      const retried = await client.send(lostUndo);
      assert.equal(retried.case, 'receipt');
      assert.equal(retried.value.committedVersion, 6n);
      assert.equal(retried.value.currentHeadVersion, 6n);
      assert.deepEqual(await getDocument(base, cookie, book.document_id), undone);
      const redo = undoCellCommand(client, book, lostUndo.operationId, { sequence: 6 });
      await edit(redo, 7);
      assert.deepEqual(book.sheets, styledWithTail.sheets);
      const redone = book;
      const redoCalculation = await calculation(base, cookie, book.document_id, book.version);
      client.socket.terminate();
      await kill();
      await start();
      book = await getDocument(base, cookie, book.document_id);
      assert.deepEqual(book, redone);
      recoveredFromSnapshot(3);
      const afterRedoRestart = await calculation(base, cookie, book.document_id, book.version);
      assert.equal(afterRedoRestart.calculated_state_hash, redoCalculation.calculated_state_hash);
      assert.deepEqual(afterRedoRestart.values, redoCalculation.values);
      client = await connect(base, cookie);
      for (const [command, version] of [
        [style, 4n],
        [lostUndo, 6n],
        [redo, 7n],
      ]) {
        const receipt = await client.send(command);
        assert.equal(receipt.case, 'receipt');
        assert.equal(receipt.value.committedVersion, version);
        assert.equal(receipt.value.currentHeadVersion, 7n);
        assert.deepEqual(await getDocument(base, cookie, book.document_id), redone);
      }
      assert.equal(JSON.parse(query(activeSql)).snapshot_version, '4');
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
