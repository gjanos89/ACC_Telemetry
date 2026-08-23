param(
    [string]$AcrDir = ""
)

$ErrorActionPreference = "Stop"

# Resolve the project directory from the script location instead of a hard-coded
# Windows user profile path. This avoids UTF-8/ANSI issues with names such as
# Gönczi János and also makes the script portable to another machine.
if ([string]::IsNullOrWhiteSpace($AcrDir)) {
    $AcrDir = $PSScriptRoot
}
$AcrDir = (Resolve-Path -LiteralPath $AcrDir).Path

$Exporter = Join-Path $AcrDir "target\release\acr_session_export.exe"
$Analyzer = Join-Path $AcrDir "target\release\acr_session_report.exe"
$TelemetryRoot = "D:\Games\ACC_Telemetry"
$SessionsDir = Join-Path $TelemetryRoot "sessions"

Write-Host "=========================================="
Write-Host "       ACC TELEMETRY SESSION REPORT"
Write-Host "=========================================="
Write-Host ""
Write-Host "ACR directory : $AcrDir"
Write-Host "Exporter      : $Exporter"
Write-Host "Analyzer      : $Analyzer"
Write-Host "Telemetry root: $TelemetryRoot"
Write-Host "Sessions      : $SessionsDir"
Write-Host ""

# Build missing release binaries automatically. Cargo will reuse existing
# artifacts, so this does not require cargo clean and only recompiles what
# actually changed.
if (!(Test-Path -LiteralPath $Exporter) -or !(Test-Path -LiteralPath $Analyzer)) {
    Write-Host "Release binaries missing; building required targets..."
    Push-Location $AcrDir
    try {
        if (!(Test-Path -LiteralPath $Exporter)) {
            cargo build --release --bin acr_session_export
            if ($LASTEXITCODE -ne 0) { throw "Failed to build acr_session_export" }
        }
        if (!(Test-Path -LiteralPath $Analyzer)) {
            cargo build --release --bin acr_session_report
            if ($LASTEXITCODE -ne 0) { throw "Failed to build acr_session_report" }
        }
    } finally {
        Pop-Location
    }
}

if (!(Test-Path -LiteralPath $Exporter)) { throw "Exporter not found: $Exporter" }
if (!(Test-Path -LiteralPath $Analyzer)) { throw "Analyzer not found: $Analyzer" }
if (!(Test-Path -LiteralPath $SessionsDir)) { New-Item -ItemType Directory -Path $SessionsDir -Force | Out-Null }

$physicsFiles = Get-ChildItem -LiteralPath $TelemetryRoot -Recurse -Filter "*.rkyv" -File |
    Where-Object { $_.Name -notlike "*.graphics.rkyv" }

if (!$physicsFiles) {
    throw "No physics rkyv files found under $TelemetryRoot"
}

$state = $null
foreach ($f in $physicsFiles) {
    Write-Host "Checking: $($f.FullName)"

    $stem = [System.IO.Path]::GetFileNameWithoutExtension($f.Name)
    $graphics = Join-Path $f.DirectoryName "$stem.graphics.rkyv"
    if (!(Test-Path -LiteralPath $graphics)) {
        continue
    }

    $sessionJson = Join-Path $f.DirectoryName "$stem.session.json"
    if (Test-Path -LiteralPath $sessionJson) {
        Write-Host "  -> archived session found; regenerating report from archive."
    } else {
        Write-Host "  -> session source found."
    }

    $state = [pscustomobject]@{
        Stem       = $stem
        SourceDir  = $f.DirectoryName
        Physics    = $f
        Graphics   = Get-Item -LiteralPath $graphics
        Json       = if (Test-Path -LiteralPath (Join-Path $f.DirectoryName "$stem.json")) { Get-Item -LiteralPath (Join-Path $f.DirectoryName "$stem.json") } else { $null }
    }
    break
}

if ($null -ne $state) {
    $stem = $state.Stem
    $sessionDir = Join-Path $SessionsDir $stem

    Write-Host ""
    Write-Host "Selected completed session:"
    Write-Host "  $stem"
    Write-Host ""

    if (!(Test-Path -LiteralPath $sessionDir)) {
        New-Item -ItemType Directory -Path $sessionDir -Force | Out-Null
    }

    if ($state.SourceDir -ne $sessionDir) {
        foreach ($f in @($state.Physics, $state.Graphics, $state.Json)) {
            if ($null -ne $f) {
                Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $sessionDir $f.Name) -Force
                Write-Host "Copied: $($f.Name)"
            }
        }
    }

    $physics = Join-Path $sessionDir "$stem.rkyv"
    $graphics = Join-Path $sessionDir "$stem.graphics.rkyv"
    $tempReport = Join-Path $sessionDir "__report_temp.html"

    Write-Host ""
    Write-Host "Exporting rkyv -> stable session data..."
    & $Exporter $physics $graphics $sessionDir
    if ($LASTEXITCODE -ne 0) { throw "Session exporter failed with exit code $LASTEXITCODE" }

    $sessionJson = Join-Path $sessionDir "$stem.session.json"
    if (!(Test-Path -LiteralPath $sessionJson)) { throw "Exporter did not create $sessionJson" }

    Write-Host ""
    Write-Host "Generating HTML report from exported data..."
    # The analyzer expects the actual physics .rkyv path. Passing the session
    # directory makes it look for the graphics sidecar one level too high.
    & $Analyzer $physics $tempReport
    if ($LASTEXITCODE -ne 0) { throw "Report analyzer failed with exit code $LASTEXITCODE" }
    if (!(Test-Path -LiteralPath $tempReport)) { throw "Report was not created: $tempReport" }

    $metaRaw = Get-Content -LiteralPath $sessionJson -Raw -Encoding UTF8 | ConvertFrom-Json
    $track = if ($metaRaw.track_name) { $metaRaw.track_name } else { "Unknown Track" }
    $car   = if ($metaRaw.car_name)   { $metaRaw.car_name }   else { "Unknown Car" }
    $date  = (Get-Item -LiteralPath $physics).LastWriteTime.ToString("yyyy-MM-dd")
    $safeCar = [regex]::Replace($car, '[<>:"/\\|?*]', '-')
    $safeTrack = [regex]::Replace($track, '[<>:"/\\|?*]', '-')
    $finalReport = Join-Path $sessionDir "$safeCar - $safeTrack - $date.html"

    if (Test-Path -LiteralPath $finalReport) { Remove-Item -LiteralPath $finalReport -Force }
    Move-Item -LiteralPath $tempReport -Destination $finalReport -Force

    Write-Host ""
    Write-Host "Report generated: $finalReport"
    Write-Host ""
} else {
    throw "No completed telemetry session found."
}
