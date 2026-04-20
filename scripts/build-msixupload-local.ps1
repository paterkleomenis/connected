param(
    [Parameter(Mandatory = $false)]
    [ValidateSet("debug", "release")]
    [string]$Profile = "release",

    [Parameter(Mandatory = $false)]
    [switch]$SkipBuild,

    [Parameter(Mandatory = $false)]
    [switch]$X64Only,

    [Parameter(Mandatory = $false)]
    [string]$OutputDir = "target\msix"
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

function Test-Arm64ToolchainAvailable {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (!(Test-Path $vswhere)) {
        return $false
    }

    $vsPath = & $vswhere -latest -products * -property installationPath
    if (-not $vsPath) {
        return $false
    }

    $msvcToolsRoot = Join-Path $vsPath "VC\Tools\MSVC"
    if (!(Test-Path $msvcToolsRoot)) {
        return $false
    }

    $msvcVersionDir = Get-ChildItem -Path $msvcToolsRoot -Directory |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $msvcVersionDir) {
        return $false
    }

    $linker = Join-Path $msvcVersionDir.FullName "bin\Hostx64\arm64\link.exe"
    return (Test-Path $linker)
}

$buildMsixScript = Join-Path $scriptDir "build-msix.ps1"
if (!(Test-Path $buildMsixScript)) {
    throw "Missing build script: $buildMsixScript"
}

if (-not $X64Only -and -not (Test-Arm64ToolchainAvailable)) {
    Write-Host "ARM64 toolchain not found locally. Falling back to x64-only packaging." -ForegroundColor Yellow
    $X64Only = $true
}

if (-not $SkipBuild) {
    Write-Host "Building x64 MSIX ($Profile)..."
    & $buildMsixScript -Arch x64 -Profile $Profile
    if ($LASTEXITCODE -ne 0) {
        throw "x64 MSIX build failed with exit code $LASTEXITCODE"
    }

    if (-not $X64Only) {
        Write-Host "Building ARM64 MSIX ($Profile)..."
        & $buildMsixScript -Arch arm64 -Profile $Profile
        if ($LASTEXITCODE -ne 0) {
            throw "ARM64 MSIX build failed with exit code $LASTEXITCODE"
        }
    }
}

$x64 = Join-Path $repoRoot "target\msix\x64\connected-desktop-windows-x64.msix"
$arm64 = Join-Path $repoRoot "target\msix\arm64\connected-desktop-windows-arm64.msix"

if (!(Test-Path $x64)) {
    throw "Missing x64 MSIX: $x64"
}
if (-not $X64Only -and !(Test-Path $arm64)) {
    throw "Missing ARM64 MSIX: $arm64"
}

$outRoot = Join-Path $repoRoot $OutputDir
New-Item -ItemType Directory -Path $outRoot -Force | Out-Null

$bundleInput = Join-Path $outRoot "bundle-input"
if (Test-Path $bundleInput) {
    Remove-Item $bundleInput -Recurse -Force
}
New-Item -ItemType Directory -Path $bundleInput -Force | Out-Null

$x64Name = "connected-desktop-windows-x64.msix"
$arm64Name = "connected-desktop-windows-arm64.msix"
Copy-Item $x64 (Join-Path $bundleInput $x64Name) -Force
if (-not $X64Only) {
    Copy-Item $arm64 (Join-Path $bundleInput $arm64Name) -Force
}

$uploadDir = Join-Path $outRoot "upload"
if (Test-Path $uploadDir) {
    Remove-Item $uploadDir -Recurse -Force
}
New-Item -ItemType Directory -Path $uploadDir -Force | Out-Null

if ($X64Only) {
    $msixUploadPath = Join-Path $outRoot "connected-desktop-windows-x64.msixupload"
    $tempZipPath = Join-Path $outRoot "connected-desktop-windows-x64.zip"
    if (Test-Path $msixUploadPath) {
        Remove-Item $msixUploadPath -Force
    }
    if (Test-Path $tempZipPath) {
        Remove-Item $tempZipPath -Force
    }

    Copy-Item $x64 (Join-Path $uploadDir $x64Name) -Force
    Compress-Archive -Path (Join-Path $uploadDir "*") -DestinationPath $tempZipPath -CompressionLevel Optimal
    Move-Item -Path $tempZipPath -Destination $msixUploadPath -Force

    Write-Host "x64-only MSIXUPLOAD created: $msixUploadPath"
    Write-Host "Note: ARM64 package was skipped."
    return
}

$sdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
if (!(Test-Path $sdkRoot)) {
    throw "Windows SDK tools not found at $sdkRoot"
}

$makeAppx = Get-ChildItem -Path $sdkRoot -Recurse -Filter makeappx.exe |
    Sort-Object FullName -Descending |
    Select-Object -First 1
if ($null -eq $makeAppx) {
    throw "makeappx.exe not found. Install Windows SDK App Certification tools."
}

$bundlePath = Join-Path $outRoot "connected-desktop-windows.msixbundle"
if (Test-Path $bundlePath) {
    Remove-Item $bundlePath -Force
}

Write-Host "Building MSIX bundle with $($makeAppx.FullName)"
& $makeAppx.FullName bundle /d $bundleInput /p $bundlePath /o
if ($LASTEXITCODE -ne 0) {
    throw "makeappx bundle failed with exit code $LASTEXITCODE"
}

Copy-Item $bundlePath (Join-Path $uploadDir "connected-desktop-windows.msixbundle") -Force

$msixUploadPath = Join-Path $outRoot "connected-desktop-windows.msixupload"
$tempZipPath = Join-Path $outRoot "connected-desktop-windows.zip"
if (Test-Path $msixUploadPath) {
    Remove-Item $msixUploadPath -Force
}
if (Test-Path $tempZipPath) {
    Remove-Item $tempZipPath -Force
}

Compress-Archive -Path (Join-Path $uploadDir "*") -DestinationPath $tempZipPath -CompressionLevel Optimal
Move-Item -Path $tempZipPath -Destination $msixUploadPath -Force

Write-Host "MSIX bundle created: $bundlePath"
Write-Host "MSIXUPLOAD created: $msixUploadPath"
