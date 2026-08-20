$ErrorActionPreference = "Stop"

# One-command bootstrap for Sippion. This bootstrap is intended to be invoked
# from a commit-SHA-pinned raw GitHub URL. It resolves the newest published
# Sippion release (including prereleases), pins all downloads to that tag,
# verifies the published installer SHA-256, then delegates binary checksum and
# GitHub artifact-attestation verification plus client registration to the
# release installer.

$repo = "Sitten-Tokyo/Sippion"
$verifyOnlyValue = if ($env:SIPPION_BOOTSTRAP_VERIFY_ONLY) { $env:SIPPION_BOOTSTRAP_VERIFY_ONLY } else { "0" }
if ($verifyOnlyValue -notin @("0", "1")) {
    throw "SIPPION_BOOTSTRAP_VERIFY_ONLY must be 0 or 1."
}
$verifyOnly = $verifyOnlyValue -eq "1"

$headers = @{
    Accept = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2026-03-10"
}
$tempRoot = Join-Path $env:TEMP ("sippion-bootstrap-{0}" -f [Guid]::NewGuid().ToString("N"))
$originalRequireAttestation = $env:SIPPION_REQUIRE_ATTESTATION
$originalReleaseTag = $env:SIPPION_RELEASE_TAG
$originalAttestationRepository = $env:SIPPION_ATTESTATION_REPOSITORY

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $releases = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$repo/releases?per_page=1"
    $tag = if ($releases -is [array]) { $releases[0].tag_name } else { $releases.tag_name }
    if ([string]::IsNullOrWhiteSpace($tag) -or $tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Could not resolve a valid published Sippion release tag."
    }

    $releaseBase = "https://github.com/$repo/releases/download/$tag"
    $installer = Join-Path $tempRoot "install.ps1"
    $installerChecksum = Join-Path $tempRoot "install.ps1.sha256"
    Invoke-WebRequest -Uri "$releaseBase/install.ps1" -OutFile $installer
    Invoke-WebRequest -Uri "$releaseBase/install.ps1.sha256" -OutFile $installerChecksum

    $expected = (((Get-Content -Raw -LiteralPath $installerChecksum) -split "\s+")[0]).ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') {
        throw "The Sippion installer checksum is invalid."
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Sippion installer checksum verification failed."
    }

    if ($verifyOnly) {
        Write-Host "Verified Sippion bootstrap installer for $tag."
        return
    }

    # Keep provenance verification enabled for the default installation path.
    # The release installer fails closed unless a GitHub CLI with
    # `gh attestation` support can verify the selected binary.
    $env:SIPPION_REQUIRE_ATTESTATION = "1"
    $env:SIPPION_RELEASE_TAG = $tag
    $env:SIPPION_ATTESTATION_REPOSITORY = $repo

    & $installer -ReleaseTag $tag -AttestationRepository $repo
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion installer failed with exit code $LASTEXITCODE."
    }

    Write-Host ""
    Write-Host "Sippion is installed and pre-registered for Codex, Claude Code, and Antigravity."
    Write-Host "Restart those AI clients to reload their MCP settings."
}
finally {
    if ($null -eq $originalRequireAttestation) { Remove-Item Env:SIPPION_REQUIRE_ATTESTATION -ErrorAction SilentlyContinue } else { $env:SIPPION_REQUIRE_ATTESTATION = $originalRequireAttestation }
    if ($null -eq $originalReleaseTag) { Remove-Item Env:SIPPION_RELEASE_TAG -ErrorAction SilentlyContinue } else { $env:SIPPION_RELEASE_TAG = $originalReleaseTag }
    if ($null -eq $originalAttestationRepository) { Remove-Item Env:SIPPION_ATTESTATION_REPOSITORY -ErrorAction SilentlyContinue } else { $env:SIPPION_ATTESTATION_REPOSITORY = $originalAttestationRepository }
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
