#!/usr/bin/env pwsh
# Build a portable Windows ZIP for Pinto Slicer.
#
# The executable is linked against the static C runtime so the ZIP runs on a
# clean Windows install without the Visual C++ redistributable.
#
# Usage: pwsh packaging/build-windows.ps1 [-OutputDir dist] [-Target x86_64-pc-windows-msvc]
#                                         [-SkipBuild] [-SkipSmokeTest]

[CmdletBinding()]
param(
    [string]$OutputDir = "dist",
    [string]$Target = "x86_64-pc-windows-msvc",
    [switch]$SkipBuild,
    [switch]$SkipSmokeTest
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $cargoToml = Get-Content Cargo.toml -Raw
    $version = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    if (-not $version) { throw "Could not parse version from Cargo.toml" }

    switch -Regex ($Target) {
        '^x86_64-' { $arch = 'x64'; break }
        '^aarch64-' { $arch = 'arm64'; break }
        default { throw "Unsupported target: $Target" }
    }

    if (-not $SkipBuild) {
        Write-Host "Building Pinto Slicer $version for $Target..."
        rustup target add $Target | Out-Null
        # Static CRT keeps the portable ZIP self-contained; keep any RUSTFLAGS
        # the caller already set.
        $rustFlags = @([Environment]::GetEnvironmentVariable('RUSTFLAGS'))
        $rustFlags = @($rustFlags | Where-Object { $_ })
        if ($rustFlags -notcontains '-C target-feature=+crt-static') {
            $rustFlags += '-C target-feature=+crt-static'
        }
        $env:RUSTFLAGS = $rustFlags -join ' '
        cargo build --release --locked --target $Target
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    }

    $exe = "target/$Target/release/pinto_slicer.exe"
    if (-not (Test-Path $exe)) { throw "Missing release binary: $exe" }

    if (-not $SkipSmokeTest) {
        $help = & $exe --help
        if ($LASTEXITCODE -ne 0 -or -not ($help -match '^Pinto Slicer')) {
            throw "The release binary failed its --help smoke test."
        }
        # The portable ZIP promises no Visual C++ redistributable; the import
        # table must not name the dynamic CRT.
        $image = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
        if ($image -match 'VCRUNTIME140|MSVCP140') {
            throw "The executable still links the dynamic C runtime; crt-static was not applied."
        }
    }

    $stage = "build/windows/Pinto Slicer"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item $exe "$stage/pinto_slicer.exe" -Force
    Copy-Item "assets/pinto-slicer.png" "$stage/pinto-slicer.png" -Force
    Copy-Item "LICENSE" "$stage/LICENSE" -Force
    Copy-Item "NOTICE" "$stage/NOTICE" -Force
    Copy-Item "README.md" "$stage/README.md" -Force

    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    $zipName = "Pinto-Slicer-$version-$arch-portable.zip"
    $zipPath = Join-Path (Resolve-Path $OutputDir).Path $zipName
    if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
    Compress-Archive -Path "$stage/*" -DestinationPath $zipPath -CompressionLevel Optimal

    $hash = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLowerInvariant()
    # sha256sum-compatible format, written without a BOM so it verifies on Linux.
    $checksumPath = Join-Path (Split-Path $zipPath) "SHA256SUMS"
    [IO.File]::WriteAllText($checksumPath, "$hash  $zipName`n", [Text.UTF8Encoding]::new($false))

    Write-Host "Created $zipPath"
    Write-Host "SHA256: $hash"
}
finally {
    Pop-Location
}
