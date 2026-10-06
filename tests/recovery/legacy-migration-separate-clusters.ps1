$ErrorActionPreference = 'Stop'
. ./scripts/env.ps1

$environmentNames = @(
    'CARGO_TARGET_DIR',
    'DATABASE_URL',
    'PGPASSWORD',
    'PXEL_KEYRING_FILE',
    'PXEL_LEGACY_MIGRATION_TEST_TARGET_URL',
    'PXEL_LEGACY_MIGRATION_TEST_DATA_ROOT',
    'PXEL_LEGACY_SOURCE_BLOB_ROOT',
    'PXEL_LEGACY_SOURCE_DATABASE_URL',
    'PXEL_LEGACY_SOURCE_DATABASE_URL_FILE',
    'PXEL_BLOB_ROOT',
    'PXEL_DATABASE_URL_FILE',
    'PXEL_MIGRATION_ROLLBACK_ARCHIVE',
    'PXEL_MIGRATION_VERIFY_ROOT',
    'PXEL_RESTORE_DATABASE_URL',
    'PXEL_RESTORE_DATABASE_URL_FILE',
    'PXEL_TEST_SERVER_BIN'
)
$priorEnvironment = @{}
foreach ($name in $environmentNames) {
    $priorEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$projectRoot = (Resolve-Path .).Path
$localRoot = (Resolve-Path .local/crypto-verification).Path
$env:CARGO_TARGET_DIR = Join-Path $projectRoot '.local\target-merge'
cargo build -p pxel-server --all-features *> $null
if ($LASTEXITCODE -ne 0) {
    throw 'The legacy migration server binary could not be built.'
}
$fixtureRoot = Join-Path $localRoot ("legacy-cli-" + [Guid]::NewGuid().ToString('N'))
if (!$fixtureRoot.StartsWith($localRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Fixture path escaped the private verification root.'
}

function New-PrivateDirectory([string] $path) {
    [IO.Directory]::CreateDirectory($path) | Out-Null
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner($sid)
    foreach ($identityValue in @($sid.Value, 'S-1-5-18', 'S-1-5-32-544')) {
        $identity = [Security.Principal.SecurityIdentifier]::new($identityValue)
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [Security.AccessControl.FileSystemRights]::FullControl,
            [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',
            [Security.AccessControl.PropagationFlags]::None,
            [Security.AccessControl.AccessControlType]::Allow
        )
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $path -AclObject $acl
}

function Get-FreePort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = ([Net.IPEndPoint]$listener.LocalEndpoint).Port
    $listener.Stop()
    return $port
}

function Invoke-Postgres([string] $name, [string[]] $arguments) {
    $binary = Join-Path $env:PXEL_PG_BIN ($name + '.exe')
    & $binary @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Private PostgreSQL utility failed: $name."
    }
}

$sourceStarted = $false
$targetStarted = $false
$fixtureComplete = $false
$sourcePort = Get-FreePort
$targetPort = Get-FreePort
$recoveryPort = Get-FreePort
$databaseUser = 'pxel_migration_fixture'
$databaseName = 'pxel'
$password = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(24)).ToLowerInvariant()

try {
    New-PrivateDirectory $fixtureRoot
    $sourceData = Join-Path $fixtureRoot 'source\pgdata'
    $targetData = Join-Path $fixtureRoot 'target\pgdata'
    $sourceBlobs = Join-Path $fixtureRoot 'source\blobs'
    $targetBlobs = Join-Path $fixtureRoot 'target\blobs'
    $legacyMigrations = Join-Path $fixtureRoot 'legacy-migrations'
    foreach ($path in @(
        (Join-Path $fixtureRoot 'source'),
        (Join-Path $fixtureRoot 'target'),
        (Join-Path $fixtureRoot 'source\pgdata'),
        (Join-Path $fixtureRoot 'target\pgdata'),
        $sourceBlobs,
        $legacyMigrations
    )) {
        New-PrivateDirectory $path
    }
    $passwordFile = Join-Path $fixtureRoot 'postgres-password'
    [IO.File]::WriteAllText($passwordFile, $password, [Text.UTF8Encoding]::new($false))
    $keyHex = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32)).ToLowerInvariant()
    $keyringFile = Join-Path $fixtureRoot 'keyring.json'
    [IO.File]::WriteAllText(
        $keyringFile,
        (@{ schema_version = 1; active_kek_id = 'migration-fixture'; keys = @(@{ id = 'migration-fixture'; key_hex = $keyHex }) } | ConvertTo-Json -Compress),
        [Text.UTF8Encoding]::new($false)
    )
    $initdb = Join-Path $env:PXEL_PG_BIN 'initdb.exe'
    foreach ($data in @($sourceData, $targetData)) {
        & $initdb -D $data -U $databaseUser --auth-host=scram-sha-256 --auth-local=trust --pwfile=$passwordFile --no-instructions
        if ($LASTEXITCODE -ne 0) {
            throw 'Private PostgreSQL initialization failed.'
        }
    }

    Invoke-Postgres 'pg_ctl' @('start', '-D', $sourceData, '-w', '-t', '60', '-s', '-l', (Join-Path $fixtureRoot 'source-postgres.log'), '-o', "-p $sourcePort -h 127.0.0.1")
    $sourceStarted = $true
    Invoke-Postgres 'pg_ctl' @('start', '-D', $targetData, '-w', '-t', '60', '-s', '-l', (Join-Path $fixtureRoot 'target-postgres.log'), '-o', "-p $targetPort -h 127.0.0.1")
    $targetStarted = $true

    $env:PGPASSWORD = $password
    foreach ($port in @($sourcePort, $targetPort)) {
        Invoke-Postgres 'createdb' @('-h', '127.0.0.1', '-p', "$port", '-U', $databaseUser, $databaseName)
    }

    foreach ($migration in Get-ChildItem migrations -File -Filter '*.sql') {
        if ([Int64]$migration.Name.Substring(0, 12) -le 202610020005) {
            Copy-Item -LiteralPath $migration.FullName -Destination (Join-Path $legacyMigrations $migration.Name)
        }
    }
    $sourceUrl = "postgresql://${databaseUser}:${password}@127.0.0.1:${sourcePort}/${databaseName}?sslmode=disable"
    $recoveryUrl = "postgresql://${databaseUser}:${password}@127.0.0.1:${recoveryPort}/${databaseName}?sslmode=disable"

    $env:DATABASE_URL = $sourceUrl
    cargo sqlx migrate run --source $legacyMigrations *> $null
    if ($LASTEXITCODE -ne 0) {
        throw 'Legacy schema migrations failed.'
    }
    $fixtureUrls = @{}
    foreach ($fixtureDatabaseName in @('pxel_legacy_api_fixture', 'pxel_legacy_cli_fixture')) {
        Invoke-Postgres 'createdb' @('-h', '127.0.0.1', '-p', "$targetPort", '-U', $databaseUser, $fixtureDatabaseName)
        $fixtureUrl = "postgresql://${databaseUser}:${password}@127.0.0.1:${targetPort}/${fixtureDatabaseName}?sslmode=disable"
        $env:DATABASE_URL = $fixtureUrl
        cargo sqlx migrate run *> $null
        if ($LASTEXITCODE -ne 0) {
            throw 'Content-bearing target fixture schema migrations failed.'
        }
        $fixtureUrls[$fixtureDatabaseName] = $fixtureUrl
    }
    $env:CARGO_TARGET_DIR = Join-Path $projectRoot '.local\target-merge'
    $env:DATABASE_URL = $sourceUrl
    $env:PXEL_KEYRING_FILE = $keyringFile
    $env:PXEL_LEGACY_MIGRATION_TEST_DATA_ROOT = $fixtureRoot
    $env:PXEL_LEGACY_SOURCE_DATABASE_URL = $sourceUrl
    $env:PXEL_RESTORE_DATABASE_URL = $recoveryUrl
    foreach ($variable in @(
        'PXEL_LEGACY_SOURCE_DATABASE_URL_FILE',
        'PXEL_DATABASE_URL_FILE',
        'PXEL_RESTORE_DATABASE_URL_FILE'
    )) {
        $secretFile = Join-Path $fixtureRoot ($variable + '.secret')
        [IO.File]::WriteAllText($secretFile, 'fixture-secret-file', [Text.UTF8Encoding]::new($false))
        Set-Item -Path ("Env:" + $variable) -Value $secretFile
    }
    Remove-Item Env:PXEL_TEST_SERVER_BIN -ErrorAction SilentlyContinue
    $env:PXEL_LEGACY_MIGRATION_TEST_TARGET_URL = $fixtureUrls['pxel_legacy_api_fixture']
    cargo test -p spreadsheet-storage --features legacy-migration --lib legacy_history_metadata_snapshots_and_export_audits_are_verified_and_encrypted -- --test-threads=1
    if ($LASTEXITCODE -ne 0) {
        throw 'Content-bearing storage API migration across separate physical clusters failed.'
    }

    $env:PXEL_TEST_SERVER_BIN = Join-Path $projectRoot '.local\target-merge\debug\pxel-server.exe'
    $env:PXEL_LEGACY_MIGRATION_TEST_TARGET_URL = $fixtureUrls['pxel_legacy_cli_fixture']
    cargo test -p spreadsheet-storage --features legacy-migration --lib legacy_history_metadata_snapshots_and_export_audits_are_verified_and_encrypted -- --test-threads=1
    if ($LASTEXITCODE -ne 0) {
        throw 'Content-bearing migration CLI across separate physical clusters failed.'
    }
    $migrationOutput = Join-Path $fixtureRoot 'content-migration-output'
    if (!(Test-Path -LiteralPath (Join-Path $migrationOutput 'rollback.pxel-backup'))) {
        throw 'Encrypted rollback archive was not published for the content-bearing migration.'
    }
    if (!(Test-Path -LiteralPath (Join-Path $migrationOutput 'rollback-verified\VERIFIED_ONLY'))) {
        throw 'Encrypted rollback archive recovery was not verified for the content-bearing migration.'
    }
    if (!(Test-Path -LiteralPath (Join-Path $migrationOutput 'target-blobs') -PathType Container)) {
        throw 'Content-bearing target blob store was not published.'
    }
    Remove-Item Env:PXEL_LEGACY_MIGRATION_TEST_TARGET_URL
    $fixtureComplete = $true
    'Content-bearing separate-cluster legacy migration CLI and encrypted rollback recovery passed.'
}
finally {
    try {
        if ($targetStarted) {
            & (Join-Path $env:PXEL_PG_BIN 'pg_ctl.exe') stop -D $targetData -m fast -w -t 30 -s
        }
        if ($sourceStarted) {
            & (Join-Path $env:PXEL_PG_BIN 'pg_ctl.exe') stop -D $sourceData -m fast -w -t 30 -s
        }
        if ($fixtureComplete) {
            $resolved = (Resolve-Path -LiteralPath $fixtureRoot).Path
            $requiredPrefix = $localRoot + [IO.Path]::DirectorySeparatorChar
            if (!$resolved.StartsWith($requiredPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw 'Refusing to remove a fixture outside the private verification root.'
            }
            foreach ($pidFile in Get-ChildItem -LiteralPath $resolved -Filter postmaster.pid -File -Recurse -ErrorAction SilentlyContinue) {
                $serverPid = [int](Get-Content -LiteralPath $pidFile.FullName -TotalCount 1)
                if (Get-Process -Id $serverPid -ErrorAction SilentlyContinue) {
                    throw "Refusing to remove a fixture with live PostgreSQL process $serverPid."
                }
            }
            [IO.Directory]::Delete($resolved, $true)
        }
    } finally {
        foreach ($name in $environmentNames) {
            $value = $priorEnvironment[$name]
            if ($null -eq $value) {
                Remove-Item ("Env:" + $name) -ErrorAction SilentlyContinue
            } else {
                Set-Item -Path ("Env:" + $name) -Value $value
            }
        }
    }
}
