param(
    [string]$OutputPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'fixtures\formulas\excel-number-precision-2026-10-02.json')
)

$formulas = @(
    '=ROUND(2.15,1)',
    '=ROUND(2.149,1)',
    '=ROUND(-1.475,2)',
    '=ROUND(1.005,2)',
    '=ROUND(0.49999999999999994,0)',
    '=ROUNDUP(-3.14159,3)',
    '=ROUNDDOWN(-3.14159,3)',
    '=ROUND(123.45,-1.9)',
    '=ROUNDUP(1,-309)',
    '=ROUND(1,-309)',
    '=ROUND(2.675,2)',
    '=ROUND(-2.675,2)',
    '=ROUND(1.2345,3)',
    '=ROUND(-1.2345,3)',
    '=ROUND(1.25,1)',
    '=ROUND(-1.25,1)',
    '=ROUNDUP(1.0000000000000002,0)',
    '=ROUNDUP(1.0000000000000008,0)',
    '=1.234567890123456',
    '=1.2345678901234567',
    '=1234567890123456',
    '=9999999999999999',
    '=0.00000000000000012345678901234567',
    '=1.2345678901234567E+30',
    '=ROUNDUP(1.2345678901234567,0)',
    '=123456789012345'
)

$excel = $null
$workbook = $null
$sheet = $null
try {
    $excel = New-Object -ComObject Excel.Application
    $excel.Visible = $false
    $excel.DisplayAlerts = $false
    $excel.ScreenUpdating = $false
    $workbook = $excel.Workbooks.Add()
    $sheet = $workbook.Worksheets.Item(1)

    for ($index = 0; $index -lt $formulas.Count; $index++) {
        $sheet.Cells.Item($index + 1, 1).Formula = $formulas[$index]
    }
    $excel.CalculateFullRebuild()

    $cases = for ($index = 0; $index -lt $formulas.Count; $index++) {
        $cell = $sheet.Cells.Item($index + 1, 1)
        $value = $cell.Value2
        $display = [string]$cell.Text
        $isError = $display -match '^#(?:NUM|VALUE|DIV/0|REF|NAME|N/A|NULL)!$'
        $tolerance = if ($formulas[$index] -eq '=0.00000000000000012345678901234567') { 1e-30 } else { $null }
        [pscustomobject]@{
            formula = $formulas[$index]
            excel_formula = [string]$cell.Formula
            result_type = if ($isError) { 'error' } else { 'number' }
            result = if ($isError) { $display } else { [double]$value }
            tolerance = $tolerance
        }
    }

    $office = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Office\ClickToRun\Configuration' -ErrorAction SilentlyContinue
    if (-not $office) {
        $office = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Office\ClickToRun\Configuration' -ErrorAction SilentlyContinue
    }
    $environment = [pscustomobject]@{
        edition = 'Microsoft Excel for Microsoft 365'
        version = [string]$excel.Version
        build = if ($office.VersionToReport) { [string]$office.VersionToReport } else { [string]$excel.Build }
        operating_system = [Environment]::OSVersion.Version.ToString()
        locale = [System.Globalization.CultureInfo]::CurrentCulture.Name
        date_system = 1900
    }
    $fixture = [pscustomobject]@{
        schema_version = 1
        verified_in_excel = $true
        observed_at = [DateTime]::UtcNow.ToString('yyyy-MM-dd')
        verifier = 'Repository Excel COM capture script'
        excel_environment = $environment
        cases = @($cases)
    }

    $resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
    $outputDirectory = [System.IO.Path]::GetDirectoryName($resolvedOutputPath)
    [System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
    $json = ConvertTo-Json -InputObject $fixture -Depth 8
    [System.IO.File]::WriteAllText($resolvedOutputPath, "$json`n", [System.Text.UTF8Encoding]::new($false))
    Write-Output "Captured $($cases.Count) Excel cases to $resolvedOutputPath"
}
finally {
    if ($workbook) { $workbook.Close($false) | Out-Null }
    if ($excel) { $excel.Quit() }
    if ($sheet) { [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($sheet) }
    if ($workbook) { [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($workbook) }
    if ($excel) { [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($excel) }
}
