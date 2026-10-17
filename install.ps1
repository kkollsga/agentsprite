<#
.SYNOPSIS
  Install pxl (AgentSprite) from a GitHub Release on Windows (x86_64).

.EXAMPLE
  irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1 | iex

.EXAMPLE
  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.ps1))) -Version v0.1.0

.PARAMETER Version
  Release tag to install (default: the latest). Environment: PXL_VERSION.
.PARAMETER Dir
  Install folder (default: %LOCALAPPDATA%\pxl\bin). Environment: PXL_INSTALL_DIR.
.PARAMETER NoSkills
  Do not run `pxl skill install` afterwards. Environment: PXL_INSTALL_SKILLS=0.

  PXL_RELEASE_BASE_URL replaces https://github.com/kkollsga/agentsprite/releases (it must hold download/<tag>/<asset>
  and, when no version is given, LATEST). It is for mirrors and tests.
#>
param(
  [string]$Version = $env:PXL_VERSION,
  [string]$Dir = $env:PXL_INSTALL_DIR,
  [switch]$NoSkills
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'kkollsga/agentsprite'
$Base = if ($env:PXL_RELEASE_BASE_URL) { $env:PXL_RELEASE_BASE_URL.TrimEnd('/') } else { "https://github.com/$Repo/releases" }
if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA 'pxl\bin' }
if ($env:PXL_INSTALL_SKILLS -eq '0') { $NoSkills = $true }

function Fail($msg) { throw "install.ps1: $msg" }

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') { Fail "unsupported CPU architecture $arch (the Windows release is x86_64; build from source with cargo build --release)" }
$Target = 'x86_64-pc-windows-msvc'

if (-not $Version) {
  if ($Base -like 'https://github.com/*') {
    $Version = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'pxl-installer' }).tag_name
    if (-not $Version) { Fail 'cannot resolve the latest release' }
  } else {
    $Version = (Invoke-RestMethod -Uri "$Base/LATEST").ToString().Trim()
  }
}
if (-not $Version.StartsWith('v')) { $Version = "v$Version" }

$Asset = "pxl-$Version-$Target.zip"
$Url = "$Base/download/$Version/$Asset"
Write-Host "pxl installer: $Version for $Target"

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("pxl-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
  Write-Host "downloading $Url"
  Invoke-WebRequest -Uri $Url -OutFile (Join-Path $Tmp $Asset)
  Invoke-WebRequest -Uri "$Url.sha256" -OutFile (Join-Path $Tmp "$Asset.sha256")

  $want = ((Get-Content (Join-Path $Tmp "$Asset.sha256") -TotalCount 1) -split '\s+')[0].ToLower()
  $have = (Get-FileHash (Join-Path $Tmp $Asset) -Algorithm SHA256).Hash.ToLower()
  if (-not $want) { Fail 'empty checksum file' }
  if ($want -ne $have) { Fail "checksum mismatch for ${Asset}: expected $want, got $have; nothing was installed" }
  Write-Host "sha256 verified: $have"

  $x = Join-Path $Tmp 'x'
  Expand-Archive -Path (Join-Path $Tmp $Asset) -DestinationPath $x
  $exe = Join-Path $x 'pxl.exe'
  if (-not (Test-Path $exe)) { Fail 'the archive has no pxl.exe' }

  New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  $dest = Join-Path $Dir 'pxl.exe'
  Copy-Item $exe "$dest.new" -Force
  Move-Item "$dest.new" $dest -Force
  Write-Host "installed: $dest ($(& $dest --version))"
} finally {
  Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

$onPath = ($env:PATH -split ';') -contains $Dir
if (-not $onPath) {
  $userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
  if (-not (($userPath -split ';') -contains $Dir)) {
    [Environment]::SetEnvironmentVariable('PATH', ($userPath.TrimEnd(';') + ";$Dir"), 'User')
    Write-Host "added $Dir to your user PATH; open a new terminal to use pxl"
  }
  $env:PATH = "$env:PATH;$Dir"
}

if ($NoSkills) {
  Write-Host ''
  Write-Host 'skills not installed (-NoSkills). Install them later with: pxl skill install'
} else {
  Write-Host ''
  Write-Host 'installing the agent skills for Claude Code and Codex (disable with -NoSkills):'
  & $dest skill install
  if ($LASTEXITCODE -ne 0) { Write-Host "skill install failed; run 'pxl skill install' yourself (an unmanaged skill folder is never replaced)" }
}

Write-Host ''
Write-Host 'next: pxl --version, pxl console list, pxl guide'
