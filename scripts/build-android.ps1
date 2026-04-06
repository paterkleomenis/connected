# Cross-platform Android build script for Just
# This script detects the OS and runs the appropriate build commands

$ErrorActionPreference = "Stop"

if ($env:OS -eq "Windows_NT") {
    # Windows - use gradlew.bat
    Set-Location android

    Write-Host "🧹 Cleaning previous builds..." -ForegroundColor Cyan
    .\gradlew.bat clean

    Write-Host "🦀 Building Rust library for release..." -ForegroundColor Cyan
    .\gradlew.bat :app:buildRustRelease

    Write-Host "🔗 Generating UniFFI bindings..." -ForegroundColor Cyan
    .\gradlew.bat :app:generateBindingsRelease

    Write-Host "🚀 Compiling release build..." -ForegroundColor Cyan
    .\gradlew.bat assembleRelease

    Write-Host "📱 Building Android App Bundle (AAB)..." -ForegroundColor Cyan
    .\gradlew.bat bundleRelease

    Write-Host "✅ Build complete!" -ForegroundColor Green
} else {
    # Unix - use bash script
    cd android && ./build_release.sh
}
