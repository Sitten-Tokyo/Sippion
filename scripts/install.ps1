[CmdletBinding()]
param(
    [string]$ReleaseBaseUrl = $env:SIPPION_RELEASE_BASE_URL,
    [string]$ReleaseTag = $env:SIPPION_RELEASE_TAG,
    [string]$ReleaseSha = $env:SIPPION_RELEASE_SHA,
    [string]$AttestationRepository = $env:SIPPION_ATTESTATION_REPOSITORY
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($AttestationRepository)) {
    $AttestationRepository = "Sitten-Tokyo/Sippion"
}
if ($AttestationRepository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "AttestationRepository must be owner/repository using GitHub-safe characters."
}

$requireAttestationValue = if ($env:SIPPION_REQUIRE_ATTESTATION) { $env:SIPPION_REQUIRE_ATTESTATION } else { "1" }
if ($requireAttestationValue -notin @("0", "1")) {
    throw "SIPPION_REQUIRE_ATTESTATION must be 0 or 1."
}
$requireAttestation = $requireAttestationValue -eq "1"
$verifyOnlyValue = if ($env:SIPPION_INSTALL_VERIFY_ONLY) { $env:SIPPION_INSTALL_VERIFY_ONLY } else { "0" }
if ($verifyOnlyValue -notin @("0", "1")) {
    throw "SIPPION_INSTALL_VERIFY_ONLY must be 0 or 1."
}
$verifyOnly = $verifyOnlyValue -eq "1"

function Resolve-LatestPublishedTag {
    $ghCommand = Get-Command gh -ErrorAction SilentlyContinue
    if ($ghCommand) {
        $resolved = (& gh api "repos/$AttestationRepository/releases?per_page=20" --jq '[.[] | select(.draft == false and .published_at != null)][0].tag_name // empty').Trim()
        if ($LASTEXITCODE -ne 0) {
            throw "Could not query published Sippion releases."
        }
        return $resolved
    }

    # The checksum-only opt-out may intentionally run without gh. Avoid ambient
    # credentials here so the public listing cannot expose writer-visible drafts.
    $publicHeaders = @{
        Accept = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2026-03-10"
    }
    $releases = @(Invoke-RestMethod -Headers $publicHeaders -Uri "https://api.github.com/repos/$AttestationRepository/releases?per_page=20")
    $published = @($releases | Where-Object { -not $_.draft -and $_.published_at })
    if ($published.Count -eq 0) {
        return ""
    }
    return [string]$published[0].tag_name
}

function Resolve-TagSha([string]$Tag) {
    $objectType = (& gh api "repos/$AttestationRepository/git/ref/tags/$Tag" --jq '.object.type').Trim()
    if ($LASTEXITCODE -ne 0) { throw "Could not resolve release tag $Tag." }
    $objectSha = (& gh api "repos/$AttestationRepository/git/ref/tags/$Tag" --jq '.object.sha').Trim()
    if ($LASTEXITCODE -ne 0) { throw "Could not resolve release tag $Tag." }
    while ($objectType -eq "tag") {
        $objectType = (& gh api "repos/$AttestationRepository/git/tags/$objectSha" --jq '.object.type').Trim()
        if ($LASTEXITCODE -ne 0) { throw "Could not resolve annotated release tag $Tag." }
        $objectSha = (& gh api "repos/$AttestationRepository/git/tags/$objectSha" --jq '.object.sha').Trim()
        if ($LASTEXITCODE -ne 0) { throw "Could not resolve annotated release tag $Tag." }
    }
    if ($objectType -ne "commit" -or $objectSha -notmatch '^[0-9a-f]{40}$') {
        throw "Release tag $Tag does not resolve to a commit SHA."
    }
    return $objectSha
}

if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
    if ([string]::IsNullOrWhiteSpace($ReleaseTag)) {
        $ReleaseTag = Resolve-LatestPublishedTag
    }
    if ([string]::IsNullOrWhiteSpace($ReleaseTag) -or $ReleaseTag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Resolved release tag is invalid: $ReleaseTag"
    }
    $ReleaseBaseUrl = "https://github.com/$AttestationRepository/releases/download/$ReleaseTag"
}
else {
    if ($requireAttestation -and [string]::IsNullOrWhiteSpace($ReleaseTag)) {
        throw "ReleaseTag is required with a custom ReleaseBaseUrl when attestation verification is enabled."
    }
    if (-not [string]::IsNullOrWhiteSpace($ReleaseTag) -and $ReleaseTag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Resolved release tag is invalid: $ReleaseTag"
    }
}

$baseUri = [Uri]$ReleaseBaseUrl
if (-not $baseUri.IsAbsoluteUri -or $baseUri.Scheme -ne "https") {
    throw "ReleaseBaseUrl must be an absolute HTTPS URL."
}
if ($env:PROCESSOR_ARCHITECTURE -notin @("AMD64", "x86_64")) {
    throw "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE. Windows x86_64 MSVC is currently supported."
}

$resolvedReleaseSha = ""
if ($requireAttestation) {
    $gh = Get-Command gh -ErrorAction SilentlyContinue
    if (-not $gh) {
        throw "GitHub CLI is required for artifact-attestation verification."
    }
    & gh attestation --help *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "A GitHub CLI version with 'gh attestation' support is required."
    }
    $resolvedReleaseSha = Resolve-TagSha $ReleaseTag
    if (-not [string]::IsNullOrWhiteSpace($ReleaseSha)) {
        if ($ReleaseSha -notmatch '^[0-9a-f]{40}$') {
            throw "ReleaseSha must be a 40-character lowercase Git commit SHA."
        }
        if ($ReleaseSha -ne $resolvedReleaseSha) {
            throw "ReleaseSha does not match the selected release tag."
        }
    }
}

$base = $ReleaseBaseUrl.TrimEnd("/")
$artifact = "sippion-windows-x86_64.exe"
$tempRoot = Join-Path $env:TEMP ("sippion-install-{0}" -f [Guid]::NewGuid().ToString("N"))
$binary = Join-Path $tempRoot $artifact
$checksum = Join-Path $tempRoot "$artifact.sha256"
$previousBinary = Join-Path $tempRoot "previous-sippion.exe"
$installDir = if ($env:SIPPION_INSTALL_DIR) { $env:SIPPION_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Sippion" }
$installPath = Join-Path $installDir "sippion.exe"
$hadPrevious = $false

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
    Invoke-WebRequest -Uri "$base/$artifact" -OutFile $binary
    Invoke-WebRequest -Uri "$base/$artifact.sha256" -OutFile $checksum

    $expected = ((Get-Content -Raw -LiteralPath $checksum) -split "\s+")[0].ToLowerInvariant()
    if ($expected -notmatch "^[0-9a-f]{64}$") {
        throw "The release checksum is not a valid SHA-256 digest."
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Sippion checksum verification failed."
    }

    if ($requireAttestation) {
        & gh attestation verify $binary `
            --repo $AttestationRepository `
            --signer-workflow "$AttestationRepository/.github/workflows/release-build.yml" `
            --source-digest $resolvedReleaseSha *> $null
        if ($LASTEXITCODE -ne 0) {
            throw "Sippion GitHub artifact attestation verification failed."
        }
    }
    else {
        Write-Warning "GitHub artifact attestation verification was explicitly disabled."
    }

    if ($verifyOnly) {
        if ($resolvedReleaseSha) {
            Write-Host "Verified Sippion release artifact $artifact from $resolvedReleaseSha."
        }
        else {
            Write-Host "Verified Sippion release artifact $artifact by checksum only."
        }
        return
    }

    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    if (Test-Path -LiteralPath $installPath) {
        $existing = Get-Item -LiteralPath $installPath -Force
        if ($existing.PSIsContainer -or (($existing.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            throw "Refusing to replace a directory, symlink, or reparse point at $installPath."
        }
        Copy-Item -LiteralPath $installPath -Destination $previousBinary -Force
        $hadPrevious = $true
    }

    Copy-Item -LiteralPath $binary -Destination $installPath -Force
    & $installPath setup
    $setupStatus = $LASTEXITCODE
    if ($setupStatus -ne 0) {
        if ($hadPrevious) {
            try {
                Copy-Item -LiteralPath $previousBinary -Destination $installPath -Force
                throw "Sippion setup failed with exit code $setupStatus; the previous binary was restored."
            }
            catch {
                if ($_.Exception.Message -like "Sippion setup failed with exit code*") {
                    throw
                }
                $rollbackPath = "$installPath.sippion-rollback"
                try { Copy-Item -LiteralPath $previousBinary -Destination $rollbackPath -Force } catch { }
                throw "Sippion setup failed with exit code $setupStatus and the previous binary could not be restored; a recovery copy was attempted at $rollbackPath."
            }
        }
        else {
            try {
                Remove-Item -LiteralPath $installPath -Force
            }
            catch {
                throw "Sippion setup failed with exit code $setupStatus and the newly installed binary could not be removed: $installPath"
            }
            throw "Sippion setup failed with exit code $setupStatus; the newly installed binary was removed."
        }
    }

    # PATH registration is convenience metadata, not part of the committed
    # binary/config transaction. Do not turn a registry policy failure into a
    # partially rolled-back installation after setup has succeeded.
    try {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $entries = @($userPath -split ";" | Where-Object { $_ })
        if ($entries -notcontains $installDir) {
            [Environment]::SetEnvironmentVariable("Path", (($entries + $installDir) -join ";"), "User")
            Write-Host "Added $installDir to the user PATH. Open a new terminal to use sippion directly."
        }
    }
    catch {
        Write-Warning "Sippion was installed, but the user PATH could not be updated automatically: $($_.Exception.Message)"
    }
    Write-Host "Installed Sippion at $installPath"
}
finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
