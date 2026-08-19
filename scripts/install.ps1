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

$gh = Get-Command gh -ErrorAction SilentlyContinue
$ghSupportsAttestation = $false
if ($gh) {
    & gh attestation --help *> $null
    $ghSupportsAttestation = $LASTEXITCODE -eq 0
}
if ($requireAttestation -and -not $ghSupportsAttestation) {
    throw "GitHub CLI with 'gh attestation' support is required for provenance verification."
}

if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
    if ([string]::IsNullOrWhiteSpace($ReleaseTag)) {
        if (-not $ghSupportsAttestation) {
            throw "Set SIPPION_RELEASE_TAG or SIPPION_RELEASE_BASE_URL when GitHub CLI is unavailable."
        }
        $tagOutput = & gh api "repos/$AttestationRepository/releases?per_page=100" --jq 'map(select(.draft == false))[0].tag_name'
        if ($LASTEXITCODE -ne 0) {
            throw "Could not resolve the newest published Sippion release from GitHub."
        }
        $ReleaseTag = ($tagOutput | Out-String).Trim()
    }
    if ($ReleaseTag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
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

    if ($ghSupportsAttestation) {
        & gh attestation verify $binary --repo $AttestationRepository
        if ($LASTEXITCODE -ne 0) {
            throw "Sippion GitHub artifact attestation verification failed."
        }
    }
    elseif ($requireAttestation) {
        throw "GitHub CLI with 'gh attestation' support is required for provenance verification."
    }
    else {
        Write-Warning "GitHub artifact attestation verification was explicitly disabled."
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
