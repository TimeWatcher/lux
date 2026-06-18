param(
    [string]$LuxHome = $env:LUX_HOME,
    [string]$LuxcPath = "",
    [string]$Version = "",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($LuxHome)) {
    if ([string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        throw "Cannot determine Lux home because USERPROFILE is not set. Pass -LuxHome <path>."
    }
    $LuxHome = Join-Path $env:USERPROFILE ".lux"
}

$luxHomeFull = [System.IO.Path]::GetFullPath($LuxHome).TrimEnd('\')
$bin = [System.IO.Path]::GetFullPath((Join-Path $luxHomeFull "bin")).TrimEnd('\')
$toolchains = [System.IO.Path]::GetFullPath((Join-Path $luxHomeFull "toolchains")).TrimEnd('\')
$defaultFile = Join-Path $luxHomeFull "default-toolchain"

New-Item -ItemType Directory -Force -Path $bin | Out-Null
New-Item -ItemType Directory -Force -Path $toolchains | Out-Null

function Normalize-PathEntry {
    param([string]$Entry)
    try {
        return [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($Entry)).TrimEnd('\')
    } catch {
        return $Entry.TrimEnd('\')
    }
}

function Get-NormalizedPath {
    param([string]$Path)
    return [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
}

function Test-SamePath {
    param(
        [string]$Left,
        [string]$Right
    )
    return (Get-NormalizedPath $Left) -ieq (Get-NormalizedPath $Right)
}

function Copy-LuxcExecutable {
    param(
        [string]$Source,
        [string]$Destination
    )

    if (Test-SamePath $Source $Destination) {
        return
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

function Write-Utf8NoBom {
    param(
        [string]$Path,
        [string]$Text
    )

    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}

function Add-LuxBinToPath {
    param([string]$BinPath)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $userEntries = @()
    if (-not [string]::IsNullOrWhiteSpace($userPath)) {
        $userEntries = $userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    if (-not ($userEntries | Where-Object { (Normalize-PathEntry $_) -ieq $BinPath })) {
        $newUserPath = if ($userEntries.Count -eq 0) { $BinPath } else { ($userEntries + $BinPath) -join ';' }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
        Write-Host "Added $BinPath to the current user's PATH."
    } else {
        Write-Host "Lux user PATH already contains $BinPath"
    }

    $processPath = $env:Path
    $processEntries = @()
    if (-not [string]::IsNullOrWhiteSpace($processPath)) {
        $processEntries = $processPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    if (-not ($processEntries | Where-Object { (Normalize-PathEntry $_) -ieq $BinPath })) {
        $env:Path = if ($processEntries.Count -eq 0) { $BinPath } else { ($processEntries + $BinPath) -join ';' }
    }
}

function Resolve-LuxcSource {
    param([string]$RequestedPath)

    if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
        $path = [System.IO.Path]::GetFullPath($RequestedPath)
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Lux compiler executable does not exist: $path"
        }
        return $path
    }

    $sameDirectoryLuxc = Join-Path $PSScriptRoot "luxc.exe"
    if (Test-Path -LiteralPath $sameDirectoryLuxc -PathType Leaf) {
        return [System.IO.Path]::GetFullPath($sameDirectoryLuxc)
    }

    return $null
}

function Get-LuxcVersion {
    param(
        [string]$SourceLuxc,
        [string]$ExplicitVersion
    )

    if (-not [string]::IsNullOrWhiteSpace($ExplicitVersion)) {
        return $ExplicitVersion.Trim()
    }

    $versionOutput = & $SourceLuxc --version 2>$null
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($versionOutput)) {
        $line = @($versionOutput)[0].Trim()
        if ($line -match '^luxc\s+(.+)$') {
            return $Matches[1].Trim()
        }
    }

    throw "Cannot determine luxc version from `$SourceLuxc --version`. Pass -Version <version>."
}

$sourceLuxc = Resolve-LuxcSource -RequestedPath $LuxcPath
if ($sourceLuxc) {
    $resolvedVersion = Get-LuxcVersion -SourceLuxc $sourceLuxc -ExplicitVersion $Version
    if ($resolvedVersion -match '[\\/]') {
        throw "Invalid Lux compiler version: $resolvedVersion"
    }

    $versionDir = Join-Path $toolchains $resolvedVersion
    $versionLuxc = Join-Path $versionDir "luxc.exe"
    $binLuxc = Join-Path $bin "luxc.exe"

    New-Item -ItemType Directory -Force -Path $versionDir | Out-Null
    Copy-LuxcExecutable -Source $sourceLuxc -Destination $versionLuxc
    Copy-LuxcExecutable -Source $sourceLuxc -Destination $binLuxc
    Write-Utf8NoBom -Path $defaultFile -Text ($resolvedVersion + "`n")

    Write-Host "Installed luxc $resolvedVersion"
    Write-Host "Toolchain: $versionLuxc"
    Write-Host "Entrypoint: $binLuxc"
} else {
    Write-Host "No luxc.exe found beside this script. PATH will be updated only."
    Write-Host "Pass -LuxcPath <path-to-luxc.exe> to install a compiler executable."
}

if (-not $NoPath) {
    Add-LuxBinToPath -BinPath $bin
}

Write-Host "Lux home: $luxHomeFull"
Write-Host "Lux bin: $bin"
Write-Host "Open a new terminal if the current session does not see luxc yet."
