param(
    [string]$Version = "latest",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Codexline\bin")
)

$ErrorActionPreference = "Stop"
$Repository = "YongshengWin/codex-cli-codexline"

$Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($Architecture) {
    "X64" { $Target = "x86_64-pc-windows-msvc" }
    default { throw "Unsupported Windows architecture: $Architecture. Use the source installation instructions." }
}

$Archive = "codexline-$Target.zip"
if ($Version -eq "latest") {
    $DownloadBase = "https://github.com/$Repository/releases/latest/download"
} else {
    $DownloadBase = "https://github.com/$Repository/releases/download/$Version"
}

$TemporaryDir = Join-Path ([System.IO.Path]::GetTempPath()) ("codexline-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $TemporaryDir | Out-Null

try {
    $ArchivePath = Join-Path $TemporaryDir $Archive
    $ChecksumPath = "$ArchivePath.sha256"
    Write-Host "Downloading Codexline for $Target..."
    Invoke-WebRequest -UseBasicParsing "$DownloadBase/$Archive" -OutFile $ArchivePath
    Invoke-WebRequest -UseBasicParsing "$DownloadBase/$Archive.sha256" -OutFile $ChecksumPath

    $ExpectedHash = ((Get-Content $ChecksumPath -Raw).Trim() -split '\s+')[0]
    $ActualHash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash
    if ($ActualHash -ine $ExpectedHash) {
        throw "SHA-256 verification failed for $Archive"
    }

    $ExtractedDir = Join-Path $TemporaryDir "extracted"
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractedDir
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Destination = Join-Path $InstallDir "codexline.exe"

    if (Test-Path $Destination) {
        $ExistingVersion = & $Destination --version 2>$null
        if ($LASTEXITCODE -ne 0 -or $ExistingVersion -notmatch '^codexline ') {
            throw "Refusing to replace unrelated file at $Destination"
        }
    }

    $Pending = "$Destination.new"
    $Backup = "$Destination.old"
    Copy-Item -Force (Join-Path $ExtractedDir "codexline.exe") $Pending
    if (Test-Path $Destination) {
        Move-Item -Force $Destination $Backup
    }
    try {
        Move-Item $Pending $Destination
        Remove-Item -Force -ErrorAction SilentlyContinue $Backup
    } catch {
        if (Test-Path $Backup) {
            Move-Item -Force $Backup $Destination
        }
        throw
    }

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = @($UserPath -split ';' | Where-Object { $_ })
    if ($PathEntries -notcontains $InstallDir) {
        $UpdatedPath = (@($PathEntries) + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $UpdatedPath, "User")
        Write-Host "Added $InstallDir to your user PATH. Open a new terminal before continuing."
    }

    $InstalledVersion = & $Destination --version
    Write-Host "Installed $InstalledVersion to $Destination"
    Write-Host "Next: codexline config; codexline doctor; codexline"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $TemporaryDir
}
