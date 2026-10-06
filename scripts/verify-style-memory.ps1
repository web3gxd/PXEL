param([ValidateRange(3, 10)][int]$Runs = 3)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')

$env:CARGO_TARGET_DIR = '.local/target-style-memory'
$env:CARGO_INCREMENTAL = '0'
cargo build --locked --release -p spreadsheet-domain --example style_memory_probe
if ($LASTEXITCODE -ne 0) { throw "Style memory probe build failed ($LASTEXITCODE)." }

$executable = (Resolve-Path -LiteralPath '.local/target-style-memory/release/examples/style_memory_probe.exe').Path
$env:DATABASE_URL = $null
$scratchRoot = Join-Path (Resolve-Path -LiteralPath '.local').Path 'style-memory-samples'
[void](New-Item -ItemType Directory -Path $scratchRoot -Force)
$measurements = [System.Collections.Generic.List[object]]::new()

function Get-Median([double[]]$Values) {
    $sorted = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
    return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2
}

function Read-ProbeOutput([string]$Path) {
    $stream = $null
    $reader = $null
    try {
        $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        $reader = [IO.StreamReader]::new($stream)
        return $reader.ReadToEnd()
    } finally {
        if ($reader) { $reader.Dispose() }
        elseif ($stream) { $stream.Dispose() }
    }
}

for ($run = 1; $run -le $Runs; $run++) {
    foreach ($mode in @('axis-only', 'rich-styles')) {
        $sampleId = [guid]::NewGuid().ToString('N')
        $stdoutPath = Join-Path $scratchRoot "$sampleId.stdout.log"
        $stderrPath = Join-Path $scratchRoot "$sampleId.stderr.log"
        $process = Start-Process -FilePath $executable -ArgumentList $mode -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
        $ready = $false
        for ($attempt = 0; $attempt -lt 200; $attempt++) {
            if ($process.HasExited) { throw "Probe exited before ready ($mode, run $run)." }
            if ((Test-Path -LiteralPath $stdoutPath) -and (Read-ProbeOutput $stdoutPath).Contains('ready mode=')) { $ready = $true; break }
            Start-Sleep -Milliseconds 10
        }
        if (-not $ready) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue; throw "Probe did not become ready ($mode, run $run)." }

        $privateBytes = [System.Collections.Generic.List[double]]::new()
        $workingSet = [System.Collections.Generic.List[double]]::new()
        for ($sample = 0; $sample -lt 100; $sample++) {
            try {
                $current = Get-Process -Id $process.Id -ErrorAction Stop
                $current.Refresh()
                $privateBytes.Add([double]$current.PrivateMemorySize64)
                $workingSet.Add([double]$current.WorkingSet64)
            } catch {
                throw "Probe exited during sampling ($mode, run $run)."
            }
            Start-Sleep -Milliseconds 10
        }
        $measurements.Add([pscustomobject]@{
            run = $run
            mode = $mode
            private_median_bytes = Get-Median $privateBytes.ToArray()
            private_max_bytes = ($privateBytes | Measure-Object -Maximum).Maximum
            working_set_median_bytes = Get-Median $workingSet.ToArray()
            working_set_max_bytes = ($workingSet | Measure-Object -Maximum).Maximum
        })
        Wait-Process -Id $process.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
}

$axisMedian = Get-Median @($measurements | Where-Object mode -eq 'axis-only' | ForEach-Object private_median_bytes)
$richMedian = Get-Median @($measurements | Where-Object mode -eq 'rich-styles' | ForEach-Object private_median_bytes)
$incrementalBytes = $richMedian - $axisMedian
$budgetBytes = 16 * 1024 * 1024
$report = [pscustomobject]@{
    kind = 'PXEL sparse-style process memory budget'
    host = [pscustomobject]@{
        os = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
        cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
        logical_cpus = [Environment]::ProcessorCount
    }
    styled_cells = 100000
    modes = $measurements
    incremental_rich_style_private_bytes_median = $incrementalBytes
    incremental_budget_bytes = $budgetBytes
    budget_passed = $incrementalBytes -le $budgetBytes
}
$report | ConvertTo-Json -Depth 5
if (-not $report.budget_passed) { throw '100,000 mixed full styles exceeded the 16 MiB incremental private-memory budget.' }
