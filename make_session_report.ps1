$ErrorActionPreference = "Stop"

Write-Host "=========================================="
Write-Host "       ACC TELEMETRY SESSION REPORT"
Write-Host "=========================================="
Write-Host ""

$AcrDir        = $PSScriptRoot
$Exporter      = Join-Path $AcrDir "target\release\acr_session_export.exe"
$Analyzer      = Join-Path $AcrDir "target\release\acr_session_report.exe"
$BaseDir       = "D:\Games\ACC_Telemetry"
$RawDir        = Join-Path $BaseDir "raw"
$SessionsDir   = Join-Path $BaseDir "sessions"
$MinimumBytes  = 1MB
$StableSeconds = 8

Write-Host "ACR directory : $AcrDir"
Write-Host "Exporter      : $Exporter"
Write-Host "Analyzer      : $Analyzer"
Write-Host "Telemetry root: $BaseDir"
Write-Host "Sessions      : $SessionsDir"
Write-Host ""

if (!(Test-Path -LiteralPath $Exporter)) { throw "Exporter not found: $Exporter" }
if (!(Test-Path -LiteralPath $Analyzer)) { throw "Analyzer not found: $Analyzer" }
if (!(Test-Path -LiteralPath $BaseDir)) { throw "Telemetry root not found: $BaseDir" }
if (!(Test-Path -LiteralPath $SessionsDir)) {
    New-Item -ItemType Directory -Path $SessionsDir -Force | Out-Null
}

function Get-CompleteStableRecording {
    param([Parameter(Mandatory=$true)][string]$PhysicsPath)

    $physics = Get-Item -LiteralPath $PhysicsPath
    $stem = [IO.Path]::GetFileNameWithoutExtension($physics.Name)
    $dir  = $physics.DirectoryName
    $gfx  = Join-Path $dir "$stem.graphics.rkyv"
    $json = Join-Path $dir "$stem.json"

    if (!(Test-Path -LiteralPath $gfx) -or !(Test-Path -LiteralPath $json)) { return $null }

    $p1 = Get-Item -LiteralPath $physics.FullName
    $g1 = Get-Item -LiteralPath $gfx
    $j1 = Get-Item -LiteralPath $json

    if ($p1.Length -lt $MinimumBytes -or $g1.Length -lt 1024 -or $j1.Length -lt 2) { return $null }

    Start-Sleep -Seconds $StableSeconds

    $p2 = Get-Item -LiteralPath $physics.FullName
    $g2 = Get-Item -LiteralPath $gfx
    $j2 = Get-Item -LiteralPath $json

    if ($p1.Length -ne $p2.Length -or $g1.Length -ne $g2.Length -or $j1.Length -ne $j2.Length) {
        return $null
    }

    [PSCustomObject]@{
        Stem      = $stem
        SourceDir = $dir
        Physics   = $p2
        Graphics  = $g2
        Json      = $j2
    }
}

function Get-LatestSavedReport {
    Get-ChildItem -LiteralPath $SessionsDir -Recurse -Filter "*.html" -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
}

$candidates = @()

if (Test-Path -LiteralPath $RawDir) {
    $candidates += Get-ChildItem -LiteralPath $RawDir -Filter "acc_physics_*.rkyv" -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notlike "*.graphics.rkyv" }
}

$candidates += Get-ChildItem -LiteralPath $BaseDir -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -ne $SessionsDir -and $_.Name -ne "raw" } |
    ForEach-Object {
        Get-ChildItem -LiteralPath $_.FullName -Filter "acc_physics_*.rkyv" -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notlike "*.graphics.rkyv" }
    }

$candidates = @(
    $candidates |
    Sort-Object LastWriteTime -Descending |
    ForEach-Object { $_.FullName } |
    Select-Object -Unique
)

$state = $null

foreach ($candidatePath in $candidates) {
    Write-Host "Checking: $candidatePath"

    try {
        $candidate = Get-Item -LiteralPath $candidatePath
        $stem = [IO.Path]::GetFileNameWithoutExtension($candidate.Name)

        $archived = Get-ChildItem -LiteralPath $SessionsDir -Directory -ErrorAction SilentlyContinue |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName "$stem.rkyv") } |
            Select-Object -First 1

        if ($archived) {
            Write-Host "  -> archived session found; regenerating report from archive."
            $state = [PSCustomObject]@{
                Stem      = $stem
                SourceDir = $archived.FullName
                Physics   = Get-Item -LiteralPath (Join-Path $archived.FullName "$stem.rkyv")
                Graphics  = Get-Item -LiteralPath (Join-Path $archived.FullName "$stem.graphics.rkyv")
                Json      = Get-Item -LiteralPath (Join-Path $archived.FullName "$stem.json")
            }
            break
        }

        $test = Get-CompleteStableRecording -PhysicsPath $candidate.FullName
        if ($null -ne $test) {
            $state = $test
            break
        }

        Write-Host "  -> not complete/stable, skipping."
    }
    catch {
        Write-Host "  -> check failed, skipping: $($_.Exception.Message)"
    }
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
            Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $sessionDir $f.Name) -Force
            Write-Host "Copied: $($f.Name)"
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
    & $Analyzer $sessionDir $tempReport
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
    Write-Host "Report: $finalReport"
}
else {
    Write-Host ""
    Write-Host "No new completed session found."
}

$latest = Get-LatestSavedReport
if ($latest) {
    Write-Host ""
    Write-Host "Latest saved session:"
    Write-Host "  $($latest.FullName)"
    Write-Host ""
    Start-Process -FilePath $latest.FullName
}

Write-Host ""
Write-Host "Done."
