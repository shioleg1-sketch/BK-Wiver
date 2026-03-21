param(
    [Parameter(Mandatory = $true)]
    [string]$HostExe,

    [Parameter(Mandatory = $true)]
    [string]$ConsoleExe,

    [string]$FfmpegExe = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot

function Resolve-FfmpegExe {
    param(
        [string]$PreferredPath
    )

    if ($PreferredPath -and (Test-Path $PreferredPath)) {
        return (Resolve-Path $PreferredPath).Path
    }

    $bundledPath = Join-Path $repoRoot "Host\third_party\ffmpeg\windows-x64\ffmpeg.exe"
    if (Test-Path $bundledPath) {
        return (Resolve-Path $bundledPath).Path
    }

    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command -and (Test-Path $command.Source)) {
        return $command.Source
    }

    throw "ffmpeg.exe not found. Provide -FfmpegExe, run scripts/fetch_ffmpeg_windows.ps1, or install ffmpeg into PATH."
}

function Ensure-FileExists {
    param(
        [string]$Path,
        [string]$Label
    )

    if (-not (Test-Path $Path)) {
        throw "$Label does not exist: $Path"
    }
}

$hostExe = (Resolve-Path $HostExe).Path
$consoleExe = (Resolve-Path $ConsoleExe).Path
$ffmpegExe = Resolve-FfmpegExe -PreferredPath $FfmpegExe

Ensure-FileExists -Path $hostExe -Label "Host executable"
Ensure-FileExists -Path $consoleExe -Label "Console executable"
Ensure-FileExists -Path $ffmpegExe -Label "FFmpeg executable"

$targets = @(
    @{
        Directory = Join-Path $repoRoot "Host\installer\windows\stage"
        ExecutableSource = $hostExe
        ExecutableName = "bk-wiver-host.exe"
    },
    @{
        Directory = Join-Path $repoRoot "Consol\installer\windows\stage"
        ExecutableSource = $consoleExe
        ExecutableName = "bk-wiver-console.exe"
    },
    @{
        Directory = Join-Path $repoRoot "dist\windows\host"
        ExecutableSource = $hostExe
        ExecutableName = "bk-wiver-host.exe"
    },
    @{
        Directory = Join-Path $repoRoot "dist\windows\console"
        ExecutableSource = $consoleExe
        ExecutableName = "bk-wiver-console.exe"
    }
)

foreach ($target in $targets) {
    New-Item -ItemType Directory -Force -Path $target.Directory | Out-Null
    Copy-Item -Force $target.ExecutableSource (Join-Path $target.Directory $target.ExecutableName)
    Copy-Item -Force $ffmpegExe (Join-Path $target.Directory "ffmpeg.exe")
}

Write-Host "Prepared Windows stage and portable bundles with ffmpeg.exe"
