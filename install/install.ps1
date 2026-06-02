param(
  [string]$Version = $env:DAIRO_CLI_VERSION,
  [string]$InstallDir = $env:DAIRO_INSTALL_DIR,
  [string]$BaseUrl = $env:DAIRO_DOWNLOAD_BASE_URL
)

$ErrorActionPreference = "Stop"
if (-not $Version) { $Version = "latest" }
if (-not $InstallDir) { $InstallDir = Join-Path $env:USERPROFILE ".dairo\bin" }
if (-not $BaseUrl) { $BaseUrl = "https://dairo.app/downloads/cli" }

$arch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString().ToLowerInvariant()
switch ($arch) {
  "x64" { $target = "x86_64-pc-windows-msvc" }
  default { throw "Dairo CLI is not available for Windows architecture: $arch" }
}

$asset = "dairo-$target.zip"
$url = "$BaseUrl/$Version/$asset"
$checksumsUrl = "$BaseUrl/$Version/checksums.txt"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("dairo-cli-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  Write-Host "Downloading Dairo CLI $Version for $target..."
  $archive = Join-Path $tmp $asset
  $checksumsPath = Join-Path $tmp "checksums.txt"
  Invoke-WebRequest -Uri $url -OutFile $archive
  Invoke-WebRequest -Uri $checksumsUrl -OutFile $checksumsPath

  $line = Get-Content $checksumsPath | Where-Object { $_ -match [regex]::Escape($asset) } | Select-Object -First 1
  if (-not $line) { throw "Could not find checksum for $asset" }
  $expected = ($line -split '\s+')[0].ToLowerInvariant()
  $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
  if ($actual -ne $expected) { throw "Checksum mismatch for $asset" }

  Expand-Archive -Path $archive -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item -Force (Join-Path $tmp "dairo.exe") (Join-Path $InstallDir "dairo.exe")
  Write-Host "Dairo CLI installed to $InstallDir\dairo.exe"
  if (($env:PATH -split ';') -notcontains $InstallDir) {
    Write-Host "Add this directory to PATH: $InstallDir"
  }
  & (Join-Path $InstallDir "dairo.exe") --version
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
