param(
  [ValidateSet("nsis", "msi", "all")]
  [string]$Bundle = "nsis",
  [switch]$SkipCheck,
  # Signs the NSIS installer for the updater and copies it to the release asset names.
  # The release operator's secret store must put the update key in TAURI_SIGNING_PRIVATE_KEY.
  [switch]$Release
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

function Write-Step($Message) {
  Write-Host ""
  Write-Host "==> $Message" -ForegroundColor Cyan
}

function Test-Command($Name) {
  return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Assert-Windows {
  $echoIsWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
  )
  if (-not $echoIsWindows) {
    throw "scripts/build-windows.ps1 must be run on Windows. This script targets the Windows Tauri toolchain and installer bundlers."
  }
}

function Invoke-Checked {
  param(
    [string]$FilePath,
    [string[]]$Arguments
  )

  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $FilePath $($Arguments -join ' ')"
  }
}

function Assert-ReleaseReady {
  if ($Bundle -ne "nsis") {
    throw "-Release builds the NSIS installer only; the updater installs Echo-windows-setup.exe."
  }
  if ([string]::IsNullOrEmpty($env:TAURI_SIGNING_PRIVATE_KEY)) {
    throw "-Release needs the update signing key in TAURI_SIGNING_PRIVATE_KEY, injected by the secret store."
  }
  $config = Get-Content -Raw (Join-Path $RepoRoot "src-tauri\tauri.conf.json") | ConvertFrom-Json
  if ($config.plugins.updater.pubkey -eq "ECHO_UPDATE_PUBLIC_KEY_NOT_SET") {
    throw "plugins.updater.pubkey in src-tauri/tauri.conf.json is still the placeholder. Commit the real public key before a release."
  }
}

Assert-Windows
if ($Release) { Assert-ReleaseReady }

Write-Step "Echo Windows build"
Write-Host "Repo: $RepoRoot"
Write-Host "Bundle: $Bundle"
Write-Host "Release: $Release"
foreach ($command in @("bun", "cargo", "rustc")) {
  if (-not (Test-Command $command)) {
    throw "$command is not available on PATH. Install the pinned Echo toolchain before building."
  }
}

Write-Step "Installing Echo dependencies"
Push-Location $RepoRoot
try {
  Invoke-Checked "bun" @("scripts/verify-toolchain.ts")
  Invoke-Checked "bun" @("install", "--frozen-lockfile")

  if (-not $SkipCheck) {
    Write-Step "Running Echo checks"
    Invoke-Checked "bun" @("run", "check")
    Invoke-Checked "bun" @("test")
  }

  Write-Step "Building Echo Windows desktop bundle"
  $bundleArg = if ($Bundle -eq "all") { "nsis,msi" } else { $Bundle }
  $buildArgs = @("run", "tauri", "build", "--ci", "--target", "x86_64-pc-windows-msvc", "--bundles", $bundleArg)
  if ($Release) { $buildArgs += @("--config", "src-tauri/tauri.release.conf.json") }
  Invoke-Checked "bun" $buildArgs
} finally {
  Pop-Location
}

$BundleDir = Join-Path $RepoRoot "src-tauri\target\x86_64-pc-windows-msvc\release\bundle"
if ($Release) {
  Write-Step "Copying the release assets"
  $version = (Get-Content -Raw (Join-Path $RepoRoot "package.json") | ConvertFrom-Json).version
  $nsis = Join-Path $BundleDir "nsis"
  $setup = Join-Path $nsis "Echo_$($version)_x64-setup.exe"
  foreach ($source in @($setup, "$setup.sig")) {
    if (-not (Test-Path $source)) { throw "Expected $source after the release build." }
  }
  Copy-Item $setup (Join-Path $nsis "Echo-windows-setup.exe") -Force
  Copy-Item "$setup.sig" (Join-Path $nsis "Echo-windows-setup.exe.sig") -Force
  Write-Host "Upload: $nsis\Echo-windows-setup.exe"
  Write-Host "Upload: $nsis\Echo-windows-setup.exe.sig"
}

Write-Step "Windows build complete"
Write-Host "Artifacts are under: $BundleDir"
