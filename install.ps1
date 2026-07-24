#requires -Version 5.1
<#
.SYNOPSIS
    simplicio-code installer for Windows — downloads a release published by
    THIS repo (github.com/wesleysimplicio/simplicio-code).

.DESCRIPTION
    Mirrors install.sh: resolves the requested (or latest) release tag,
    downloads the windows-x86_64 artifact plus SHA256SUMS.txt, verifies the
    checksum before installing anything, and installs to
    $env:LOCALAPPDATA\simplicio-code\bin.

    NOTE: as of this change, this repo's own release workflow
    (.github/workflows/release.yml) marks the Windows *build* job
    best-effort/non-blocking — see that file's header comment for the
    underlying `/dev/stdout`-in-protoc-args issue. This installer works
    once a windows-x86_64 artifact exists in a release; it does not itself
    build one.

.PARAMETER Version
    Specific version to install, e.g. "0.3.0-beta.3". Defaults to latest.

.EXAMPLE
    irm https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.ps1 | iex
#>
param(
    [string]$Version = "",
    [string]$Repo = "wesleysimplicio/simplicio-code",
    [string]$BinDir = "$env:LOCALAPPDATA\simplicio-code\bin",
    [switch]$SkipSigVerify
)

$ErrorActionPreference = "Stop"

if ($Version -and $Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._]+)?$') {
    Write-Error "Invalid version format: $Version (expected X.Y.Z or X.Y.Z-suffix)"
    exit 1
}

$apiBase = "https://api.github.com/repos/$Repo"
$dlBase = "https://github.com/$Repo/releases/download"
$platform = "windows-x86_64"

if (-not $Version) {
    Write-Host "Resolving latest release for $Repo..."
    $release = Invoke-RestMethod -Uri "$apiBase/releases/latest" -Headers @{ "User-Agent" = "simplicio-code-installer" }
    $Version = $release.tag_name -replace '^v', ''
    if (-not $Version) {
        Write-Error "Could not resolve the latest release from $apiBase/releases/latest"
        exit 1
    }
}

$tag = "v$Version"
$artifact = "simplicio-code-$Version-$platform.exe"

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ("simplicio-code-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

try {
    $artifactPath = Join-Path $workDir $artifact
    $checksumsPath = Join-Path $workDir "SHA256SUMS.txt"

    Write-Host "Downloading simplicio-code $Version ($platform)..."
    try {
        Invoke-WebRequest -Uri "$dlBase/$tag/$artifact" -OutFile $artifactPath -UseBasicParsing
    } catch {
        Write-Error "Artifact not found for $platform in release $tag. simplicio-code may not yet publish this platform."
        exit 1
    }

    try {
        Invoke-WebRequest -Uri "$dlBase/$tag/SHA256SUMS.txt" -OutFile $checksumsPath -UseBasicParsing
    } catch {
        Write-Error "SHA256SUMS.txt not found for release $tag; refusing to install an unverifiable artifact."
        exit 1
    }

    Write-Host "Verifying checksum..."
    $checksumLine = Select-String -Path $checksumsPath -Pattern ([regex]::Escape($artifact)) | Select-Object -First 1
    if (-not $checksumLine) {
        Write-Error "$artifact has no entry in SHA256SUMS.txt; refusing to install."
        exit 1
    }
    $expected = ($checksumLine.Line -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Path $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Write-Error "Checksum mismatch for $artifact (expected $expected, got $actual). Download may be truncated or tampered. Aborting."
        exit 1
    }
    Write-Host "  Checksum OK ($actual)."

    if (-not $SkipSigVerify) {
        Write-Host "Note: manifest signature verification on Windows requires openssl.exe on PATH; this installer" `
            "does not attempt it automatically. Checksum verification above already guards against a truncated" `
            "or substituted binary. See install.sh for the openssl-based signature check on macOS/Linux."
    }

    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    $dest = Join-Path $BinDir "simplicio-code.exe"
    Move-Item -Path $artifactPath -Destination $dest -Force

    Write-Host ""
    Write-Host "simplicio-code $Version installed to $dest"
    $pathDirs = $env:PATH -split ';'
    if ($pathDirs -contains $BinDir) {
        Write-Host "Run 'simplicio-code --version' to get started."
    } else {
        Write-Host "Add $BinDir to your PATH, then run 'simplicio-code --version':"
        Write-Host "  `$env:PATH = `"$BinDir;`$env:PATH`""
    }
} finally {
    Remove-Item -Path $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
