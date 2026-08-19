$ErrorActionPreference = "Stop"

# Minimal immutable bootstrap for Sippion. It downloads a pinned GitHub CLI into
# a temporary directory, verifies that CLI against GitHub's published checksum,
# then uses an anonymously fetched inline attestation bundle to verify Sippion.
# Nothing from the temporary GitHub CLI is installed persistently.

$repo = "Sitten-Tokyo/Sippion"
$ghVersion = "2.97.0"
$ghChecksumsSha256 = "61905c69ec8660f310814ec98395cdd0c2d07aabf024c597ec45813984a02334"
$verifyOnly = $env:SIPPION_BOOTSTRAP_VERIFY_ONLY -eq "1"
if ($env:SIPPION_BOOTSTRAP_VERIFY_ONLY -and $env:SIPPION_BOOTSTRAP_VERIFY_ONLY -notin @("0", "1")) {
    throw "SIPPION_BOOTSTRAP_VERIFY_ONLY must be 0 or 1."
}

if ($env:PROCESSOR_ARCHITECTURE -notin @("AMD64", "x86_64")) {
    throw "Unsupported bootstrap architecture: $env:PROCESSOR_ARCHITECTURE. Windows x86_64 is supported."
}

$releaseHeaders = @{
    Accept = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2026-03-10"
}
$attestationHeaders = @{
    Accept = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2022-11-28"
}

$tempRoot = Join-Path $env:TEMP ("sippion-bootstrap-{0}" -f [Guid]::NewGuid().ToString("N"))
$originalPath = $env:PATH

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $ghArchive = "gh_${ghVersion}_windows_amd64.zip"
    $ghChecksums = Join-Path $tempRoot "gh_${ghVersion}_checksums.txt"
    $ghArchivePath = Join-Path $tempRoot $ghArchive
    $ghReleaseBase = "https://github.com/cli/cli/releases/download/v$ghVersion"

    Invoke-WebRequest -Uri "$ghReleaseBase/gh_${ghVersion}_checksums.txt" -OutFile $ghChecksums
    $actualChecksumsHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ghChecksums).Hash.ToLowerInvariant()
    if ($actualChecksumsHash -ne $ghChecksumsSha256) {
        throw "SHA-256 verification failed for the pinned GitHub CLI checksum file."
    }

    $checksumLine = Get-Content -LiteralPath $ghChecksums | Where-Object {
        $_ -match ("\s" + [regex]::Escape($ghArchive) + "$")
    } | Select-Object -First 1
    if (-not $checksumLine) {
        throw "Pinned GitHub CLI checksum entry was not found."
    }
    $ghExpected = (($checksumLine -split "\s+")[0]).ToLowerInvariant()
    if ($ghExpected -notmatch '^[0-9a-f]{64}$') {
        throw "Pinned GitHub CLI checksum entry is invalid."
    }

    Invoke-WebRequest -Uri "$ghReleaseBase/$ghArchive" -OutFile $ghArchivePath
    $ghActual = (Get-FileHash -Algorithm SHA256 -LiteralPath $ghArchivePath).Hash.ToLowerInvariant()
    if ($ghActual -ne $ghExpected) {
        throw "SHA-256 verification failed for the pinned GitHub CLI archive."
    }

    $ghExtractRoot = Join-Path $tempRoot "gh"
    Expand-Archive -LiteralPath $ghArchivePath -DestinationPath $ghExtractRoot -Force
    $ghBin = Get-ChildItem -LiteralPath $ghExtractRoot -Recurse -File -Filter "gh.exe" |
        Where-Object { $_.Directory.Name -eq "bin" } |
        Select-Object -First 1 -ExpandProperty FullName
    if ([string]::IsNullOrWhiteSpace($ghBin) -or -not (Test-Path -LiteralPath $ghBin -PathType Leaf)) {
        throw "Verified GitHub CLI archive did not contain gh.exe under a bin directory."
    }

    $releases = Invoke-RestMethod -Headers $releaseHeaders -Uri "https://api.github.com/repos/$repo/releases?per_page=1"
    $tag = if ($releases -is [array]) { $releases[0].tag_name } else { $releases.tag_name }
    if ([string]::IsNullOrWhiteSpace($tag) -or $tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Could not resolve a valid published Sippion release tag."
    }

    $releaseBase = "https://github.com/$repo/releases/download/$tag"
    $installer = Join-Path $tempRoot "install.ps1"
    $installerChecksum = Join-Path $tempRoot "install.ps1.sha256"
    Invoke-WebRequest -Uri "$releaseBase/install.ps1" -OutFile $installer
    Invoke-WebRequest -Uri "$releaseBase/install.ps1.sha256" -OutFile $installerChecksum

    $installerExpected = (((Get-Content -Raw -LiteralPath $installerChecksum) -split "\s+")[0]).ToLowerInvariant()
    if ($installerExpected -notmatch '^[0-9a-f]{64}$') {
        throw "The Sippion installer checksum is invalid."
    }
    $installerActual = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
    if ($installerActual -ne $installerExpected) {
        throw "Sippion installer checksum verification failed."
    }

    $attestationResponse = Invoke-RestMethod -Headers $attestationHeaders -Uri "https://api.github.com/repos/$repo/attestations/sha256:$installerActual?predicate_type=provenance&per_page=1"
    $bundle = $attestationResponse.attestations[0].bundle
    if ($null -eq $bundle) {
        throw "GitHub did not return an inline Sippion installer attestation bundle."
    }
    $installerBundle = Join-Path $tempRoot "installer-attestation.bundle.json"
    $bundle | ConvertTo-Json -Depth 100 -Compress | Set-Content -LiteralPath $installerBundle -Encoding utf8NoBOM
    & $ghBin attestation verify $installer --repo $repo --bundle $installerBundle *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion installer provenance verification failed."
    }

    if ($verifyOnly) {
        Write-Host "Verified Sippion bootstrap path for $tag."
        return
    }

    $env:PATH = "$(Split-Path -Parent $ghBin);$originalPath"
    $env:SIPPION_RELEASE_TAG = $tag
    $env:SIPPION_ATTESTATION_REPOSITORY = $repo
    & $installer -ReleaseTag $tag -AttestationRepository $repo
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion installer failed with exit code $LASTEXITCODE."
    }

    Write-Host ""
    Write-Host "Sippion is installed and registered for Codex, Claude Code, and Antigravity."
    Write-Host "Restart those AI clients to reload their MCP settings."
}
finally {
    $env:PATH = $originalPath
    Remove-Item Env:SIPPION_RELEASE_TAG -ErrorAction SilentlyContinue
    Remove-Item Env:SIPPION_ATTESTATION_REPOSITORY -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
