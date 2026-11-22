<#
.SYNOPSIS
  Build loopwebm (CLI + GUI) inside Docker from a Windows host.

.EXAMPLE
  .\scripts\build.ps1                         # host OS + arch only (default)
  .\scripts\build.ps1 -All                    # all 6 targets (+ macOS universal)
  .\scripts\build.ps1 -Target linux-arm64,windows-amd64
  .\scripts\build.ps1 -Test                   # run the test suite
  .\scripts\build.ps1 -RebuildImage           # force a toolchain image rebuild

  macOS targets need a MacOSX<ver>.sdk.tar.xz in docker\sdk\ (see README).
#>
param(
  [switch]$All,
  [string[]]$Target = @(),
  [switch]$Test,
  [switch]$RebuildImage
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$AllTargets = @('linux-amd64','linux-arm64','windows-amd64','windows-arm64','macos-amd64','macos-arm64')

function Get-HostTarget {
  $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
  switch ($arch) { 'ARM64' { 'windows-arm64' } 'AMD64' { 'windows-amd64' } default { throw "Unsupported host arch $arch" } }
}

$targets = @()
if ($All) { $targets = $AllTargets }
foreach ($t in $Target) { $targets += ($t -split ',') | Where-Object { $_ } }
if (-not $Test -and $targets.Count -eq 0) {
  $targets = @(Get-HostTarget)
  Write-Host "No target given; building for this host: $($targets[0])  (use -All for every platform)"
}
foreach ($t in $targets) { if ($AllTargets -notcontains $t) { throw "Unknown target '$t' (valid: $($AllTargets -join ' '))" } }

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) { throw 'Docker is required: https://docs.docker.com/get-docker/' }

$sdk = Get-ChildItem "$Root\docker\sdk" -Filter 'MacOSX*.sdk.tar.*' -ErrorAction SilentlyContinue
if (($targets | Where-Object { $_ -like 'macos-*' }) -and -not $sdk) {
  Write-Warning 'macOS targets need a macOS SDK tarball in docker\sdk\ (see README). They will be skipped.'
}

# Toolchain image tag = hash of its inputs.
$text = (Get-Content -Raw "$Root\docker\Dockerfile") + (Get-Content -Raw "$Root\docker\build-libvpx.sh") +
        ((Get-ChildItem "$Root\docker\sdk" | ForEach-Object Name) -join "`n")
$sha = [System.Security.Cryptography.SHA256]::Create()
$hash = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($text))) -replace '-','').Substring(0,12).ToLower()
$image = "loopwebm-toolchain:$hash"

docker image inspect $image *> $null
if ($RebuildImage -or $LASTEXITCODE -ne 0) {
  Write-Host "==> Building toolchain image $image (first time takes 30-40 minutes)"
  docker build -t $image -t loopwebm-toolchain:latest -f "$Root\docker\Dockerfile" "$Root\docker"
  if ($LASTEXITCODE -ne 0) { throw 'docker build failed' }
}

$runArgs = if ($Test) { @('--test') } else { $targets }
docker run --rm -t `
  -v "${Root}:/src" `
  -v loopwebm-cargo:/cache/cargo `
  -v loopwebm-target:/cache/target `
  -e CARGO_HOME=/cache/cargo `
  -e CARGO_TARGET_DIR=/cache/target `
  $image bash /src/scripts/in-container-build.sh @runArgs
if ($LASTEXITCODE -ne 0) { throw 'build failed' }
