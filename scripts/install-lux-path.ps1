param(
    [string]$LuxHome = $env:LUX_HOME
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($LuxHome)) {
    if ([string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        throw "Cannot determine Lux home because USERPROFILE is not set. Pass -LuxHome <path>."
    }
    $LuxHome = Join-Path $env:USERPROFILE ".lux"
}

$bin = [System.IO.Path]::GetFullPath((Join-Path $LuxHome "bin")).TrimEnd('\')
New-Item -ItemType Directory -Force -Path $bin | Out-Null

function Normalize-PathEntry {
    param([string]$Entry)
    try {
        return [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($Entry)).TrimEnd('\')
    } catch {
        return $Entry.TrimEnd('\')
    }
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$userEntries = @()
if (-not [string]::IsNullOrWhiteSpace($userPath)) {
    $userEntries = $userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}

if (-not ($userEntries | Where-Object { (Normalize-PathEntry $_) -ieq $bin })) {
    $newUserPath = if ($userEntries.Count -eq 0) { $bin } else { ($userEntries + $bin) -join ';' }
    [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    Write-Host "Added $bin to the current user's PATH."
} else {
    Write-Host "Lux user PATH already contains $bin"
}

$processPath = $env:Path
$processEntries = @()
if (-not [string]::IsNullOrWhiteSpace($processPath)) {
    $processEntries = $processPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}

if (-not ($processEntries | Where-Object { (Normalize-PathEntry $_) -ieq $bin })) {
    $env:Path = if ($processEntries.Count -eq 0) { $bin } else { ($processEntries + $bin) -join ';' }
}

Write-Host "Lux bin: $bin"
Write-Host "Open a new terminal if the current session does not see luxc yet."
