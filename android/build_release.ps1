# Build script for Play Store release (PowerShell version)
# Usage: .\build_release.ps1

$ErrorActionPreference = "Stop"

Write-Host "🔨 Building Connected App for Play Store..." -ForegroundColor Cyan
Write-Host ""

# Load .env file if it exists (for signing configuration)
$ScriptDir = $PSScriptRoot
$EnvFile = Join-Path $ScriptDir ".env"

if (Test-Path $EnvFile) {
    Write-Host "🔑 Loading signing configuration from .env..." -ForegroundColor Yellow
    Get-Content $EnvFile | Where-Object { $_ -notmatch '^\s*#' -and $_ -match '\S' } | ForEach-Object {
        if ($_ -match '^\s*([^=]+?)\s*=\s*(.+?)\s*$') {
            $key = $matches[1].Trim()
            $value = $matches[2].Trim()
            [Environment]::SetEnvironmentVariable($key, $value, "Process")
            # Also set in script scope for immediate use
            Set-Item -Path "env:$key" -Value $value
        }
    }
    Write-Host "✅ .env loaded" -ForegroundColor Green
} else {
    Write-Host "⚠️  No .env file found at $EnvFile" -ForegroundColor Yellow
    Write-Host "   Signing with debug key (create .env from .env.example for release signing)" -ForegroundColor Yellow
}

# Set JAVA_HOME if not already set and Android Studio Java exists
if (-not $env:JAVA_HOME) {
    $AndroidStudioJava = "C:\Program Files\Android\Android Studio\jbr"
    if (Test-Path $AndroidStudioJava) {
        Write-Host "☕ Setting JAVA_HOME to Android Studio Java..." -ForegroundColor Yellow
        [Environment]::SetEnvironmentVariable("JAVA_HOME", $AndroidStudioJava, "Process")
        $env:JAVA_HOME = $AndroidStudioJava
    }
}

# Add Java to PATH if JAVA_HOME is set
if ($env:JAVA_HOME -and -not ($env:PATH -like "*$env:JAVA_HOME*")) {
    $env:PATH += ";$env:JAVA_HOME\bin"
}

Write-Host ""

# Check if we're in the right directory
if (-not (Test-Path "gradlew.bat")) {
    Write-Host "❌ Error: Run this script from the android/ directory" -ForegroundColor Red
    exit 1
}

# Check for Rust toolchain
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "❌ Error: Rust toolchain (cargo) not found" -ForegroundColor Red
    exit 1
}

# Check for NDK
if (-not $env:ANDROID_NDK_HOME -and -not (Test-Path "local.properties")) {
    Write-Host "⚠️  Warning: ANDROID_NDK_HOME not set and no local.properties found" -ForegroundColor Yellow
}

Write-Host "✅ Prerequisites check passed" -ForegroundColor Green
Write-Host ""

# Check signing configuration
if ($env:ANDROID_KEYSTORE_PASSWORD) {
    Write-Host "✅ Release signing configured" -ForegroundColor Green
} else {
    Write-Host "⚠️  No release signing configured — will use DEBUG key" -ForegroundColor Yellow
}
Write-Host ""

# Clean previous builds
Write-Host "🧹 Cleaning previous builds..." -ForegroundColor Cyan
.\gradlew.bat clean
Write-Host ""

# Build Rust library for release
Write-Host "🦀 Building Rust library for release..." -ForegroundColor Cyan
.\gradlew.bat :app:buildRustRelease
Write-Host ""

# Generate UniFFI bindings
Write-Host "🔗 Generating UniFFI bindings..." -ForegroundColor Cyan
.\gradlew.bat :app:generateBindingsRelease
Write-Host ""

# Compile release build
Write-Host "🚀 Compiling release build..." -ForegroundColor Cyan
.\gradlew.bat assembleRelease
Write-Host ""

# Check if build was successful
if (Test-Path "app\build\outputs\apk\release\app-release.apk") {
    Write-Host "✅ Release APK built successfully!" -ForegroundColor Green
    Write-Host "📁 Location: app\build\outputs\apk\release\app-release.apk" -ForegroundColor Cyan
    Write-Host ""

    # Show file size
    $ApkFile = Get-Item "app\build\outputs\apk\release\app-release.apk"
    $ApkSize = "{0:N2} MB" -f ($ApkFile.Length / 1MB)
    Write-Host "📦 APK Size: $ApkSize" -ForegroundColor Cyan
    Write-Host ""

    # Try to build AAB
    Write-Host "📱 Building Android App Bundle (AAB)..." -ForegroundColor Cyan
    .\gradlew.bat bundleRelease

    if (Test-Path "app\build\outputs\bundle\release\app-release.aab") {
        Write-Host "✅ Release AAB built successfully!" -ForegroundColor Green
        Write-Host "📁 Location: app\build\outputs\bundle\release\app-release.aab" -ForegroundColor Cyan

        $AabFile = Get-Item "app\build\outputs\bundle\release\app-release.aab"
        $AabSize = "{0:N2} MB" -f ($AabFile.Length / 1MB)
        Write-Host "📦 AAB Size: $AabSize" -ForegroundColor Cyan
    } else {
        Write-Host "❌ AAB build failed" -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "❌ Release build failed" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "🎉 Build complete!" -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:"
Write-Host "1. Test the APK on a device: adb install -r app\build\outputs\apk\release\app-release.apk"
Write-Host "2. Upload the AAB to Play Console"
Write-Host "3. See PLAY_STORE_GUIDE.md for detailed upload instructions"
