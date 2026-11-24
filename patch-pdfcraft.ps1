<# Copy a local Windows release build into the installed PdfCraft directory.
   Build it with: cargo build --release --locked -p pdfcraft
   Use -WhatIf to print the destination without replacing the file. #>
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$BinaryPath = (Join-Path $PSScriptRoot 'target\release\pdfcraft.exe'),
    [string]$InstallPath
)

$ErrorActionPreference = 'Stop'

function Find-PdfCraft {
    $appPaths = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe'
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe'
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe'
    )
    foreach ($key in $appPaths) {
        if (Test-Path -LiteralPath $key) {
            $entry = Get-Item -LiteralPath $key
            $path = [Environment]::ExpandEnvironmentVariables([string]$entry.GetValue('')).Trim('"')
            if ($path -and (Test-Path -LiteralPath $path -PathType Leaf)) {
                return $path
            }
        }
    }
    foreach ($root in @($env:LOCALAPPDATA, $env:ProgramW6432, $env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $root) { continue }
        foreach ($relativePath in @('PdfCraft\pdfcraft.exe', 'Programs\PdfCraft\pdfcraft.exe')) {
            $path = Join-Path $root $relativePath
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                return $path
            }
        }
    }
    $runningPaths = @(Get-Process -Name pdfcraft -ErrorAction SilentlyContinue |
        ForEach-Object Path | Where-Object { $_ } | Sort-Object -Unique)
    if ($runningPaths.Count -eq 1) {
        return $runningPaths[0]
    }
    throw 'Could not identify a single PdfCraft installation. Pass -InstallPath with the full path to pdfcraft.exe.'
}

if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
    throw "Build the release binary first with cargo build --release --locked -p pdfcraft, or pass -BinaryPath. Missing: $BinaryPath"
}
$source = (Resolve-Path -LiteralPath $BinaryPath).ProviderPath
if (-not $InstallPath) { $InstallPath = Find-PdfCraft }
$destination = (Resolve-Path -LiteralPath $InstallPath).ProviderPath
if ($source -eq $destination -or [IO.Path]::GetFileName($destination) -ne 'pdfcraft.exe') {
    throw "Expected a separate installed pdfcraft.exe. Found: $destination"
}
foreach ($executable in @($source, $destination)) {
    if ([Diagnostics.FileVersionInfo]::GetVersionInfo($executable).InternalName -ne 'pdfcraft') {
        throw "Expected the PdfCraft executable's Windows version metadata: $executable"
    }
}

$hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -eq $hash) {
    Write-Output "Already installed: $destination"
    return
}
if (-not $PSCmdlet.ShouldProcess($destination, "Install $source and back up the previous executable")) { return }

$suffix = [Guid]::NewGuid().ToString('N')
$stage = "$destination.$suffix.tmp"
$backup = "$destination.$(Get-Date -Format yyyyMMdd-HHmmss).$suffix.bak"
try {
    try {
        [IO.File]::Copy($source, $stage)
    } catch {
        if ($_.Exception.GetBaseException() -isnot [UnauthorizedAccessException]) { throw }
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = [Security.Principal.WindowsPrincipal]::new($identity)
        if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw }
        # Resolve both paths before elevation so the child patches this exact installation.
        $arguments = @($PSCommandPath, $source, $destination) | ForEach-Object { "'" + $_.Replace("'", "''") + "'" }
        $command = "& $($arguments[0]) -BinaryPath $($arguments[1]) -InstallPath $($arguments[2])"
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
        $launch = @{
            FilePath = (Get-Process -Id $PID).Path
            Verb = 'RunAs'
            WindowStyle = 'Hidden'
            Wait = $true
            PassThru = $true
            ArgumentList = @('-NoProfile', '-EncodedCommand', $encoded)
        }
        $child = Start-Process @launch
        if ($child.ExitCode -ne 0 -or (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $hash) {
            throw 'The elevated replacement did not complete. Close PdfCraft and check the installer directory permissions.'
        }
        Write-Output "Installed and verified: $destination"
        return
    }

    if ((Get-FileHash -LiteralPath $stage -Algorithm SHA256).Hash -ne $hash) {
        throw 'The build changed during copying. Finish building before running the patch again.'
    }
    $running = @(Get-Process -Name pdfcraft -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $destination })
    foreach ($process in $running) {
        if (-not $process.HasExited -and -not $process.CloseMainWindow()) {
            throw "Close PdfCraft before patching. Process ID: $($process.Id)"
        }
    }
    foreach ($process in $running) {
        if (-not $process.WaitForExit(60000)) {
            throw "PdfCraft is still open. Respond to any save prompt, then rerun the script. Process ID: $($process.Id)"
        }
    }
    [IO.File]::Replace($stage, $destination, $backup)
    if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $hash) {
        throw "Verification failed. Previous executable: $backup"
    }
    Write-Output "Installed and verified: $destination"
    Write-Output "Previous executable: $backup"
} finally {
    if (Test-Path -LiteralPath $stage -PathType Leaf) {
        Remove-Item -LiteralPath $stage
    }
}
