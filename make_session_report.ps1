param([string]$AcrDir = "")
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($AcrDir)) { $AcrDir = $PSScriptRoot }
$AcrDir = (Resolve-Path -LiteralPath $AcrDir).Path
$Exporter = Join-Path $AcrDir "target\release\acr_session_export.exe"
$Analyzer = Join-Path $AcrDir "target\release\acr_session_report.exe"
$TelemetryRoot = "D:\Games\ACC_Telemetry"
$RawDir = Join-Path $TelemetryRoot "raw"
$SessionsDir = Join-Path $TelemetryRoot "sessions"

if (!(Test-Path $Exporter)) { throw "Exporter not found: $Exporter" }
if (!(Test-Path $Analyzer)) { throw "Analyzer not found: $Analyzer" }
if (!(Test-Path $RawDir)) { throw "Raw directory not found: $RawDir" }
if (!(Test-Path $SessionsDir)) { New-Item -ItemType Directory -Path $SessionsDir -Force | Out-Null }

$physicsFiles = Get-ChildItem -LiteralPath $RawDir -Filter "*.rkyv" -File |
    Where-Object { $_.Name -notlike "*.graphics.rkyv" } |
    Sort-Object LastWriteTime -Descending

$state = $null
foreach ($f in $physicsFiles) {
    $stem = [IO.Path]::GetFileNameWithoutExtension($f.Name)
    $graphics = Join-Path $RawDir "$stem.graphics.rkyv"
    if (Test-Path -LiteralPath $graphics) {
        $jsonPath = Join-Path $RawDir "$stem.json"
        $state = [pscustomobject]@{ Stem=$stem; Physics=$f; Graphics=Get-Item $graphics; Json=if(Test-Path $jsonPath){Get-Item $jsonPath}else{$null} }
        break
    }
}
if ($null -eq $state) { throw "No complete telemetry session found in $RawDir" }

$stem = $state.Stem
$sessionDir = Join-Path $SessionsDir $stem
New-Item -ItemType Directory -Path $sessionDir -Force | Out-Null
foreach ($f in @($state.Physics,$state.Graphics,$state.Json)) {
    if ($null -ne $f) { Copy-Item $f.FullName (Join-Path $sessionDir $f.Name) -Force }
}

$physics = Join-Path $sessionDir "$stem.rkyv"
$graphics = Join-Path $sessionDir "$stem.graphics.rkyv"
$tempReport = Join-Path $sessionDir "__report_temp.html"

& $Exporter $physics $graphics $sessionDir
if ($LASTEXITCODE -ne 0) { throw "Session exporter failed with exit code $LASTEXITCODE" }
& $Analyzer $physics $tempReport
if ($LASTEXITCODE -ne 0) { throw "Report analyzer failed with exit code $LASTEXITCODE" }
if (!(Test-Path $tempReport)) { throw "Report was not created: $tempReport" }

$track = "Unknown Track"; $car = "Unknown Car"
$metaPath = Join-Path $sessionDir "$stem.json"
if (Test-Path $metaPath) {
    try {
        $meta = Get-Content $metaPath -Raw -Encoding UTF8 | ConvertFrom-Json
        function Find-JsonString([object]$node,[string[]]$wanted) {
            if ($null -eq $node) { return $null }
            if ($node -is [PSCustomObject]) {
                foreach($key in $wanted){ $p=$node.PSObject.Properties[$key]; if($null -ne $p -and $null -ne $p.Value -and -not [string]::IsNullOrWhiteSpace([string]$p.Value)){return [string]$p.Value} }
                foreach($p in $node.PSObject.Properties){$v=Find-JsonString $p.Value $wanted;if($null -ne $v){return $v}}
            } elseif ($node -is [Collections.IEnumerable] -and $node -isnot [string]) { foreach($x in $node){$v=Find-JsonString $x $wanted;if($null -ne $v){return $v}} }
            return $null
        }
        $t=Find-JsonString $meta @('track','track_name','trackName','track_id','trackId'); $c=Find-JsonString $meta @('car_model','carModel','car','vehicle_model','vehicleModel')
        if($t){$track=$t}; if($c){$car=$c}
    } catch { Write-Warning "Could not read metadata: $($_.Exception.Message)" }
}
$date=(Get-Item $physics).LastWriteTime.ToString('yyyy-MM-dd')
$safeCar=[regex]::Replace($car,'[<>:"/\\|?*]','-'); $safeTrack=[regex]::Replace($track,'[<>:"/\\|?*]','-')
$finalReport=Join-Path $sessionDir "$safeCar - $safeTrack - $date.html"
if(Test-Path $finalReport){Remove-Item $finalReport -Force}; Move-Item $tempReport $finalReport -Force
Write-Host "Report generated: $finalReport"
Start-Process $finalReport
