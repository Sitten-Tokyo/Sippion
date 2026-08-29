param(
    [string]$ReleaseTag = "__RELEASE_TAG__",
    [string]$ReleaseCommit = "__RELEASE_SHA__",
    [string]$InstallerSha256 = "__INSTALLER_PS1_SHA256__",
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"
$repo = "Sitten-Tokyo/Sippion"
$tag = if ($env:SIPPION_RELEASE_TAG) { $env:SIPPION_RELEASE_TAG } else { $ReleaseTag }
$releaseSha = if ($env:SIPPION_RELEASE_SHA) { $env:SIPPION_RELEASE_SHA } else { $ReleaseCommit }
$expectedInstallerSha256 = if ($env:SIPPION_INSTALLER_PS1_SHA256) { $env:SIPPION_INSTALLER_PS1_SHA256 } else { $InstallerSha256 }
$verifyOnly = $VerifyOnly -or ($env:SIPPION_BOOTSTRAP_VERIFY_ONLY -eq "1")
$installerUrl = "https://github.com/$repo/releases/download/$tag/install.ps1"

if ($tag -eq "__RELEASE_TAG__" -or $tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Sippion bootstrap is not pinned to a valid release tag."
}
if ($releaseSha -eq "__RELEASE_SHA__" -or $releaseSha -notmatch '^[0-9a-f]{40}$') {
    throw "Sippion bootstrap is not pinned to a valid release commit."
}
if ($expectedInstallerSha256 -eq "__INSTALLER_PS1_SHA256__" -or $expectedInstallerSha256 -notmatch '^[0-9a-f]{64}$') {
    throw "Sippion bootstrap is not pinned to a valid install.ps1 checksum."
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw "GitHub CLI (gh) is required to verify the Sippion release attestation before installation."
}

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("sippion-bootstrap-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$installer = Join-Path $tempRoot "install.ps1"
$originalRequireAttestation = $env:SIPPION_REQUIRE_ATTESTATION
$originalReleaseTag = $env:SIPPION_RELEASE_TAG
$originalAttestationRepository = $env:SIPPION_ATTESTATION_REPOSITORY

try {
    $headers = @{ Accept = "application/vnd.github+json" }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/tags/$tag" -Headers $headers
    if ($release.target_commitish -ne $releaseSha) {
        throw "Sippion release target does not match the pinned release commit."
    }

    Invoke-WebRequest -UseBasicParsing -Uri $installerUrl -OutFile $installer
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
    if ($actual -ne $expectedInstallerSha256) {
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
        Write-Information "Verified Sippion bootstrap installer provenance for $tag." -InformationAction Continue
        return
    }

    $env:SIPPION_REQUIRE_ATTESTATION = "1"
    $env:SIPPION_RELEASE_TAG = $tag
    $env:SIPPION_ATTESTATION_REPOSITORY = $repo

    & $installer -ReleaseTag $tag -AttestationRepository $repo
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion installer failed with exit code $LASTEXITCODE."
    }

    Write-Information "" -InformationAction Continue
    Write-Information "Sippion is installed and pre-registered for Codex, Claude Code, and Antigravity." -InformationAction Continue
    Write-Information "Restart those AI clients to reload their MCP settings." -InformationAction Continue
}
finally {
    if ($null -eq $originalRequireAttestation) { Remove-Item Env:SIPPION_REQUIRE_ATTESTATION -ErrorAction SilentlyContinue } else { $env:SIPPION_REQUIRE_ATTESTATION = $originalRequireAttestation }
    if ($null -eq $originalReleaseTag) { Remove-Item Env:SIPPION_RELEASE_TAG -ErrorAction SilentlyContinue } else { $env:SIPPION_RELEASE_TAG = $originalReleaseTag }
    if ($null -eq $originalAttestationRepository) { Remove-Item Env:SIPPION_ATTESTATION_REPOSITORY -ErrorAction SilentlyContinue } else { $env:SIPPION_ATTESTATION_REPOSITORY = $originalAttestationRepository }
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
