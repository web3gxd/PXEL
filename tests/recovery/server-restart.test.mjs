import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { setTimeout } from 'node:timers/promises';
import { test } from 'node:test';
import {
  api,
  calculation,
  releaseSessions,
  formulaCommand,
  connect,
  createDocument,
  deleteColumnsCommand,
  deleteRowsCommand,
  extendRowsCommand,
  insertColumnsCommand,
  insertRowsCommand,
  getDocument,
  login,
  setCommand,
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
