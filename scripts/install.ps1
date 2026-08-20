[CmdletBinding()]
param(
    [string]$ReleaseBaseUrl = $env:SIPPION_RELEASE_BASE_URL,
    [string]$ReleaseTag = $env:SIPPION_RELEASE_TAG,
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

$headers = @{
    Accept = "application/vnd.github+json"
    "X-GitHub-Api-Version" = "2026-03-10"
}
$apiToken = if ($env:GH_TOKEN) { $env:GH_TOKEN } else { $env:GITHUB_TOKEN }
if ([string]::IsNullOrWhiteSpace($apiToken) -and (Get-Command gh -ErrorAction SilentlyContinue)) {
    try {
        $apiToken = (& gh auth token 2>$null).Trim()
    }
    catch {
        $apiToken = $null
    }
}
if (-not [string]::IsNullOrWhiteSpace($apiToken)) {
    $headers.Authorization = "Bearer $apiToken"
}

if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
    if ([string]::IsNullOrWhiteSpace($ReleaseTag)) {
        $releases = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$AttestationRepository/releases?per_page=1"
        $ReleaseTag = if ($releases -is [array]) { $releases[0].tag_name } else { $releases.tag_name }
    }
    if ([string]::IsNullOrWhiteSpace($ReleaseTag) -or $ReleaseTag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
        throw "Resolved release tag is invalid: $ReleaseTag"
    }
    $ReleaseBaseUrl = "https://github.com/$AttestationRepository/releases/download/$ReleaseTag"
}

$baseUri = [Uri]$ReleaseBaseUrl
if (-not $baseUri.IsAbsoluteUri -or $baseUri.Scheme -ne "https") {
    throw "ReleaseBaseUrl must be an absolute HTTPS URL."
}
if ($env:PROCESSOR_ARCHITECTURE -notin @("AMD64", "x86_64")) {
    throw "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE. Windows x86_64 MSVC is currently supported."
}

$base = $ReleaseBaseUrl.TrimEnd("/")
$artifact = "sippion-windows-x86_64.exe"
$tempRoot = Join-Path $env:TEMP ("sippion-install-{0}" -f [Guid]::NewGuid().ToString("N"))
$binary = Join-Path $tempRoot $artifact
$checksum = Join-Path $tempRoot "$artifact.sha256"
$installDir = if ($env:SIPPION_INSTALL_DIR) { $env:SIPPION_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Sippion" }
$installPath = Join-Path $installDir "sippion.exe"

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
        $gh = Get-Command gh -ErrorAction SilentlyContinue
        if (-not $gh) {
            throw "GitHub CLI is required for artifact-attestation verification."
        }
        & gh attestation --help *> $null
        if ($LASTEXITCODE -ne 0) {
            throw "A GitHub CLI version with 'gh attestation' support is required."
        }
        & gh attestation verify $binary --repo $AttestationRepository
        if ($LASTEXITCODE -ne 0) {
            throw "Sippion GitHub artifact attestation verification failed."
        }
    }
    else {
        Write-Warning "GitHub artifact attestation verification was explicitly disabled."
    }

    if ($verifyOnly) {
        Write-Host "Verified Sippion release artifact $artifact."
        return
    }

    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -LiteralPath $binary -Destination $installPath -Force
    & $installPath setup
    if ($LASTEXITCODE -ne 0) {
        throw "Sippion setup failed with exit code $LASTEXITCODE."
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ";" | Where-Object { $_ })
    if ($entries -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable("Path", (($entries + $installDir) -join ";"), "User")
        Write-Host "Added $installDir to the user PATH. Open a new terminal to use sippion directly."
    }
    Write-Host "Installed Sippion at $installPath"
}
finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
