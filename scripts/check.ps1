[CmdletBinding()]
param(
    [switch]$BuildInstaller
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
Import-Module (Join-Path $PSScriptRoot 'GateRunner.psm1') -Force

$checks = @(
    @{ Name = 'Frontend TypeScript and Vite build'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'build'); Optional = $false },
    @{ Name = 'Locale validation'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'check:locales'); Optional = $false },
    @{ Name = 'UI literal lint'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'lint:ui'); Optional = $false },
    @{ Name = 'Source policy'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'check:source'); Optional = $false },
    @{ Name = 'Policy tests'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'test:policy'); Optional = $false },
    @{ Name = 'Rust formatting'; FilePath = 'cargo.exe'; ArgumentList = @('fmt', '--check', '--manifest-path', 'src-tauri/Cargo.toml'); Optional = $false },
    @{ Name = 'Rust Clippy'; FilePath = 'cargo.exe'; ArgumentList = @('clippy', '--manifest-path', 'src-tauri/Cargo.toml', '--all-targets', '--', '-D', 'warnings'); Optional = $false },
    @{ Name = 'Rust tests'; FilePath = 'cargo.exe'; ArgumentList = @('test', '--manifest-path', 'src-tauri/Cargo.toml'); Optional = $false }
)

if ($BuildInstaller) {
    $checks += @{ Name = 'Tauri Windows installer'; FilePath = 'npm.cmd'; ArgumentList = @('run', 'tauri', 'build'); Optional = $false }
}

$results = Invoke-GateSequence -Checks $checks
$passed = @($results | Where-Object { $_.Status -eq 'Passed' }).Count
$skipped = @($results | Where-Object { $_.Status -eq 'Skipped' }).Count
Write-Host "Validation complete: $passed passed, $skipped skipped."
