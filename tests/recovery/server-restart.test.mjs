import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { setTimeout } from 'node:timers/promises';
import { test } from 'node:test';
import {
  api,
  batchValuesCommand,
  calculation,
  releaseSessions,
  formulaCommand,
  connect,
  copyRangeCommand,
  createDocument,
  deleteColumnsCommand,
  deleteRowsCommand,
  extendRowsCommand,
  insertColumnsCommand,
  insertRowsCommand,
  getDocument,
  login,
  setCommand,
  undoCellCommand,
  uuid7,
} from '../helpers/client.mjs';

test(
  'hard server restart replays content and resolves a discarded ACK by operation identity',
  { timeout: 60000 },
  async () => {
    const base = 'http://127.0.0.1:18082';
    let child;
    async function start() {
      child = spawn(
        resolve(
          process.env.PXEL_RECOVERY_SERVER_BINARY ??
            'target/debug/pxel-server' + (process.platform === 'win32' ? '.exe' : ''),
        ),
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
        const ownerIdentity = await (await api(base, cookie, '/api/dev/identity')).json();
        const recipientAccount = 'recovery_acl_' + uuid7().replaceAll('-', '').slice(-12);
        const recipientHome = await login(base, recipientAccount);
        const recipientIdentity = await (
          await api(base, recipientHome, '/api/dev/identity')
        ).json();
        const aclPath = `/api/documents/${snapshot.document_id}/permissions`;
        const grant = await api(base, cookie, aclPath, 'POST', {
          action: 'grant',
          change_id: uuid7(),
          target_user_id: recipientIdentity.actor_id,
          role: 'viewer',
        });
        assert.equal(grant.status, 200);
        const recipient = await login(base, recipientAccount, ownerIdentity.workspace_id);
        const tableSource = '=VLOOKUP(7,A1:A2,1,FALSE)+HLOOKUP(7,A1:A2,2,FALSE)';
        const { snapshot: tableBook } = await createDocument(base, cookie);
        const tableWriter = await connect(base, cookie);
        assert.equal(
          (await tableWriter.send(setCommand(tableWriter, tableBook, 7))).case,
          'committed',
        );
        assert.equal(
          (
            await tableWriter.send(
              setCommand(tableWriter, { ...tableBook, version: '2' }, 13, { row: 1, sequence: 2 }),
            )
          ).case,
          'committed',
        );
        const tableCommand = formulaCommand(
          tableWriter,
          { ...tableBook, version: '3' },
          tableSource,
          { column: 1, sequence: 3 },
        );
        const tableAck = await tableWriter.send(tableCommand);
        assert.equal(tableAck.case, 'committed');
        const tableBefore = await calculation(base, cookie, tableBook.document_id, 4);
        assert.deepEqual(tableBefore.values[0].value, { type: 'number', value: 20 });
        tableWriter.socket.terminate();
        await kill();
        await start();
        const tableRestored = await getDocument(base, cookie, tableBook.document_id);
        assert.equal(tableRestored.version, '4');
        assert.equal(
          tableRestored.state_hash,
          Buffer.from(tableAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          tableRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          tableSource,
        );
        const tableAfter = await calculation(base, cookie, tableBook.document_id, 4);
        assert.equal(tableAfter.calculated_state_hash, tableBefore.calculated_state_hash);
        assert.deepEqual(tableAfter.values, tableBefore.values);
        const tableEditor = await connect(base, cookie);
        try {
          assert.equal((await tableEditor.send(tableCommand)).case, 'receipt');
          assert.equal(
            (
              await tableEditor.send(
                setCommand(tableEditor, tableRestored, 21, { row: 1, sequence: 4 }),
              )
            ).case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, tableBook.document_id, 5)).values[0].value,
            { type: 'number', value: 28 },
          );
        } finally {
          tableEditor.socket.terminate();
        }
        assert.equal((await getDocument(base, recipient, snapshot.document_id)).version, '3');
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
        const indexSource = '=SUM(INDEX(A1:A2,0))+INDEX(A1:A2,1)';
        const { snapshot: indexBook } = await createDocument(base, cookie);
        const indexWriter = await connect(base, cookie);
        assert.equal(
          (await indexWriter.send(setCommand(indexWriter, indexBook, 7))).case,
          'committed',
        );
        const indexCommand = formulaCommand(
          indexWriter,
          { ...indexBook, version: '2' },
          indexSource,
          { column: 1, sequence: 2 },
        );
        const indexAck = await indexWriter.send(indexCommand);
        assert.equal(indexAck.case, 'committed');
        const indexBefore = await calculation(base, cookie, indexBook.document_id, 3);
        assert.deepEqual(indexBefore.values[0].value, { type: 'number', value: 14 });
        indexWriter.socket.terminate();
        await kill();
        await start();
        const indexRestored = await getDocument(base, cookie, indexBook.document_id);
        assert.equal(indexRestored.version, '3');
        assert.equal(
          indexRestored.state_hash,
          Buffer.from(indexAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          indexRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          indexSource,
        );
        const indexAfter = await calculation(base, cookie, indexBook.document_id, 3);
        assert.equal(indexAfter.calculated_state_hash, indexBefore.calculated_state_hash);
        assert.deepEqual(indexAfter.values, indexBefore.values);
        const indexEditor = await connect(base, cookie);
        try {
          assert.equal((await indexEditor.send(indexCommand)).case, 'receipt');
          assert.equal(
            (await indexEditor.send(setCommand(indexEditor, indexRestored, 13, { sequence: 3 })))
              .case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, indexBook.document_id, 4)).values[0].value,
            { type: 'number', value: 26 },
          );
        } finally {
          indexEditor.socket.terminate();
        }
        const xSource = '=SUM(XLOOKUP(A1,A1:A2,A1:A2))+XLOOKUP(7,A1:A2,A1:A2,9)';
        const { snapshot: xBook } = await createDocument(base, cookie);
        const xWriter = await connect(base, cookie);
        assert.equal((await xWriter.send(setCommand(xWriter, xBook, 7))).case, 'committed');
        const xCommand = formulaCommand(xWriter, { ...xBook, version: '2' }, xSource, {
          column: 1,
          sequence: 2,
        });
        const xAck = await xWriter.send(xCommand);
        assert.equal(xAck.case, 'committed');
        const xBefore = await calculation(base, cookie, xBook.document_id, 3);
        assert.deepEqual(xBefore.values[0].value, { type: 'number', value: 14 });
        xWriter.socket.terminate();
        await kill();
        await start();
        const xRestored = await getDocument(base, cookie, xBook.document_id);
        assert.equal(xRestored.version, '3');
        assert.equal(
          xRestored.state_hash,
          Buffer.from(xAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          xRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          xSource,
        );
        const xAfter = await calculation(base, cookie, xBook.document_id, 3);
        assert.equal(xAfter.calculated_state_hash, xBefore.calculated_state_hash);
        assert.deepEqual(xAfter.values, xBefore.values);
        const xEditor = await connect(base, cookie);
        try {
          assert.equal((await xEditor.send(xCommand)).case, 'receipt');
          assert.equal(
            (await xEditor.send(setCommand(xEditor, xRestored, 13, { sequence: 3 }))).case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, xBook.document_id, 4)).values[0].value,
            { type: 'number', value: 22 },
          );
        } finally {
          xEditor.socket.terminate();
        }
        const multiSource =
          '=SUMIFS(B1:B2,A1:A2,">5",B1:B2,">20")+COUNTIFS(A1:A2,">5",B1:B2,">20")';
        const { snapshot: multiBook } = await createDocument(base, cookie);
        const multiWriter = await connect(base, cookie);
        for (const [index, [row, column, value]] of [
          [0, 0, 7],
          [1, 0, 13],
          [0, 1, 21],
          [1, 1, 29],
        ].entries()) {
          assert.equal(
            (
              await multiWriter.send(
                setCommand(multiWriter, { ...multiBook, version: String(index + 1) }, value, {
                  row,
                  column,
                  sequence: index + 1,
                }),
              )
            ).case,
            'committed',
          );
        }
        const multiCommand = formulaCommand(
          multiWriter,
          { ...multiBook, version: '5' },
          multiSource,
          { column: 2, sequence: 5 },
        );
        const multiAck = await multiWriter.send(multiCommand);
        assert.equal(multiAck.case, 'committed');
        const multiBefore = await calculation(base, cookie, multiBook.document_id, 6);
        assert.deepEqual(multiBefore.values[0].value, { type: 'number', value: 52 });
        multiWriter.socket.terminate();
        await kill();
        await start();
        const multiRestored = await getDocument(base, cookie, multiBook.document_id);
        assert.equal(multiRestored.version, '6');
        assert.equal(
          multiRestored.state_hash,
          Buffer.from(multiAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          multiRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          multiSource,
        );
        const multiAfter = await calculation(base, cookie, multiBook.document_id, 6);
        assert.equal(multiAfter.calculated_state_hash, multiBefore.calculated_state_hash);
        assert.deepEqual(multiAfter.values, multiBefore.values);
        const multiEditor = await connect(base, cookie);
        try {
          assert.equal((await multiEditor.send(multiCommand)).case, 'receipt');
          assert.equal(
            (
              await multiEditor.send(
                setCommand(multiEditor, multiRestored, 19, { row: 1, column: 1, sequence: 6 }),
              )
            ).case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, multiBook.document_id, 7)).values[0].value,
            { type: 'number', value: 22 },
          );
        } finally {
          multiEditor.socket.terminate();
        }
        const criteriaSource = '=SUMIF(A1:A2,">0",B1)+COUNTIF(A1:A2,">0")';
        const { snapshot: criteriaBook } = await createDocument(base, cookie);
        const criteriaWriter = await connect(base, cookie);
        for (const [index, [row, column, value]] of [
          [0, 0, 7],
          [1, 0, 13],
          [0, 1, 21],
          [1, 1, 29],
        ].entries()) {
          assert.equal(
            (
              await criteriaWriter.send(
                setCommand(criteriaWriter, { ...criteriaBook, version: String(index + 1) }, value, {
                  row,
                  column,
                  sequence: index + 1,
                }),
              )
            ).case,
            'committed',
          );
        }
        const criteriaCommand = formulaCommand(
          criteriaWriter,
          { ...criteriaBook, version: '5' },
          criteriaSource,
          { column: 2, sequence: 5 },
        );
        const criteriaAck = await criteriaWriter.send(criteriaCommand);
        assert.equal(criteriaAck.case, 'committed');
        const criteriaBefore = await calculation(base, cookie, criteriaBook.document_id, 6);
        assert.deepEqual(criteriaBefore.values[0].value, { type: 'number', value: 52 });
        criteriaWriter.socket.terminate();
        await kill();
        await start();
        const criteriaRestored = await getDocument(base, cookie, criteriaBook.document_id);
        assert.equal(criteriaRestored.version, '6');
        assert.equal(
          criteriaRestored.state_hash,
          Buffer.from(criteriaAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          criteriaRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          criteriaSource,
        );
        const criteriaAfter = await calculation(base, cookie, criteriaBook.document_id, 6);
        assert.equal(criteriaAfter.calculated_state_hash, criteriaBefore.calculated_state_hash);
        assert.deepEqual(criteriaAfter.values, criteriaBefore.values);
        const criteriaEditor = await connect(base, cookie);
        try {
          assert.equal((await criteriaEditor.send(criteriaCommand)).case, 'receipt');
          assert.equal(
            (
              await criteriaEditor.send(
                setCommand(criteriaEditor, criteriaRestored, 39, {
                  row: 1,
                  column: 1,
                  sequence: 6,
                }),
              )
            ).case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, criteriaBook.document_id, 7)).values[0].value,
            { type: 'number', value: 62 },
          );
        } finally {
          criteriaEditor.socket.terminate();
        }
        const matchSource = '=MATCH(A1+1,A1:A2,1)+IFERROR(MATCH(7,A1:A2,0),9)';
        const { snapshot: matchBook } = await createDocument(base, cookie);
        const matchWriter = await connect(base, cookie);
        assert.equal(
          (await matchWriter.send(setCommand(matchWriter, matchBook, 7))).case,
          'committed',
        );
        const matchCommand = formulaCommand(
          matchWriter,
          { ...matchBook, version: '2' },
          matchSource,
          { column: 1, sequence: 2 },
        );
        const matchAck = await matchWriter.send(matchCommand);
        assert.equal(matchAck.case, 'committed');
        const matchBefore = await calculation(base, cookie, matchBook.document_id, 3);
        assert.deepEqual(matchBefore.values[0].value, { type: 'number', value: 2 });
        matchWriter.socket.terminate();
        await kill();
        await start();
        const matchRestored = await getDocument(base, cookie, matchBook.document_id);
        assert.equal(matchRestored.version, '3');
        assert.equal(
          matchRestored.state_hash,
          Buffer.from(matchAck.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          matchRestored.sheets[0].cells.find((cell) => cell.value.type === 'formula').value.value
            .source,
          matchSource,
        );
        const matchAfter = await calculation(base, cookie, matchBook.document_id, 3);
        assert.equal(matchAfter.calculated_state_hash, matchBefore.calculated_state_hash);
        assert.deepEqual(matchAfter.values, matchBefore.values);
        const matchEditor = await connect(base, cookie);
        try {
          assert.equal((await matchEditor.send(matchCommand)).case, 'receipt');
          assert.equal(
            (await matchEditor.send(setCommand(matchEditor, matchRestored, 13, { sequence: 3 })))
              .case,
            'committed',
          );
          assert.deepEqual(
            (await calculation(base, cookie, matchBook.document_id, 4)).values[0].value,
            { type: 'number', value: 10 },
          );
        } finally {
          matchEditor.socket.terminate();
        }
        assert.equal((await getDocument(base, recipient, snapshot.document_id)).version, '3');
        const revoked = await api(base, cookie, aclPath, 'POST', {
          action: 'revoke',
          change_id: uuid7(),
          target_user_id: recipientIdentity.actor_id,
        });
        assert.equal(revoked.status, 200);
        await kill();
        await start();
        assert.ok(
          [403, 404].includes(
            (await api(base, recipient, '/api/documents/' + snapshot.document_id)).status,
          ),
        );
        const structureWriter = await connect(base, cookie);
        const beforeExtension = await getDocument(base, cookie, snapshot.document_id);
        const appendedRow = uuid7();
        const extension = await structureWriter.send(
          extendRowsCommand(structureWriter, beforeExtension, [appendedRow], { sequence: 3 }),
        );
        assert.equal(extension.case, 'committed');
        assert.equal(extension.value.committedVersion, 4n);
        structureWriter.socket.terminate();
        await kill();
        await start();
        const structuralRecovery = await getDocument(base, cookie, snapshot.document_id);
        assert.equal(structuralRecovery.version, '4');
        assert.equal(structuralRecovery.sheets[0].rows.at(-1), appendedRow);
        assert.equal(
          structuralRecovery.state_hash,
          Buffer.from(extension.value.confirmedStateHash).toString('hex'),
        );
        const { snapshot: insertionBase } = await createDocument(base, cookie);
        const insertionWriter = await connect(base, cookie);
        const referenceFormula = await insertionWriter.send(
          formulaCommand(insertionWriter, insertionBase, '=A2', { column: 1, sequence: 1 }),
        );
        assert.equal(referenceFormula.case, 'committed');
        const newRow = uuid7();
        const insertion = await insertionWriter.send(
          insertRowsCommand(
            insertionWriter,
            { ...insertionBase, version: '2' },
            insertionBase.sheets[0].rows[1],
            [newRow],
            { sequence: 2 },
          ),
        );
        assert.equal(insertion.case, 'committed');
        insertionWriter.socket.terminate();
        await kill();
        await start();
        const insertedRecovery = await getDocument(base, cookie, insertionBase.document_id);
        assert.equal(insertedRecovery.version, '3');
        assert.equal(insertedRecovery.sheets[0].rows[1], newRow);
        assert.equal(insertedRecovery.sheets[0].rows[2], insertionBase.sheets[0].rows[1]);
        const recoveredFormula = insertedRecovery.sheets[0].cells.find(
          (cell) => cell.value.type === 'formula',
        )?.value.value;
        assert.equal(recoveredFormula?.source, '=A2');
        assert.equal(recoveredFormula?.current_text, '=A3');
        assert.equal(
          insertedRecovery.state_hash,
          Buffer.from(insertion.value.confirmedStateHash).toString('hex'),
        );
        const { snapshot: deletionBase } = await createDocument(base, cookie);
        const deletionWriter = await connect(base, cookie);
        const formulaBeforeDelete = await deletionWriter.send(
          formulaCommand(deletionWriter, deletionBase, '=A2', { column: 1, sequence: 1 }),
        );
        assert.equal(formulaBeforeDelete.case, 'committed');
        const removed = deletionBase.sheets[0].rows[1];
        const deletion = await deletionWriter.send(
          deleteRowsCommand(deletionWriter, { ...deletionBase, version: '2' }, [removed], {
            sequence: 2,
          }),
        );
        assert.equal(deletion.case, 'committed');
        deletionWriter.socket.terminate();
        await kill();
        await start();
        const deletedRecovery = await getDocument(base, cookie, deletionBase.document_id);
        assert.equal(deletedRecovery.version, '3');
        assert.equal(deletedRecovery.sheets[0].rows.includes(removed), false);
        assert.equal(
          deletedRecovery.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value.value
            .current_text,
          '=#REF!',
        );
        assert.equal(
          deletedRecovery.state_hash,
          Buffer.from(deletion.value.confirmedStateHash).toString('hex'),
        );
        const { snapshot: columnBase } = await createDocument(base, cookie);
        const columnWriter = await connect(base, cookie);
        const formulaBeforeColumn = await columnWriter.send(
          formulaCommand(columnWriter, columnBase, '=A1', { column: 1, sequence: 1 }),
        );
        assert.equal(formulaBeforeColumn.case, 'committed');
        const insertedColumn = uuid7();
        const columnInsertion = await columnWriter.send(
          insertColumnsCommand(
            columnWriter,
            { ...columnBase, version: '2' },
            columnBase.sheets[0].columns[0],
            [insertedColumn],
            { sequence: 2 },
          ),
        );
        assert.equal(columnInsertion.case, 'committed');
        columnWriter.socket.terminate();
        await kill();
        await start();
        const columnRecovery = await getDocument(base, cookie, columnBase.document_id);
        assert.equal(columnRecovery.version, '3');
        assert.equal(columnRecovery.sheets[0].columns[0], insertedColumn);
        assert.equal(
          columnRecovery.sheets[0].cells.find((cell) => cell.value.type === 'formula')?.value.value
            .current_text,
          '=B1',
        );
        assert.equal(
          columnRecovery.state_hash,
          Buffer.from(columnInsertion.value.confirmedStateHash).toString('hex'),
        );
        const { snapshot: columnDeletionBase } = await createDocument(base, cookie);
        const columnDeletionWriter = await connect(base, cookie);
        const formulaBeforeColumnDelete = await columnDeletionWriter.send(
          formulaCommand(columnDeletionWriter, columnDeletionBase, '=A1', {
            column: 1,
            sequence: 1,
          }),
        );
        assert.equal(formulaBeforeColumnDelete.case, 'committed');
        const deletedColumn = await columnDeletionWriter.send(
          deleteColumnsCommand(
            columnDeletionWriter,
            { ...columnDeletionBase, version: '2' },
            [columnDeletionBase.sheets[0].columns[0]],
            { sequence: 2 },
          ),
        );
        assert.equal(deletedColumn.case, 'committed');
        columnDeletionWriter.socket.terminate();
        await kill();
        await start();
        const columnDeletionRecovery = await getDocument(
          base,
          cookie,
          columnDeletionBase.document_id,
        );
        assert.equal(columnDeletionRecovery.version, '3');
        assert.equal(columnDeletionRecovery.sheets[0].columns.length, 25);
        assert.equal(
          columnDeletionRecovery.sheets[0].cells.find((cell) => cell.value.type === 'formula')
            ?.value.value.current_text,
          '=#REF!',
        );
        assert.equal(
          columnDeletionRecovery.state_hash,
          Buffer.from(deletedColumn.value.confirmedStateHash).toString('hex'),
        );
        const { snapshot: batchBase } = await createDocument(base, cookie);
        const batchWriter = await connect(base, cookie);
        const batchCommand = batchValuesCommand(batchWriter, batchBase, [
          { row: 0, column: 0, value: 12 },
          { row: 1, column: 1, value: 'replayed' },
        ]);
        const batchCommit = await batchWriter.send(batchCommand);
        assert.equal(batchCommit.case, 'committed');
        batchWriter.socket.terminate();
        await kill();
        await start();
        const batchRecovered = await getDocument(base, cookie, batchBase.document_id);
        assert.equal(batchRecovered.version, '2');
        assert.equal(batchRecovered.sheets[0].cells.length, 2);
        assert.equal(
          batchRecovered.state_hash,
          Buffer.from(batchCommit.value.confirmedStateHash).toString('hex'),
        );
        const batchRetry = await connect(base, cookie);
        assert.equal((await batchRetry.send(batchCommand)).case, 'receipt');
        batchRetry.socket.terminate();

        const { snapshot: undoBase } = await createDocument(base, cookie);
        const undoWriter = await connect(base, cookie);
        const original = setCommand(undoWriter, undoBase, 'before undo');
        assert.equal((await undoWriter.send(original)).case, 'committed');
        undoWriter.socket.terminate();
        await kill();
        await start();
        const beforeUndo = await getDocument(base, cookie, undoBase.document_id);
        assert.equal(beforeUndo.version, '2');
        const undoClient = await connect(base, cookie);
        const undo = undoCellCommand(undoClient, beforeUndo, original.operationId, {
          sequence: 2,
        });
        const undoCommit = await undoClient.send(undo);
        assert.equal(undoCommit.case, 'committed');
        undoClient.socket.terminate();
        await kill();
        await start();
        const undoRecovered = await getDocument(base, cookie, undoBase.document_id);
        assert.equal(undoRecovered.version, '3');
        assert.equal(undoRecovered.sheets[0].cells.length, 0);
        assert.equal(
          undoRecovered.state_hash,
          Buffer.from(undoCommit.value.confirmedStateHash).toString('hex'),
        );
        const undoRetry = await connect(base, cookie);
        assert.equal((await undoRetry.send(undo)).case, 'receipt');
        assert.equal((await getDocument(base, cookie, undoBase.document_id)).version, '3');
        undoRetry.socket.terminate();

        const { snapshot: formulaUndoBase } = await createDocument(base, cookie);
        const formulaUndoWriter = await connect(base, cookie);
        assert.equal(
          (await formulaUndoWriter.send(formulaCommand(formulaUndoWriter, formulaUndoBase, '=2+3')))
            .case,
          'committed',
        );
        const overwritten = setCommand(
          formulaUndoWriter,
          { ...formulaUndoBase, version: '2' },
          42,
          {
            sequence: 2,
          },
        );
        assert.equal((await formulaUndoWriter.send(overwritten)).case, 'committed');
        const formulaUndo = undoCellCommand(
          formulaUndoWriter,
          { ...formulaUndoBase, version: '3' },
          overwritten.operationId,
          { sequence: 3 },
        );
        const formulaUndoCommit = await formulaUndoWriter.send(formulaUndo);
        assert.equal(formulaUndoCommit.case, 'committed');
        formulaUndoWriter.socket.terminate();
        await kill();
        await start();
        const formulaUndoRecovered = await getDocument(base, cookie, formulaUndoBase.document_id);
        assert.equal(formulaUndoRecovered.version, '4');
        assert.equal(formulaUndoRecovered.sheets[0].cells[0].value.value.source, '=2+3');
        assert.equal(
          formulaUndoRecovered.state_hash,
          Buffer.from(formulaUndoCommit.value.confirmedStateHash).toString('hex'),
        );
        const afterFormulaUndo = await calculation(base, cookie, formulaUndoBase.document_id, 4);
        assert.deepEqual(afterFormulaUndo.values[0].value, { type: 'number', value: 5 });
        const formulaUndoRetry = await connect(base, cookie);
        assert.equal((await formulaUndoRetry.send(formulaUndo)).case, 'receipt');
        formulaUndoRetry.socket.terminate();

        const authoredWriter = await connect(base, cookie);
        const authoredFormula = formulaCommand(authoredWriter, formulaUndoRecovered, '=8+9', {
          sequence: 4,
        });
        assert.equal((await authoredWriter.send(authoredFormula)).case, 'committed');
        authoredWriter.socket.terminate();
        await kill();
        await start();
        const beforeAuthoredUndo = await getDocument(base, cookie, formulaUndoBase.document_id);
        assert.equal(beforeAuthoredUndo.version, '5');
        assert.equal(beforeAuthoredUndo.sheets[0].cells[0].value.value.source, '=8+9');
        const authoredUndoWriter = await connect(base, cookie);
        const authoredUndo = undoCellCommand(
          authoredUndoWriter,
          beforeAuthoredUndo,
          authoredFormula.operationId,
          { sequence: 5 },
        );
        const authoredUndoAck = await authoredUndoWriter.send(authoredUndo);
        assert.equal(authoredUndoAck.case, 'committed');
        const authoredUndoCalculation = await calculation(
          base,
          cookie,
          formulaUndoBase.document_id,
          6,
        );
        authoredUndoWriter.socket.terminate();
        await kill();
        await start();
        const restoredAuthoredUndo = await getDocument(base, cookie, formulaUndoBase.document_id);
        assert.equal(restoredAuthoredUndo.version, '6');
        assert.equal(restoredAuthoredUndo.sheets[0].cells[0].value.value.source, '=2+3');
        assert.equal(
          restoredAuthoredUndo.state_hash,
          Buffer.from(authoredUndoAck.value.confirmedStateHash).toString('hex'),
        );
        const restoredAuthoredCalculation = await calculation(
          base,
          cookie,
          formulaUndoBase.document_id,
          6,
        );
        assert.deepEqual(restoredAuthoredCalculation.values[0].value, { type: 'number', value: 5 });
        assert.equal(
          restoredAuthoredCalculation.calculated_state_hash,
          authoredUndoCalculation.calculated_state_hash,
        );
        const authoredUndoRetry = await connect(base, cookie);
        assert.equal((await authoredUndoRetry.send(authoredUndo)).case, 'receipt');
        authoredUndoRetry.socket.terminate();

        const redoWriter = await connect(base, cookie);
        const redo = undoCellCommand(redoWriter, restoredAuthoredUndo, authoredUndo.operationId, {
          sequence: 6,
        });
        const redoAck = await redoWriter.send(redo);
        assert.equal(redoAck.case, 'committed');
        const redoCalculation = await calculation(base, cookie, formulaUndoBase.document_id, 7);
        redoWriter.socket.terminate();
        await kill();
        await start();
        const redoRecovered = await getDocument(base, cookie, formulaUndoBase.document_id);
        assert.equal(redoRecovered.version, '7');
        assert.equal(redoRecovered.sheets[0].cells[0].value.value.source, '=8+9');
        assert.equal(
          redoRecovered.state_hash,
          Buffer.from(redoAck.value.confirmedStateHash).toString('hex'),
        );
        const recoveredRedoCalculation = await calculation(
          base,
          cookie,
          formulaUndoBase.document_id,
          7,
        );
        assert.deepEqual(recoveredRedoCalculation.values[0].value, { type: 'number', value: 17 });
        assert.equal(
          recoveredRedoCalculation.calculated_state_hash,
          redoCalculation.calculated_state_hash,
        );
        const redoRetry = await connect(base, cookie);
        assert.equal((await redoRetry.send(redo)).case, 'receipt');
        assert.equal((await getDocument(base, cookie, formulaUndoBase.document_id)).version, '7');
        redoRetry.socket.terminate();

        const { snapshot: rangeBase } = await createDocument(base, cookie);
        const rangeWriter = await connect(base, cookie);
        assert.equal(
          (await rangeWriter.send(setCommand(rangeWriter, rangeBase, 3))).case,
          'committed',
        );
        assert.equal(
          (
            await rangeWriter.send(
              formulaCommand(rangeWriter, { ...rangeBase, version: '2' }, '=A1*2', {
                column: 1,
                sequence: 2,
              }),
            )
          ).case,
          'committed',
        );
        const rangeCopy = copyRangeCommand(
          rangeWriter,
          { ...rangeBase, version: '3' },
          { destinationRow: 1, destinationColumn: 2, rows: 1, columns: 2 },
          { sequence: 3 },
        );
        const rangeCommit = await rangeWriter.send(rangeCopy);
        assert.equal(rangeCommit.case, 'committed');
        rangeWriter.socket.terminate();
        await kill();
        await start();
        const rangeRecovered = await getDocument(base, cookie, rangeBase.document_id);
        assert.equal(rangeRecovered.version, '4');
        assert.equal(
          rangeRecovered.state_hash,
          Buffer.from(rangeCommit.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          rangeRecovered.sheets[0].cells.find(
            (cell) =>
              cell.row_id === rangeBase.sheets[0].rows[1] &&
              cell.column_id === rangeBase.sheets[0].columns[3],
          )?.value.value.current_text,
          '=(C2*2)',
        );
        const rangeResult = await calculation(base, cookie, rangeBase.document_id, 4);
        assert.deepEqual(
          rangeResult.values.find(
            (cell) =>
              cell.row_id === rangeBase.sheets[0].rows[1] &&
              cell.column_id === rangeBase.sheets[0].columns[3],
          )?.value,
          { type: 'number', value: 6 },
        );
        const rangeRetry = await connect(base, cookie);
        assert.equal((await rangeRetry.send(rangeCopy)).case, 'receipt');
        const rangeUndo = undoCellCommand(rangeRetry, rangeRecovered, rangeCopy.operationId, {
          sequence: 4,
        });
        const rangeUndoCommit = await rangeRetry.send(rangeUndo);
        assert.equal(rangeUndoCommit.case, 'committed');
        rangeRetry.socket.terminate();
        await kill();
        await start();
        const undoneRange = await getDocument(base, cookie, rangeBase.document_id);
        assert.equal(undoneRange.version, '5');
        assert.equal(
          undoneRange.state_hash,
          Buffer.from(rangeUndoCommit.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(undoneRange.sheets[0].cells.length, 2);
        const rangeRedoWriter = await connect(base, cookie);
        const rangeRedo = undoCellCommand(rangeRedoWriter, undoneRange, rangeUndo.operationId, {
          sequence: 5,
        });
        const rangeRedoCommit = await rangeRedoWriter.send(rangeRedo);
        assert.equal(rangeRedoCommit.case, 'committed');
        const rangeRedoCalculation = await calculation(base, cookie, rangeBase.document_id, 6);
        rangeRedoWriter.socket.terminate();
        await kill();
        await start();
        const redoneRange = await getDocument(base, cookie, rangeBase.document_id);
        assert.equal(redoneRange.version, '6');
        assert.equal(
          redoneRange.state_hash,
          Buffer.from(rangeRedoCommit.value.confirmedStateHash).toString('hex'),
        );
        assert.equal(
          (await calculation(base, cookie, rangeBase.document_id, 6)).calculated_state_hash,
          rangeRedoCalculation.calculated_state_hash,
        );
        const rangeFinalRetry = await connect(base, cookie);
        assert.equal((await rangeFinalRetry.send(rangeUndo)).case, 'receipt');
        assert.equal((await rangeFinalRetry.send(rangeRedo)).case, 'receipt');
        assert.equal((await getDocument(base, cookie, rangeBase.document_id)).version, '6');
        rangeFinalRetry.socket.terminate();

        const { snapshot: delayedBase } = await createDocument(base, cookie);
        const delayedWriter = await connect(base, cookie);
        assert.equal(
          (await delayedWriter.send(setCommand(delayedWriter, delayedBase, 11))).case,
          'committed',
        );
        const authored = { ...delayedBase, version: '2' };
        const delayedFormula = formulaCommand(delayedWriter, authored, '=A1+1', {
          column: 1,
          sequence: 3,
        });
        assert.equal(
          (
            await delayedWriter.send(
              insertRowsCommand(delayedWriter, authored, authored.sheets[0].rows[0], [uuid7()], {
                sequence: 2,
              }),
            )
          ).case,
          'committed',
        );
        delayedWriter.socket.terminate();
        await kill();
        await start();
        // Force head recovery first: the actor cache has no pre-restart axes.
        assert.equal((await getDocument(base, cookie, delayedBase.document_id)).version, '3');
        const delayedRetry = await connect(base, cookie);
        try {
          const bound = await delayedRetry.send(delayedFormula);
          assert.equal(bound.case, 'committed');
          assert.equal(bound.value.committedVersion, 4n);
          assert.equal(bound.value.transformed, true);
          assert.equal(bound.value.canonicalOperation, undefined);
          const result = await calculation(base, cookie, delayedBase.document_id, 4);
          assert.deepEqual(result.values[0].value, { type: 'number', value: 12 });
          const projection = await getDocument(base, cookie, delayedBase.document_id);
          const stored = projection.sheets[0].cells.find((cell) => cell.value.type === 'formula');
          assert.equal(stored.value.value.source, '=A1+1');
          assert.equal(stored.value.value.current_text, '=(A2+1)');
          assert.equal((await delayedRetry.send(delayedFormula)).case, 'receipt');
          assert.equal(
            (
              await delayedRetry.send(
                deleteRowsCommand(delayedRetry, projection, [authored.sheets[0].rows[0]], {
                  sequence: 4,
                }),
              )
            ).case,
            'committed',
          );
          delayedRetry.socket.terminate();
          await kill();
          await start();
          assert.equal((await getDocument(base, cookie, delayedBase.document_id)).version, '5');
          const afterDeletion = await connect(base, cookie);
          try {
            // The formula origin must survive even though its old A1 target was deleted.
            const survivingOrigin = {
              ...authored,
              sheets: [
                {
                  ...authored.sheets[0],
                  rows: [authored.sheets[0].rows[1], ...authored.sheets[0].rows.slice(2)],
                },
              ],
            };
            const command = formulaCommand(afterDeletion, survivingOrigin, '=A1', {
              column: 2,
              sequence: 5,
            });
            assert.equal((await afterDeletion.send(command)).case, 'committed');
            const errors = await calculation(base, cookie, delayedBase.document_id, 6);
            assert.deepEqual(
              errors.values.map((cell) => cell.value),
              [{ type: 'error', value: '#REF!' }],
            );
            assert.equal(errors.values[0].row_id, authored.sheets[0].rows[1]);
            assert.equal(errors.values[0].column_id, authored.sheets[0].columns[2]);
          } finally {
            afterDeletion.socket.terminate();
          }
        } finally {
          delayedRetry.socket.terminate();
        }
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
