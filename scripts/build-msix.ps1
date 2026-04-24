param(
    [Parameter(Mandatory = $false)]
    [ValidateSet("x64", "arm64")]
    [string]$Arch = "x64",

    [Parameter(Mandatory = $false)]
    [ValidateSet("debug", "release")]
    [string]$Profile = "release",

    [Parameter(Mandatory = $false)]
    [ValidateRange(0, 65535)]
    [int]$PackageBuild = -1
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

$target = if ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$buildDirName = if ($Profile -eq "release") { "release" } else { "debug" }

function Configure-Arm64WindowsToolchain {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (!(Test-Path $vswhere)) {
        throw "vswhere.exe not found at $vswhere. Install Visual Studio Build Tools."
    }

    $vsPath = & $vswhere -latest -products * -property installationPath
    if (-not $vsPath) {
        throw "Visual Studio installation not found. Install Visual Studio Build Tools with C++ workloads."
    }

    $msvcToolsRoot = Join-Path $vsPath "VC\Tools\MSVC"
    if (!(Test-Path $msvcToolsRoot)) {
        throw "MSVC tools not found at $msvcToolsRoot. Install C++ build tools."
    }

    $msvcVersionDir = Get-ChildItem -Path $msvcToolsRoot -Directory |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $msvcVersionDir) {
        throw "Could not locate an MSVC version under $msvcToolsRoot"
    }

    $linker = Join-Path $msvcVersionDir.FullName "bin\Hostx64\arm64\link.exe"
    if (!(Test-Path $linker)) {
        throw "ARM64 linker not found at $linker. Install ARM64 MSVC tools component."
    }

    $clangDir = Join-Path $vsPath "VC\Tools\Llvm\x64\bin"
    $clang = Join-Path $clangDir "clang-cl.exe"
    $llvmLib = Join-Path $clangDir "llvm-lib.exe"

    $msvcBinDir = Join-Path $msvcVersionDir.FullName "bin\Hostx64\arm64"
    $clMsvc = Join-Path $msvcBinDir "cl.exe"
    $libMsvc = Join-Path $msvcBinDir "lib.exe"

    if ((Test-Path $clang) -and (Test-Path $llvmLib)) {
        $env:PATH = "$clangDir;$msvcBinDir;$env:PATH"
        $env:CC_aarch64_pc_windows_msvc = $clang
        $env:CXX_aarch64_pc_windows_msvc = $clang
        $env:AR_aarch64_pc_windows_msvc = $llvmLib
        Write-Host "Configured ARM64 toolchain: clang-cl + link.exe"
    }
    elseif ((Test-Path $clMsvc) -and (Test-Path $libMsvc)) {
        $env:PATH = "$msvcBinDir;$env:PATH"
        $env:CC_aarch64_pc_windows_msvc = $clMsvc
        $env:CXX_aarch64_pc_windows_msvc = $clMsvc
        $env:AR_aarch64_pc_windows_msvc = $libMsvc
        Write-Host "Configured ARM64 toolchain: cl.exe + link.exe"
    }
    else {
        throw "ARM64 C/C++ compiler not found. Install ARM64 MSVC tools (and optionally Microsoft.VisualStudio.Component.VC.Llvm.Clang)."
    }

    $env:CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER = $linker
}

$cargoVersionLine = (Get-Content "$repoRoot\Cargo.toml" | Select-String -Pattern '^\s*version\s*=\s*"([^"]+)"' -AllMatches | Select-Object -Last 1)
if ($null -eq $cargoVersionLine) {
    throw "Could not determine workspace version from Cargo.toml"
}
$rawVersion = $cargoVersionLine.Matches[0].Groups[1].Value

function Convert-ToMsixVersion([string]$version, [int]$buildOverride) {
    if ($version -match '^(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?$') {
        $major = $matches[1]
        $minor = $matches[2]
        $patch = $matches[3]

        # Microsoft Store requires the revision segment (4th) to be 0.
        # To keep package full names unique for local re-submissions, use
        # PackageBuild as the 3rd segment when provided.
        $build = if ($buildOverride -ge 0) { "$buildOverride" } else { $patch }
        return "$major.$minor.$build.0"
    }
    throw "Unsupported version format '$version'. Expected semantic version like 2.9.4"
}

$packageVersion = Convert-ToMsixVersion $rawVersion $PackageBuild

if ($Arch -eq "arm64") {
    Configure-Arm64WindowsToolchain
}

Write-Host "Building connected-desktop ($target, $buildDirName)..."
if ($Profile -eq "release") {
    cargo build -p connected-desktop --target $target --release
} else {
    cargo build -p connected-desktop --target $target
}

$exePath = Join-Path $repoRoot "target\$target\$buildDirName\connected-desktop.exe"
if (!(Test-Path $exePath)) {
    throw "Built executable not found at $exePath"
}

$dllPath = Join-Path $repoRoot "target\$target\$buildDirName\WebView2Loader.dll"
if (!(Test-Path $dllPath)) {
    $candidate = Get-ChildItem -Path (Join-Path $repoRoot "target\$target\$buildDirName\build") -Recurse -Filter "WebView2Loader.dll" |
        Where-Object { $_.FullName -match '\\out\\(x64|arm64)\\WebView2Loader\.dll$' } |
        Select-Object -First 1
    if ($null -ne $candidate) {
        Copy-Item $candidate.FullName $dllPath -Force
    }
}
if (!(Test-Path $dllPath)) {
    throw "WebView2Loader.dll not found for target '$target'"
}

$outRoot = Join-Path $repoRoot "target\msix\$Arch"
$layoutDir = Join-Path $outRoot "layout"
$assetsDir = Join-Path $layoutDir "Assets"
$manifestTemplate = Join-Path $repoRoot "packaging\windows\AppxManifest.xml"

if (Test-Path $layoutDir) {
    Remove-Item $layoutDir -Recurse -Force
}
New-Item -ItemType Directory -Path $assetsDir -Force | Out-Null

Copy-Item $exePath (Join-Path $layoutDir "connected-desktop.exe") -Force
Copy-Item $dllPath (Join-Path $layoutDir "WebView2Loader.dll") -Force

$icon = Join-Path $repoRoot "desktop\assets\logo.png"
if (!(Test-Path $icon)) {
    throw "Logo not found at $icon"
}

Copy-Item $icon (Join-Path $assetsDir "StoreLogo.png") -Force
Copy-Item $icon (Join-Path $assetsDir "Square150x150Logo.png") -Force
Copy-Item $icon (Join-Path $assetsDir "Square44x44Logo.png") -Force

$archManifest = if ($Arch -eq "arm64") { "arm64" } else { "x64" }
$manifestContent = Get-Content $manifestTemplate -Raw
$manifestContent = $manifestContent.Replace("__VERSION__", $packageVersion).Replace("__ARCH__", $archManifest)
$manifestPath = Join-Path $layoutDir "AppxManifest.xml"
Set-Content -Path $manifestPath -Value $manifestContent -Encoding UTF8

$msixName = "connected-desktop-windows-$Arch.msix"
$msixPath = Join-Path $outRoot $msixName
if (Test-Path $msixPath) {
    Remove-Item $msixPath -Force
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

Write-Host "Packing MSIX with $($makeAppx.FullName)"
& $makeAppx.FullName pack /d $layoutDir /p $msixPath /o
if ($LASTEXITCODE -ne 0) {
    throw "makeappx failed with exit code $LASTEXITCODE"
}

Write-Host "MSIX created: $msixPath"
Write-Host "Note: MSIX is unsigned. Sign with signtool before distribution if needed."
