# Optional isolated Windows toolchain used for local verification.
$pxelRoot = Split-Path -Parent $PSScriptRoot
$pxelCargo = Join-Path $pxelRoot '.tools\cargo'
if (Test-Path -LiteralPath $pxelCargo) {
    $env:CARGO_HOME = $pxelCargo
    $env:RUSTUP_HOME = Join-Path $pxelRoot '.tools\rustup'
    $env:PATH = (Join-Path $pxelCargo 'bin') + [IO.Path]::PathSeparator + $env:PATH
}
$pxelCompiler = Join-Path $pxelRoot '.tools\gcc\mingw64\bin'
if (Test-Path -LiteralPath $pxelCompiler) {
    $env:PATH = $pxelCompiler + [IO.Path]::PathSeparator + $env:PATH
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $pxelCompiler 'gcc.exe'
    $env:CC_x86_64_pc_windows_gnu = $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER
    $env:AR_x86_64_pc_windows_gnu = Join-Path $pxelCompiler 'ar.exe'
}
$pxelNodeTools = Join-Path $pxelRoot '.tools\node-tools\node_modules\.bin'
if (Test-Path -LiteralPath $pxelNodeTools) {
    $env:PATH = $pxelNodeTools + [IO.Path]::PathSeparator + $env:PATH
}
$pxelLocalBin = Join-Path $pxelRoot '.tools\bin'
if (Test-Path -LiteralPath $pxelLocalBin) {
    $env:PATH = $pxelLocalBin + [IO.Path]::PathSeparator + $env:PATH
}
$pxelCliBin = Join-Path $pxelRoot '.tools\cli\bin'
if (Test-Path -LiteralPath $pxelCliBin) {
    $env:PATH = $pxelCliBin + [IO.Path]::PathSeparator + $env:PATH
}
$pxelAuditBin = Join-Path $pxelRoot '.tools\audit-cli\bin'
if (Test-Path -LiteralPath $pxelAuditBin) {
    $env:PATH = $pxelAuditBin + [IO.Path]::PathSeparator + $env:PATH
}
$pxelPostgres = Join-Path $pxelRoot '.tools\postgresql-18.6\pgsql\bin'
if (Test-Path -LiteralPath $pxelPostgres) {
    $env:PXEL_PG_BIN = $pxelPostgres
}
if (Test-Path -LiteralPath (Join-Path $pxelRoot '.local\database-url')) {
    $env:DATABASE_URL = [IO.File]::ReadAllText((Join-Path $pxelRoot '.local\database-url')).Trim()
}
