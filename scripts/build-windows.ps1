param(
  [ValidateSet("nsis", "msi", "all")]
  [string]$Bundle = "nsis",
  [switch]$SkipCheck
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

Assert-Windows

Write-Step "Echo Windows build"
Write-Host "Repo: $RepoRoot"
Write-Host "Bundle: $Bundle"
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
  Invoke-Checked "bun" @("run", "tauri", "build", "--ci", "--target", "x86_64-pc-windows-msvc", "--bundles", $bundleArg)
} finally {
  Pop-Location
}

Write-Step "Windows build complete"
Write-Host "Artifacts are under: $RepoRoot\src-tauri\target\x86_64-pc-windows-msvc\release\bundle"
