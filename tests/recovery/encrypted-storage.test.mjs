import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import {
  appendFileSync,
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  truncateSync,
  writeFileSync,
  writeSync,
} from 'node:fs';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test } from 'node:test';
import {
  calculation,
  connect,
  createDocument,
  formulaCommand,
  getDocument,
  login,
  releaseSessions,
  setCommand,
  undoCellCommand,
} from '../helpers/client.mjs';
import { exe, freePort, killServer, startServer } from './private-postgres.mjs';

const envelopePrefix = Buffer.from('5058454c454e430000010001', 'hex');
const localRoot = resolve('.local');

// Capture privately; a failed psql command must never print its SQL or result.
function command(binary, args, env, label, { capture = true, timeout = 30000 } = {}) {
  const result = spawnSync(binary, args, {
    env,
    encoding: 'utf8',
    windowsHide: true,
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'ignore',
    timeout,
    maxBuffer: 2 * 1024 * 1024,
  });
  if (result.error || result.status !== 0)
    throw new Error(`${label} failed; inspect private fixture`);
  return capture ? result.stdout.trim() : '';
}

function privateDirectory(path, create = true) {
  if (create) mkdirSync(path, { mode: 0o700 });
  if (process.platform !== 'win32') {
    chmodSync(path, 0o700);
    return;
  }
  const powershell = join(
    process.env.SystemRoot ?? 'C:\\Windows',
    'System32',
    'WindowsPowerShell',
    'v1.0',
    'powershell.exe',
  );
  assert.ok(isAbsolute(powershell), 'fixture ACL helper must use an absolute executable');
  const script = `$ErrorActionPreference='Stop'; try {
    Import-Module ($PSHOME + '\\Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1');
    $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
    $acl=[System.Security.AccessControl.DirectorySecurity]::new();
    $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
    $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);
    $acl.AddAccessRule($rule);
    Microsoft.PowerShell.Security\\Set-Acl -LiteralPath $env:PXEL_FIXTURE_ACL_PATH -AclObject $acl;
    $actual=Microsoft.PowerShell.Security\\Get-Acl -LiteralPath $env:PXEL_FIXTURE_ACL_PATH;
    if(-not $actual.AreAccessRulesProtected -or $actual.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 1 };
    foreach($entry in $actual.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
      if($entry.IdentityReference.Value -ne $sid.Value -or $entry.AccessControlType -ne 'Allow') { exit 1 }
    }; exit 0
  } catch { exit 1 }`;
  command(
    powershell,
    ['-NoProfile', '-NonInteractive', '-Command', script],
    { ...process.env, PSModulePath: '', PXEL_FIXTURE_ACL_PATH: path },
    'private fixture ACL',
  );
}

function isolatedEnvironment() {
  const env = { ...process.env };
  for (const name of Object.keys(env)) {
    if (/^(PXEL_|PG)/i.test(name) || name.toUpperCase() === 'DATABASE_URL') delete env[name];
  }
  return env;
}

function assertInside(root, path) {
  const part = relative(root, resolve(path));
  assert.ok(
    part !== '' && part !== '..' && !part.startsWith('..' + sep) && !isAbsolute(part),
    'fixture path escaped its owned root',
  );
}

function filesUnder(root) {
  const output = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    assertInside(root, path);
    assert.ok(!lstatSync(path).isSymbolicLink(), 'fixture contains an unexpected link');
    if (entry.isDirectory()) output.push(...filesUnder(path));
    else if (entry.isFile()) output.push(path);
    else throw new Error('fixture contains an unexpected filesystem entry');
  }
  return output;
}

function assertNoSecrets(path, needles) {
  const longest = Math.max(...needles.map((needle) => needle.length));
  const fd = openSync(path, 'r');
  const chunk = Buffer.alloc(64 * 1024);
  let tail = Buffer.alloc(0);
  try {
    for (;;) {
      const count = readSync(fd, chunk, 0, chunk.length, null);
      if (count === 0) break;
      const bytes = Buffer.concat([tail, chunk.subarray(0, count)]);
      assert.ok(
        !needles.some((needle) => bytes.includes(needle)),
        'plaintext document or credentials reached a persisted artifact or log',
      );
      tail = Buffer.from(bytes.subarray(Math.max(0, bytes.length - longest + 1)));
    }
  } finally {
    closeSync(fd);
  }
}

function envelope(hex) {
  const bytes = Buffer.from(hex, 'hex');
  assert.ok(
    bytes.length >= 56 && bytes.subarray(0, 12).equals(envelopePrefix),
    'persisted record is not an authenticated encrypted envelope',
  );
  return { generation: bytes.readUInt32BE(12), nonce: bytes.subarray(16, 40).toString('hex') };
}

test(
  'encrypted CLI backup restores snapshot plus Undo tail and fresh write lineage survives rotation',
  { timeout: 600000 },
  async (t) => {
    assert.ok(
      process.env.PXEL_PG_BIN,
      'PXEL_PG_BIN must point to private PostgreSQL 18.6 binaries',
    );
    assert.ok(
      process.env.PXEL_TEST_SERVER_BIN,
      'PXEL_TEST_SERVER_BIN must point to the built server',
    );
    const binary = resolve(process.env.PXEL_TEST_SERVER_BIN);
    mkdirSync(localRoot, { recursive: true });
    const root = mkdtempSync(join(localRoot, 'encrypted-recovery-'));
    privateDirectory(root, false);
    const ownedRoot = realpathSync(root);
    assertInside(realpathSync(localRoot), ownedRoot);
    const secrets = join(root, 'secrets');
    const source = join(root, 'source');
    const archives = join(root, 'archives');
    const recoveryParent = join(root, 'recovery');
    const logs = join(root, 'logs');
    for (const path of [secrets, source, archives, recoveryParent, logs]) privateDirectory(path);
    const sourceData = join(source, 'pgdata');
    const sourceBlobs = join(source, 'blobs');
    privateDirectory(sourceBlobs);
    const restoredRoot = join(recoveryParent, 'restored');
    const restoredData = join(restoredRoot, 'pgdata');
    const restoredBlobs = join(restoredRoot, 'blobs');
    const archive = join(archives, 'workbook.pxel-backup');
    const keyringFile = join(secrets, 'keyring.json');
    const wrongKeyringFile = join(secrets, 'wrong-keyring.json');
    const passwordFile = join(secrets, 'postgres-password');
    const keys = ['source', 'freshRotate', 'freshRestore'].map((id) => ({
      id,
      key_hex: randomBytes(32).toString('hex'),
    }));
    const wrongKeys = keys.map(({ id }) => ({ id, key_hex: randomBytes(32).toString('hex') }));
    for (const [path, value] of [
      [keyringFile, keys],
      [wrongKeyringFile, wrongKeys],
    ]) {
      writeFileSync(
        path,
        JSON.stringify({ schema_version: 1, active_kek_id: 'source', keys: value }),
        { mode: 0o600, flag: 'wx' },
      );
    }
    const password = randomBytes(24).toString('hex');
    writeFileSync(passwordFile, password, { mode: 0o600, flag: 'wx' });
    const ports = new Set();
    async function distinctPort() {
      let port;
      do {
        port = await freePort();
      } while (ports.has(port));
      ports.add(port);
      return port;
    }
    const sourcePort = await distinctPort();
    const restorePort = await distinctPort();
    const appPort = await distinctPort();
    const base = `http://127.0.0.1:${appPort}`;
    const common = {
      ...isolatedEnvironment(),
      PGPASSWORD: password,
      PGCONNECT_TIMEOUT: '5',
      PXEL_PG_BIN: resolve(process.env.PXEL_PG_BIN),
      PXEL_KEYRING_FILE: keyringFile,
      PXEL_DEV_AUTH: '1',
      PXEL_BIND: `127.0.0.1:${appPort}`,
      PXEL_ALLOWED_ORIGINS: 'http://127.0.0.1:5173',
      PXEL_SNAPSHOT_INTERVAL: '2',
    };
    const databaseUrl = (port) => `postgresql://pxel:${password}@127.0.0.1:${port}/pxel`;
    const sourceUrlFile = join(secrets, 'source-database-url');
    const restoredUrlFile = join(secrets, 'restored-database-url');
    writeFileSync(sourceUrlFile, databaseUrl(sourcePort), { mode: 0o600, flag: 'wx' });
    writeFileSync(restoredUrlFile, databaseUrl(restorePort), { mode: 0o600, flag: 'wx' });
    const sourceEnv = {
      ...common,
      PXEL_DATABASE_URL_FILE: sourceUrlFile,
      PXEL_BLOB_ROOT: sourceBlobs,
    };
    const restoredEnv = {
      ...common,
      PXEL_DATABASE_URL_FILE: restoredUrlFile,
      PXEL_BLOB_ROOT: restoredBlobs,
    };
    const marker = 'private-original-' + randomBytes(24).toString('hex');
    const overwritten = 'private-undo-preimage-' + randomBytes(24).toString('hex');
    const formulaMarker = 'private-formula-' + randomBytes(24).toString('hex');
    const formula = `=IF(B1=7,"${formulaMarker}","other")`;
    const afterRestore = 'private-restored-write-' + randomBytes(24).toString('hex');
    const afterRotate = 'private-rotated-write-' + randomBytes(24).toString('hex');
    const needles = [
      marker,
      overwritten,
      formulaMarker,
      formula,
      afterRestore,
      afterRotate,
      password,
      ...keys.map((key) => key.key_hex),
      ...wrongKeys.map((key) => key.key_hex),
    ].map((value) => Buffer.from(value));
    needles.push(
      ...keys.map((key) => Buffer.from(key.key_hex, 'hex')),
      ...wrongKeys.map((key) => Buffer.from(key.key_hex, 'hex')),
    );
    let server;
    let socket;
    let passed = false;
    let cliNumber = 0;
    const postgresPids = new Map();

    function psql(port, sql, env) {
      return command(
        exe('psql'),
        [
          '-X',
          '-v',
          'ON_ERROR_STOP=1',
          '-h',
          '127.0.0.1',
          '-p',
          String(port),
          '-U',
          'pxel',
          '-d',
          'pxel',
          '-tAc',
          sql,
        ],
        env,
        'private psql query',
      );
    }
    function jsonSql(port, query, env) {
      return JSON.parse(
        psql(
          port,
          `SELECT COALESCE(json_agg(row_to_json(item)),'[]'::json) FROM (${query}) item`,
          env,
        ),
      );
    }
    function persisted(port, id, env) {
      return jsonSql(
        port,
        `SELECT committed_version, encode(payload,'hex') payload,
        encode(command_fingerprint,'hex') fingerprint, encode(canonical_event_hash,'hex') event_hash
        FROM document_operations WHERE document_id='${id}' ORDER BY committed_version`,
        env,
      );
    }
    function keyRows(port, id, env) {
      return jsonSql(
        port,
        `SELECT dek_generation, active, kek_id, wrap_revision,
        encode(physical_key_id,'hex') physical, encode(wrapping_nonce,'hex') nonce,
        encode(wrapped_dek,'hex') wrapped FROM document_crypto_keys
        WHERE document_id='${id}' ORDER BY dek_generation`,
        env,
      );
    }
    function snapshotMetadata(port, id, env) {
      return jsonSql(
        port,
        `SELECT snapshot_id, snapshot_version, object_key, state,
        encode(state_hash,'hex') state_hash, encode(payload_checksum,'hex') checksum, byte_length
        FROM document_snapshots WHERE document_id='${id}' ORDER BY snapshot_version,snapshot_id`,
        env,
      );
    }
    function recordPostgresPid(data, port) {
      const lines = readFileSync(join(data, 'postmaster.pid'), 'utf8').split(/\r?\n/);
      const pid = Number(lines[0]);
      assert.ok(Number.isSafeInteger(pid) && pid > 0, 'private PostgreSQL did not publish its PID');
      assert.ok(Number(lines[3]) === port, 'private PostgreSQL started on the wrong port');
      postgresPids.set(data, pid);
    }
    function startPg(data, port, env, log, config) {
      const options = config
        ? `-c config_file="${config.replaceAll('\\', '/')}" -c port=${port}`
        : `-h 127.0.0.1 -p ${port} -c fsync=on -c full_page_writes=on -c synchronous_commit=on`;
      command(
        exe('pg_ctl'),
        ['start', '-D', data, '-l', join(logs, log), '-w', '-t', '60', '-o', options],
        env,
        'private PostgreSQL startup',
        { capture: false, timeout: 70000 },
      );
      recordPostgresPid(data, port);
    }
    function stopPg(data, env) {
      if (!existsSync(data)) return;
      assertInside(ownedRoot, data);
      const status = spawnSync(exe('pg_ctl'), ['status', '-D', data], {
        env,
        windowsHide: true,
        stdio: 'ignore',
        timeout: 10000,
      });
      if (status.error) throw new Error('private PostgreSQL status check failed');
      if (status.status === 3 || !existsSync(join(data, 'postmaster.pid'))) return;
      assert.ok(status.status === 0, 'private PostgreSQL status could not be established');
      const pid = Number(readFileSync(join(data, 'postmaster.pid'), 'utf8').split(/\r?\n/)[0]);
      if (postgresPids.has(data))
        assert.ok(postgresPids.get(data) === pid, 'private PostgreSQL PID changed');
      command(
        exe('pg_ctl'),
        ['stop', '-D', data, '-m', 'fast', '-w', '-t', '30'],
        env,
        'private PostgreSQL shutdown',
        { capture: false, timeout: 40000 },
      );
      postgresPids.delete(data);
    }
    async function stopApp() {
      if (socket) {
        socket.terminate();
        socket = undefined;
      }
      try {
        if (server && server.exitCode === null && server.signalCode === null)
          await releaseSessions(base);
      } finally {
        await killServer(server);
        server = undefined;
      }
    }
    function cli(args, env, success = true) {
      const result = spawnSync(binary, args, {
        env,
        encoding: 'utf8',
        windowsHide: true,
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout: 240000,
        maxBuffer: 2 * 1024 * 1024,
      });
      const stdout = result.stdout ?? '';
      const stderr = result.stderr ?? '';
      writeFileSync(join(logs, `cli-${++cliNumber}.log`), stdout + '\n' + stderr, {
        mode: 0o600,
        flag: 'wx',
      });
      assert.ok(
        !result.error && result.signal === null,
        'maintenance CLI did not finish within its bound',
      );
      assert.ok(
        success ? result.status === 0 : result.status !== 0,
        success
          ? 'maintenance CLI failed; inspect private CLI log'
          : 'invalid encrypted backup was accepted',
      );
      assert.ok(
        !needles.some((needle) => Buffer.from(stdout + stderr).includes(needle)),
        'maintenance CLI disclosed private content or credentials',
      );
    }
    async function authenticate() {
      const cookie = await login(base);
      needles.push(Buffer.from(cookie), Buffer.from(cookie.split('=')[1]));
      return cookie;
    }
    async function commit(client, cookie, book, intent) {
      const result = await client.send(intent);
      assert.equal(result.case, 'committed');
      const next = await getDocument(base, cookie, book.document_id);
      assert.ok(
        BigInt(next.version) === result.value.committedVersion,
        'HTTP head differs from durable ACK',
      );
      assert.ok(
        next.state_hash === Buffer.from(result.value.confirmedStateHash).toString('hex'),
        'HTTP authored hash differs from durable ACK',
      );
      return next;
    }
    function cell(book, column) {
      return book.sheets[0].cells.find(
        (entry) =>
          entry.row_id === book.sheets[0].rows[0] &&
          entry.column_id === book.sheets[0].columns[column],
      );
    }

    try {
      assert.equal(
        command(exe('postgres'), ['--version'], common, 'PostgreSQL version'),
        'postgres (PostgreSQL) 18.6',
      );
      command(
        exe('initdb'),
        [
          '-D',
          sourceData,
          '-U',
          'pxel',
          '--encoding=UTF8',
          '--locale=C',
          '--auth-host=scram-sha-256',
          '--auth-local=scram-sha-256',
          `--pwfile=${passwordFile}`,
        ],
        sourceEnv,
        'private initdb',
        { timeout: 60000 },
      );
      privateDirectory(sourceData, false);
      appendFileSync(
        join(sourceData, 'postgresql.conf'),
        '\nlog_statement=none\nlog_parameter_max_length=0\nlog_parameter_max_length_on_error=0\nlog_min_error_statement=panic\n',
      );
      startPg(sourceData, sourcePort, sourceEnv, 'source-postgres.log');
      command(
        exe('createdb'),
        ['-h', '127.0.0.1', '-p', String(sourcePort), '-U', 'pxel', 'pxel'],
        sourceEnv,
        'private database creation',
      );
      server = await startServer(sourceEnv, base, join(logs, 'source-server.log'));
      let cookie = await authenticate();
      let { snapshot: book } = await createDocument(base, cookie);
      let writer = await connect(base, cookie);
      socket = writer.socket;
      book = await commit(writer, cookie, book, setCommand(writer, book, marker));
      book = await commit(
        writer,
        cookie,
        book,
        setCommand(writer, book, 7, { column: 1, sequence: 2 }),
      );
      book = await commit(
        writer,
        cookie,
        book,
        formulaCommand(writer, book, formula, { column: 2, sequence: 3 }),
      );
      const overwriteIntent = setCommand(writer, book, overwritten, { sequence: 4 });
      book = await commit(writer, cookie, book, overwriteIntent);
      assert.equal(book.version, '5');
      let snapshotRows;
      for (let attempt = 0; attempt < 150; attempt++) {
        snapshotRows = jsonSql(
          sourcePort,
          `SELECT object_key, encode(state_hash,'hex') state_hash,
          byte_length FROM document_snapshots WHERE document_id='${book.document_id}'
          AND snapshot_version=5 AND state='ACTIVE'`,
          sourceEnv,
        );
        if (snapshotRows.length === 1) break;
        await delay(100);
      }
      assert.equal(snapshotRows.length, 1, 'encrypted snapshot v5 did not become ACTIVE');
      envelope(snapshotRows[0].state_hash);
      const snapshotPath = join(sourceBlobs, snapshotRows[0].object_key);
      const snapshotBytes = readFileSync(snapshotPath);
      assert.ok(
        snapshotBytes.subarray(48, 60).equals(envelopePrefix),
        'snapshot blob contains plaintext',
      );
      assert.equal(snapshotBytes.length, Number(snapshotRows[0].byte_length) + 48);
      book = await commit(
        writer,
        cookie,
        book,
        undoCellCommand(writer, book, overwriteIntent.operationId, { sequence: 5 }),
      );
      assert.equal(book.version, '6');
      assert.ok(
        cell(book, 0)?.value.value === marker,
        'Undo did not restore the original private value',
      );
      assert.ok(cell(book, 2)?.value.value.source === formula, 'authored formula was not retained');
      const calculated = await calculation(base, cookie, book.document_id, book.version);
      assert.ok(
        calculated.values.some((entry) => entry.value.value === formulaMarker),
        'authored formula did not calculate before backup',
      );
      const backedUpBook = book;
      const oldRecords = persisted(sourcePort, book.document_id, sourceEnv);
      assert.equal(oldRecords.length, 6);
      for (const row of oldRecords) {
        envelope(row.payload);
        envelope(row.fingerprint);
        envelope(row.event_hash);
      }
      const sourceKeys = keyRows(sourcePort, book.document_id, sourceEnv);
      assert.ok(
        sourceKeys.length === 1 && sourceKeys[0].active && sourceKeys[0].kek_id === 'source',
        'source did not use its explicitly configured bootstrap key',
      );
      await stopApp();
      cli(['backup', 'create', archive, '--maintenance-confirmed'], sourceEnv);
      assert.ok(
        readFileSync(archive).subarray(0, 8).equals(Buffer.from('PXELBKP\0')),
        'CLI did not produce the encrypted backup format',
      );
      cli(['backup', 'verify', archive], sourceEnv);
      const tampered = join(archives, 'tampered.pxel-backup');
      const truncated = join(archives, 'truncated.pxel-backup');
      copyFileSync(archive, tampered);
      copyFileSync(archive, truncated);
      const fd = openSync(tampered, 'r+');
      try {
        const offset = Math.floor(statSync(tampered).size / 2);
        const byte = Buffer.alloc(1);
        assert.equal(readSync(fd, byte, 0, 1, offset), 1);
        byte[0] ^= 1;
        writeSync(fd, byte, 0, 1, offset);
      } finally {
        closeSync(fd);
      }
      truncateSync(truncated, statSync(truncated).size - 1);
      cli(['backup', 'verify', tampered], sourceEnv, false);
      cli(['backup', 'verify', truncated], sourceEnv, false);
      cli(
        ['backup', 'verify', archive],
        { ...sourceEnv, PXEL_KEYRING_FILE: wrongKeyringFile },
        false,
      );
      assert.ok(!existsSync(restoredRoot), 'verification unexpectedly created a restore stage');
      cli(
        [
          'backup',
          'restore',
          archive,
          restoredRoot,
          '--to',
          'freshRestore',
          '--maintenance-confirmed',
        ],
        { ...sourceEnv, PXEL_RESTORE_DATABASE_URL_FILE: restoredUrlFile },
      );
      assert.ok(
        existsSync(join(restoredRoot, 'COMPLETE')),
        'restore did not complete write activation',
      );
      assert.ok(
        !existsSync(join(restoredData, 'postmaster.pid')),
        'CLI left the recovery PostgreSQL running',
      );
      startPg(
        restoredData,
        restorePort,
        restoredEnv,
        'restored-postgres.log',
        join(restoredRoot, 'restore-postgresql.conf'),
      );
      assert.equal(psql(restorePort, 'SELECT pg_is_in_recovery()', restoredEnv), 'f');
      assert.equal(
        psql(
          restorePort,
          'SELECT active_kek_id FROM crypto_key_state WHERE singleton',
          restoredEnv,
        ),
        'freshRestore',
      );
      assert.equal(
        psql(
          restorePort,
          "SELECT count(*) FROM crypto_restore_runs WHERE state='COMPLETE'",
          restoredEnv,
        ),
        '1',
      );
      const renewedKeys = keyRows(restorePort, book.document_id, restoredEnv);
      const activeKey = renewedKeys.find((key) => key.active);
      assert.ok(
        renewedKeys.length === 2 &&
          activeKey.dek_generation === 2 &&
          activeKey.kek_id === 'freshRestore' &&
          activeKey.physical !== sourceKeys[0].physical,
        'restore did not create an independent fresh write generation',
      );
      assert.ok(
        JSON.stringify(persisted(restorePort, book.document_id, restoredEnv)) ===
          JSON.stringify(oldRecords),
        'restore changed existing encrypted journal records',
      );
      assert.ok(
        readFileSync(join(restoredBlobs, snapshotRows[0].object_key)).equals(snapshotBytes),
        'restore changed the encrypted snapshot blob',
      );
      server = await startServer(restoredEnv, base, join(logs, 'restored-server.log'));
      cookie = await authenticate();
      book = await getDocument(base, cookie, backedUpBook.document_id);
      assert.ok(
        book.version === backedUpBook.version && book.state_hash === backedUpBook.state_hash,
        'restored current head or authored hash differs from the acknowledged backup point',
      );
      assert.ok(
        cell(book, 0)?.value.value === marker && cell(book, 2)?.value.value.source === formula,
        'snapshot plus Undo journal tail did not restore authored content',
      );
      const restoredCalculation = await calculation(base, cookie, book.document_id, book.version);
      assert.ok(
        restoredCalculation.values.some((entry) => entry.value.value === formulaMarker),
        'formula calculation did not recover',
      );
      const recoveryEvents = readFileSync(join(logs, 'restored-server.log'), 'utf8')
        .split(/\r?\n/)
        .filter(Boolean)
        .map((line) => JSON.parse(line))
        .filter((entry) => entry.fields?.event === 'document_recovery');
      assert.ok(
        recoveryEvents.some(
          (entry) => entry.fields.snapshot_version === 5 && entry.fields.replayed_events === 1,
        ),
        'restored server did not recover snapshot v5 plus the Undo v6 tail',
      );
      writer = await connect(base, cookie);
      socket = writer.socket;
      book = await commit(
        writer,
        cookie,
        book,
        setCommand(writer, book, afterRestore, { column: 3 }),
      );
      const newRecord = persisted(restorePort, book.document_id, restoredEnv).at(-1);
      const newEnvelope = envelope(newRecord.payload);
      assert.equal(newEnvelope.generation, 2, 'post-restore write used an old DEK generation');
      assert.ok(
        !oldRecords.some((row) => envelope(row.payload).nonce === newEnvelope.nonce),
        'post-restore write reused an old journal nonce',
      );
      assert.equal(
        psql(
          restorePort,
          `SELECT count(*) FROM crypto_nonce_reservations
        WHERE physical_key_id=decode('${activeKey.physical}','hex') AND nonce=decode('${newEnvelope.nonce}','hex')`,
          restoredEnv,
        ),
        '1',
      );
      await stopApp();
      const beforeRotation = persisted(restorePort, book.document_id, restoredEnv);
      const beforeSnapshots = snapshotMetadata(restorePort, book.document_id, restoredEnv);
      const beforeKeys = keyRows(restorePort, book.document_id, restoredEnv);
      const blobHashes = filesUnder(restoredBlobs).map((path) => [
        relative(restoredBlobs, path),
        createHash('sha256').update(readFileSync(path)).digest('hex'),
      ]);
      const rotationHead = book;
      cli(['keys', 'rotate', '--to', 'freshRotate'], restoredEnv);
      assert.ok(
        JSON.stringify(persisted(restorePort, book.document_id, restoredEnv)) ===
          JSON.stringify(beforeRotation),
        'KEK rotation changed journal ciphertext or committed versions',
      );
      assert.ok(
        JSON.stringify(snapshotMetadata(restorePort, book.document_id, restoredEnv)) ===
          JSON.stringify(beforeSnapshots),
        'KEK rotation changed encrypted snapshot metadata',
      );
      const afterKeys = keyRows(restorePort, book.document_id, restoredEnv);
      assert.ok(
        afterKeys.length === beforeKeys.length &&
          afterKeys.every((key, index) => {
            const previous = beforeKeys[index];
            return (
              key.kek_id === 'freshRotate' &&
              key.dek_generation === previous.dek_generation &&
              key.physical === previous.physical &&
              key.active === previous.active &&
              key.wrap_revision === previous.wrap_revision + 1 &&
              key.nonce !== previous.nonce &&
              key.wrapped !== previous.wrapped
            );
          }),
        'KEK rotation did not rewrap every retained generation without changing its identity',
      );
      for (const [name, hash] of blobHashes) {
        assert.ok(
          createHash('sha256')
            .update(readFileSync(join(restoredBlobs, name)))
            .digest('hex') === hash,
          'KEK rotation changed snapshot ciphertext',
        );
      }
      assert.equal(
        psql(
          restorePort,
          'SELECT active_kek_id FROM crypto_key_state WHERE singleton',
          restoredEnv,
        ),
        'freshRotate',
      );
      server = await startServer(restoredEnv, base, join(logs, 'rotated-server.log'));
      cookie = await authenticate();
      book = await getDocument(base, cookie, rotationHead.document_id);
      assert.ok(
        book.version === rotationHead.version && book.state_hash === rotationHead.state_hash,
        'restarting after rotation changed the authored head',
      );
      writer = await connect(base, cookie);
      socket = writer.socket;
      book = await commit(
        writer,
        cookie,
        book,
        setCommand(writer, book, afterRotate, { column: 4 }),
      );
      assert.equal(
        envelope(persisted(restorePort, book.document_id, restoredEnv).at(-1).payload).generation,
        2,
      );
      const { snapshot: freshBook } = await createDocument(base, cookie);
      const freshKeys = keyRows(restorePort, freshBook.document_id, restoredEnv);
      assert.ok(
        freshKeys.length === 1 && freshKeys[0].active && freshKeys[0].kek_id === 'freshRotate',
        'new document ignored the durable rotated KEK',
      );
      await stopApp();
      stopPg(restoredData, restoredEnv);
      stopPg(sourceData, sourceEnv);
      for (const path of [
        ...filesUnder(archives),
        ...filesUnder(logs),
        ...filesUnder(sourceBlobs),
        ...filesUnder(restoredBlobs),
        ...filesUnder(sourceData),
        ...filesUnder(restoredData),
      ])
        assertNoSecrets(path, needles);
      passed = true;
    } finally {
      try {
        await stopApp();
      } finally {
        try {
          stopPg(restoredData, restoredEnv);
        } finally {
          try {
            stopPg(sourceData, sourceEnv);
          } finally {
            if (passed && process.env.PXEL_TEST_KEEP_RECOVERY !== '1') {
              assert.ok(realpathSync(root) === ownedRoot, 'private fixture cleanup root changed');
              assertInside(realpathSync(localRoot), ownedRoot);
              // Native filesystem removal stays inside this exact, owned fixture.
              rmSync(ownedRoot, { recursive: true, force: false });
            } else t.diagnostic(`Private encrypted recovery fixture retained at ${root}`);
          }
        }
      }
    }
  },
);
