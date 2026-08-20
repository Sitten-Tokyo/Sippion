$ErrorActionPreference = "Stop"

# One-command bootstrap for Sippion. This bootstrap is intended to be invoked
# from a commit-SHA-pinned raw GitHub URL. It resolves the newest non-draft
# published release (including prereleases), verifies installer checksum and
# provenance before execution, then delegates binary verification and setup.

$repo = "Sitten-Tokyo/Sippion"
$verifyOnlyValue = if ($env:SIPPION_BOOTSTRAP_VERIFY_ONLY) { $env:SIPPION_BOOTSTRAP_VERIFY_ONLY } else { "0" }
if ($verifyOnlyValue -notin @("0", "1")) {
    throw "SIPPION_BOOTSTRAP_VERIFY_ONLY must be 0 or 1."
}
$verifyOnly = $verifyOnlyValue -eq "1"

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) {
    throw "GitHub CLI is required for provenance verification."
}
& gh attestation --help *> $null
if ($LASTEXITCODE -ne 0) {
    throw "A GitHub CLI version with 'gh attestation' support is required."
}

$tempRoot = Join-Path $env:TEMP ("sippion-bootstrap-{0}" -f [Guid]::NewGuid().ToString("N"))
$originalRequireAttestation = $env:SIPPION_REQUIRE_ATTESTATION
$originalReleaseTag = $env:SIPPION_RELEASE_TAG
$originalAttestationRepository = $env:SIPPION_ATTESTATION_REPOSITORY

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $tag = (& gh release list --repo $repo --exclude-drafts --limit 1 --json tagName --jq '.[0].tagName').Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($tag) -or $tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Could not resolve a valid non-draft published Sippion release tag."
    }
    $releaseSha = (& gh api "repos/$repo/commits/$tag" --jq '.sha').Trim()
    if ($LASTEXITCODE -ne 0 -or $releaseSha -notmatch '^[0-9a-f]{40}$') {
        throw "Could not resolve the published release tag to a commit SHA."
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

    & gh attestation verify $installer `
        --repo $repo `
        --signer-workflow "$repo/.github/workflows/release-draft.yml" `
        --source-digest $releaseSha *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion installer provenance verification failed."
    }

    if ($verifyOnly) {
        Write-Host "Verified Sippion bootstrap installer provenance for $tag."
        return
    }

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
