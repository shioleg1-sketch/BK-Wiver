$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$targetDir = Join-Path $repoRoot "Host\third_party\ffmpeg\windows-x64"
$tempRoot = Join-Path $env:TEMP "bk-wiver-ffmpeg"
$zipPath = Join-Path $tempRoot "ffmpeg-release-essentials.zip"
$extractDir = Join-Path $tempRoot "extract"
$downloadUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"

New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

Write-Host "Downloading FFmpeg from $downloadUrl"
Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath

if (Test-Path $extractDir) {
    Remove-Item -Recurse -Force $extractDir
}

Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

$ffmpegExe = Get-ChildItem -Path $extractDir -Filter "ffmpeg.exe" -Recurse | Select-Object -First 1
$ffprobeExe = Get-ChildItem -Path $extractDir -Filter "ffprobe.exe" -Recurse | Select-Object -First 1
$dllFiles = Get-ChildItem -Path $extractDir -Filter "*.dll" -Recurse

if (-not $ffmpegExe) {
    throw "ffmpeg.exe not found in downloaded archive"
}

if (-not $ffprobeExe) {
    throw "ffprobe.exe not found in downloaded archive"
}

Copy-Item -Force $ffmpegExe.FullName (Join-Path $targetDir "ffmpeg.exe")
Copy-Item -Force $ffprobeExe.FullName (Join-Path $targetDir "ffprobe.exe")

foreach ($dll in $dllFiles) {
    Copy-Item -Force $dll.FullName (Join-Path $targetDir $dll.Name)
}

Write-Host "FFmpeg prepared in $targetDir"
