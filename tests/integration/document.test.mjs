import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  api,
  bytes,
  connect,
  createDocument,
  formulaCommand,
  calculation,
  requestCalculation,
  getDocument,
  login,
  nextFrame,
  setCommand,
  subscribeDocument,
  uuid7,
} from '../helpers/client.mjs';
const base = process.env.PXEL_TEST_SERVER ?? 'http://127.0.0.1:8080';

test('another socket receives a coalescing head hint and reads canonical state', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const first = await connect(base, cookie);
  const second = await connect(base, cookie);
  try {
    const update = nextFrame(second);
    subscribeDocument(second, snapshot.document_id, snapshot.version);
    const committed = await first.send(setCommand(first, snapshot, 42));
    assert.equal(committed.case, 'committed');
    const hint = await update;
    assert.equal(hint.case, 'documentHeadChanged');
    assert.equal(hint.value.headVersion, 2n);
    assert.equal(
      Buffer.from(hint.value.documentId).toString('hex'),
      snapshot.document_id.replaceAll('-', ''),
    );
    const current = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(current.version, '2');
    assert.equal(current.sheets[0].cells[0].value.value, 42);
    const returning = await connect(base, cookie);
    try {
      const missed = nextFrame(returning);
      subscribeDocument(returning, snapshot.document_id, 1);
      const resumed = await missed;
      assert.equal(resumed.case, 'documentHeadChanged');
      assert.equal(resumed.value.headVersion, 2n);
    } finally {
      returning.socket.close();
    }
  } finally {
    first.socket.close();
    second.socket.close();
  }
});

test('explicit recalc persists one system epoch per operation identity and survives replay', async () => {
  const cookie = await login(base);
  const { snapshot } = await createDocument(base, cookie);
  const client = await connect(base, cookie);
  try {
    const formula = await client.send(formulaCommand(client, snapshot, '=2+3', { sequence: 1 }));
    assert.equal(formula.case, 'committed');
    const operationId = uuid7();
    const path = `/api/documents/${snapshot.document_id}/recalculate`;
    const first = await api(base, cookie, path, 'POST', { operation_id: operationId });
    assert.equal(first.status, 200);
    const receipt = await first.json();
    assert.equal(receipt.committed_version, '3');
    assert.equal(receipt.current_head_version, '3');
    const head = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(head.calculation_context.epoch, '1');
    assert.equal(head.version, '3');
    assert.equal(head.sheets[0].cells[0].value.value.source, '=2+3');
    const { chunks } = await requestCalculation(client, snapshot.document_id, 3);
    const final = chunks.at(-1);
    assert.equal(final.calculationEpoch, 1n);
    assert.equal(
      Buffer.from(final.contextId).toString('hex'),
      head.calculation_context.context_id.replaceAll('-', ''),
    );
    assert.equal(final.cells[0].value.kind.value, 5);
    const duplicate = await api(base, cookie, path, 'POST', { operation_id: operationId });
    assert.equal(duplicate.status, 200);
    assert.equal((await duplicate.json()).committed_version, '3');
    assert.equal(
      (await getDocument(base, cookie, snapshot.document_id)).state_hash,
      head.state_hash,
    );
    const later = await api(base, cookie, path, 'POST', { operation_id: uuid7() });
    assert.equal(later.status, 200);
    assert.equal((await later.json()).committed_version, '4');
    const advanced = await getDocument(base, cookie, snapshot.document_id);
    assert.equal(advanced.calculation_context.epoch, '2');
    assert.notEqual(advanced.calculation_context.context_id, head.calculation_context.context_id);
    const outsider = await login(base, 'epoch_' + uuid7().replaceAll('-', '').slice(0, 18));
    assert.ok(
      [403, 404].includes(
        (await api(base, outsider, path, 'POST', { operation_id: uuid7() })).status,
      ),
    );
    const invalidOrigin = await fetch(base + path, {
      method: 'POST',
      headers: {
        Origin: 'https://other.example',
        Cookie: cookie,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ operation_id: uuid7() }),
    });
    assert.equal(invalidOrigin.status, 403);
  } finally {
    client.socket.terminate();
  }
});

test(
  'calculation WebSocket stream is chunked, complete, read-only and authorized',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      for (let row = 0; row < 24; row++) {
        const result = await client.send(
          formulaCommand(client, snapshot, '="' + 'x'.repeat(16384) + '"', {
            row,
            sequence: row + 1,
          }),
        );
        assert.equal(result.case, 'committed');
      }
      const before = await getDocument(base, cookie, snapshot.document_id);
      const { chunks } = await requestCalculation(client, snapshot.document_id, 25);
      assert.ok(chunks.length >= 2);
      let cells = 0;
      for (let sequence = 0; sequence < chunks.length; sequence++) {
        const chunk = chunks[sequence];
        assert.equal(chunk.patchSequence, sequence);
        assert.equal(chunk.inputVersion, 25n);
        assert.equal(chunk.fullSnapshot, true);
        assert.equal(chunk.errorCode, 0);
        assert.equal(chunk.isFinal, sequence === chunks.length - 1);
        assert.equal(chunk.calculatedStateHash.length, chunk.isFinal ? 32 : 0);
        for (const cell of chunk.cells) {
          assert.equal(cell.value.kind.case, 'text');
          assert.equal(cell.value.kind.value.length, 16384);
          cells++;
        }
      }
      assert.equal(cells, 24);
      const http = await calculation(base, cookie, snapshot.document_id, 25);
      assert.equal(
        Buffer.from(chunks.at(-1).calculatedStateHash).toString('hex'),
        http.calculated_state_hash,
      );
      const after = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(after.version, before.version);
      assert.equal(after.state_hash, before.state_hash);
      const stranger = await connect(
        base,
        await login(base, 'stream_' + uuid7().replaceAll('-', '').slice(0, 18)),
      );
      try {
        assert.equal((await requestCalculation(stranger, snapshot.document_id, 25)).error.code, 3);
      } finally {
        stranger.socket.terminate();
      }
    } finally {
      client.socket.terminate();
    }
  },
);

test(
  'server calculation is versioned, incremental and cycles remain typed values',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      assert.equal((await client.send(setCommand(client, snapshot, 2))).case, 'committed');
      assert.equal(
        (await client.send(setCommand(client, snapshot, 3, { column: 1, sequence: 2 }))).case,
        'committed',
      );
      const formula = await client.send(
        formulaCommand(client, snapshot, '=A1+B1', { column: 2, sequence: 3 }),
      );
      assert.equal(formula.case, 'committed');
      assert.equal(formula.value.committedVersion, 4n);
      let state = await calculation(base, cookie, snapshot.document_id, 4);
      assert.equal(state.status, 'complete');
      assert.match(state.calculated_state_hash, /^[0-9a-f]{64}$/);
      assert.deepEqual(state.values[0].value, { type: 'number', value: 5 });
      await client.send(
        formulaCommand(client, { ...snapshot, version: '4' }, '=10', { column: 3, sequence: 4 }),
      );
      state = await calculation(base, cookie, snapshot.document_id, 5);
      assert.equal(state.evaluated_cells, 1);
      await client.send(setCommand(client, { ...snapshot, version: '5' }, 8, { sequence: 5 }));
      state = await calculation(base, cookie, snapshot.document_id, 6);
      assert.equal(state.evaluated_cells, 1);
      assert.deepEqual(
        state.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[2]).value,
        { type: 'number', value: 11 },
      );
      await client.send(
        formulaCommand(client, { ...snapshot, version: '6' }, '=C1', { column: 2, sequence: 6 }),
      );
      state = await calculation(base, cookie, snapshot.document_id, 7);
      assert.equal(state.status, 'complete');
      assert.deepEqual(
        state.values.find((cell) => cell.column_id === snapshot.sheets[0].columns[2]).value,
        { type: 'error', value: '#CYCLE!' },
      );
      const stranger = await login(base, 'calc_' + uuid7().replaceAll('-', '').slice(0, 20));
      assert.ok(
        [403, 404].includes(
          (await api(base, stranger, `/api/documents/${snapshot.document_id}/calculation`)).status,
        ),
      );
    } finally {
      client.socket.close();
    }
  },
);

test(
  'formula text is server-bound, durable and deduplicated independently of AST',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const client = await connect(base, cookie);
    try {
      const command = formulaCommand(client, snapshot, '=A1+SUM($B$1:B2)', { column: 2 });
      const result = await client.send(command);
      assert.equal(result.case, 'committed');
      const canonical = result.value.canonicalOperation;
      assert.equal(canonical.kind.case, 'setCellFormula');
      const formula = canonical.kind.value.formula;
      assert.equal(formula.source, command.formulaCommand.source);
      assert.equal(formula.ast.schemaVersion, 1);
      assert.equal(formula.boundAtVersion, 1n);
      const reference = formula.ast.nodes.find((node) => node.kind.case === 'cell').kind.value;
      assert.deepEqual(reference.rowId, bytes(snapshot.sheets[0].rows[0]));
      assert.deepEqual(reference.columnId, bytes(snapshot.sheets[0].columns[0]));
      assert.equal((await client.send(command)).case, 'receipt');
      const altered = structuredClone(command);
      altered.formulaCommand.source = '=A2';
      assert.equal((await client.send(altered)).value.code, 6);
      const loaded = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(loaded.version, '2');
      assert.equal(loaded.sheets[0].cells[0].value.type, 'formula');
      assert.equal(loaded.sheets[0].cells[0].value.value.source, formula.source);

      const untrustedAst = setCommand(client, loaded, 0, { sequence: 2 });
      untrustedAst.operation = canonical;
      assert.equal((await client.send(untrustedAst)).value.code, 3);
      const invalid = formulaCommand(client, loaded, '=SUM(', { sequence: 3 });
      assert.equal((await client.send(invalid)).value.code, 11);
      const missingBase = formulaCommand(client, loaded, '=A1', { sequence: 4 });
      missingBase.baseVersion = 0n;
      assert.equal((await client.send(missingBase)).value.code, 10);
      assert.equal((await getDocument(base, cookie, snapshot.document_id)).version, '2');
    } finally {
      client.socket.close();
    }
  },
);

test(
  'create, durable commit, exact retry, collision and stable-ID validation',
  { timeout: 30000 },
  async () => {
    const cookie = await login(base);
    const { intent, snapshot } = await createDocument(base, cookie);
    assert.equal(snapshot.version, '1');
    assert.equal(snapshot.sheets[0].rows.length, 1000);
    assert.equal(snapshot.sheets[0].columns.length, 26);
    assert.deepEqual(snapshot.sheets[0].cells, []);
    const repeated = await api(base, cookie, '/api/documents', 'POST', intent);
    assert.equal(repeated.status, 201);
    assert.deepEqual(await getDocument(base, cookie, intent.document_id), snapshot);
    const client = await connect(base, cookie);
    try {
      const command = setCommand(client, snapshot, 42);
      const committed = await client.send(command);
      assert.equal(committed.case, 'committed');
      assert.equal(committed.value.committedVersion, 2n);
      const duplicate = await client.send(command);
      assert.equal(duplicate.case, 'receipt');
      assert.equal(duplicate.value.committedVersion, 2n);
      const altered = structuredClone(command);
      altered.operation.kind.value.value.kind.value = 43;
      const collision = await client.send(altered);
      assert.equal(collision.case, 'rejected');
      assert.equal(collision.value.code, 6);
      const invalid = setCommand(client, snapshot, 'invalid target', { sequence: 2 });
      invalid.operation.kind.value.address.rowId = bytes(uuid7());
      const rejected = await client.send(invalid);
      assert.equal(rejected.case, 'rejected');
      assert.equal(rejected.value.code, 9);
      const impersonated = setCommand(client, snapshot, 'wrong actor', { sequence: 3 });
      impersonated.actorId = bytes(uuid7());
      const denied = await client.send(impersonated);
      assert.equal(denied.case, 'rejected');
      assert.equal(denied.value.code, 1);
      const current = await getDocument(base, cookie, intent.document_id);
      assert.equal(current.version, '2');
      assert.equal(
        current.state_hash,
        Buffer.from(committed.value.confirmedStateHash).toString('hex'),
      );
      assert.deepEqual(current.sheets[0].cells, [
        {
          row_id: snapshot.sheets[0].rows[0],
          column_id: snapshot.sheets[0].columns[0],
          value: { type: 'number', value: 42 },
        },
      ]);
    } finally {
      client.socket.terminate();
    }
  },
);

test(
  'two connections with stale base get ordered independent property commits',
  { timeout: 20000 },
  async () => {
    const cookie = await login(base);
    const { snapshot } = await createDocument(base, cookie);
    const a = await connect(base, cookie),
      b = await connect(base, cookie);
    try {
      const results = await Promise.all([
        a.send(setCommand(a, snapshot, 'left')),
        b.send(setCommand(b, snapshot, 'right', { column: 1 })),
      ]);
      assert.ok(results.every((r) => r.case === 'committed'));
      assert.deepEqual(results.map((r) => r.value.committedVersion).sort(), [2n, 3n]);
      const state = await getDocument(base, cookie, snapshot.document_id);
      assert.equal(state.version, '3');
      assert.equal(state.sheets[0].cells.length, 2);
      assert.deepEqual(state.sheets[0].cells.map((c) => c.value.value).sort(), ['left', 'right']);
    } finally {
      a.socket.terminate();
      b.socket.terminate();
    }
  },
);

test(
  'another authenticated tenant cannot read or write a document',
  { timeout: 20000 },
  async () => {
    const owner = await login(base);
    const stranger = await login(base, 'stranger_' + uuid7().replaceAll('-', '').slice(-16));
    const { snapshot } = await createDocument(base, owner);
    const response = await api(base, stranger, '/api/documents/' + snapshot.document_id);
    assert.ok([403, 404].includes(response.status));
    const other = await connect(base, stranger);
    try {
      const result = await other.send(setCommand(other, snapshot, 'not allowed'));
      assert.equal(result.case, 'rejected');
      assert.ok([1, 2].includes(result.value.code));
      assert.equal((await getDocument(base, owner, snapshot.document_id)).version, '1');
    } finally {
      other.socket.terminate();
    }
  },
);
