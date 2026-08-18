[CmdletBinding()]
param(
    [string]$ReleaseBaseUrl = $env:SIPPION_RELEASE_BASE_URL
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
    $ReleaseBaseUrl = "https://github.com/OWNER/REPOSITORY/releases/latest/download"
}
if ($ReleaseBaseUrl -like "*OWNER/REPOSITORY*") {
    throw "Set SIPPION_RELEASE_BASE_URL to the published Sippion release URL."
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
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash.ToLowerInvariant()
    if ([string]::IsNullOrWhiteSpace($expected) -or $actual -ne $expected) {
        throw "Sippion checksum verification failed."
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
