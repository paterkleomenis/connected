param(
    [Parameter(Mandatory = $false)]
    [ValidateSet("x64", "arm64")]
    [string]$Arch = "x64",

    [Parameter(Mandatory = $false)]
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

$target = if ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$buildDirName = if ($Profile -eq "release") { "release" } else { "debug" }

$cargoVersionLine = (Get-Content "$repoRoot\Cargo.toml" | Select-String -Pattern '^\s*version\s*=\s*"([^"]+)"' -AllMatches | Select-Object -Last 1)
if ($null -eq $cargoVersionLine) {
    throw "Could not determine workspace version from Cargo.toml"
}
$rawVersion = $cargoVersionLine.Matches[0].Groups[1].Value

function Convert-ToMsixVersion([string]$version) {
    if ($version -match '^(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?$') {
        $major = $matches[1]
        $minor = $matches[2]
        $patch = $matches[3]
        $build = if ($matches[4]) { $matches[4] } else { "0" }
        return "$major.$minor.$patch.$build"
    }
    throw "Unsupported version format '$version'. Expected semantic version like 2.9.4"
}

$packageVersion = Convert-ToMsixVersion $rawVersion

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
