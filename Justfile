# Justfile - Task runner for Connected
# Install with: cargo install just
# Usage: just <recipe>

# Cross-platform shell configuration
set shell := ["bash", "-cu"]
set windows-shell := ["cmd.exe", "/c"]

set dotenv-load := true

# Default recipe shows available commands
[private]
default:
    @just --list --unsorted

# Quick start for new contributors
[doc("Setup development environment")]
setup:
    @echo "🦀 Setting up Connected development environment..."
    ./scripts/setup-dev.sh

# Format all code
[doc("Format all code (Rust, TOML, Markdown)")]
fmt:
    cargo fmt
    taplo fmt
    @echo "✅ Formatting complete"

# Run all linters (fast)
[doc("Run all linters (fast - no build)")]
lint:
    cargo fmt -- --check
    cargo clippy --workspace -- -D warnings
    typos
    taplo fmt --check
    @echo "✅ All checks passed"

# Type-check only (faster than build)
[doc("Type-check all packages")]
check:
    cargo check --workspace

# Build debug version
[doc("Build debug version")]
build:
    cargo build --workspace

# Build release version
[doc("Build release version")]
build-release:
    cargo build --workspace --release

# Run tests
[doc("Run all tests")]
test:
    cargo test --workspace

# Run tests with coverage (requires cargo-tarpaulin)
[doc("Run tests with coverage report")]
test-coverage:
    cargo tarpaulin --workspace --out Html --out Stdout

# Security audit
[doc("Audit dependencies for security vulnerabilities")]
audit:
    cargo audit
    cargo deny check
    @echo "✅ Security audit passed"

# Pre-commit check
[doc("Pre-commit checks")]
pre-commit:
    python -m pre_commit run --all-files --show-diff-on-failure
    @echo "✅ Pre-commit checks passed"

# Full CI simulation (matches GitHub Actions CI)
[doc("Full CI simulation (slow)")]
ci:
    @echo "=== Pre-commit Hooks & Code Quality ==="
    just pre-commit
    @echo "=== Security Audit ==="
    just audit
    @echo "=== Build ==="
    just build
    @echo "=== Version Checks ==="
    just check-versions
    @echo "✅ All CI checks passed"

[doc("Check version consistency scripts")]
check-versions:
    {{ if os_family() == "windows" { "if exist scripts\\check-versions.sh (where bash >nul 2>nul && bash scripts/check-versions.sh || echo ⚠️ bash not found, skipping check-versions) else (echo ⚠️ scripts/check-versions.sh not found)" } else { "if [ -f scripts/check-versions.sh ]; then bash scripts/check-versions.sh; else echo \"⚠️ scripts/check-versions.sh not found\"; fi" } }}

# Windows-local CI helpers (PowerShell)
[doc("Run Windows CI steps locally (PowerShell)")]
ci-windows:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ci-windows.ps1

[doc("Build Windows MSI locally (PowerShell)")]
build-windows-msi:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ci-windows.ps1

[doc("Build Windows MSIX locally (PowerShell)")]
build-windows-msix ARCH="x64" PROFILE="release":
    cargo build --workspace --{{PROFILE}} --target {{ARCH}}-pc-windows-msvc
    just clean-webview-locales PROFILE={{PROFILE}}
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-msix.ps1 -Arch {{ARCH}} -Profile {{PROFILE}}

# Clean build artifacts
[doc("Clean build artifacts")]
clean:
    cargo clean
    {{ if os_family() == "windows" { "if exist target rd /s /q target" } else { "rm -rf target" } }}
    @echo ✅ Clean complete

# Update dependencies
[doc("Update all dependencies")]
update:
    cargo update
    cargo upgrade --workspace

# Check for outdated dependencies
[doc("Check for outdated dependencies")]
outdated:
    cargo outdated --workspace

# Build Android library
[doc("Build Android library (requires cargo-ndk)")]
build-android:
    cargo ndk -t aarch64-linux-android -t armv7-linux-androideabi -t x86_64-linux-android -t i686-linux-android -o android/app/src/main/jniLibs build --release

# iOS-specific commands
[doc("Generate Swift UniFFI bindings for iOS")]
ios-generate-bindings:
    ./scripts/ios/generate-bindings.sh

[doc("Build Rust iOS libraries and create xcframework")]
ios-build-rust:
    ./scripts/ios/build-rust-ios.sh

[doc("Generate iOS Xcode project from xcodegen spec")]
ios-generate-project:
    xcodegen generate --spec ios/project.yml

# Build macOS App and DMG
[doc("Build macOS .app bundle and DMG installer")]
build-macos:
    {{ if os_family() == "windows" { "echo ERROR: This command is only available on macOS/Linux && exit /b 2" } else { "echo \"ERROR: This command is only available on macOS/Linux\"; exit 2" } }}

# Install pre-commit hooks
[doc("Install pre-commit hooks")]
install-hooks:
    python -m pre_commit install
    python -m pre_commit install --hook-type commit-msg
    @echo "✅ Hooks installed"

# Run pre-commit on all files
[doc("Run hooks on all files")]
run-hooks:
    python -m pre_commit run --all-files

# Watch for changes and run checks
[doc("Watch for changes and run checks")]
watch:
    cargo watch -x check -x test

# Generate documentation
[doc("Generate and open documentation")]
docs:
    cargo doc --workspace --no-deps --open

# Benchmark (if benchmarks exist)
[doc("Run benchmarks")]
bench:
    cargo bench --workspace

# Desktop-specific commands
[doc("Run desktop application (debug)")]
run-desktop:
    cargo run -p connected-desktop

[doc("Run desktop application (release)")]
run-desktop-release:
    cargo run -p connected-desktop --release

# Android-specific commands
[doc("Build Android debug APK")]
build-android-apk:
    {{ if os_family() == "windows" { "cd /d android && gradlew.bat assembleDebug" } else { "cd android && ./gradlew assembleDebug" } }}

[doc("Build Android release APK")]
build-android-release:
    {{ if os_family() == "windows" { "cd /d android && gradlew.bat assembleRelease" } else { "cd android && ./gradlew assembleRelease" } }}

[doc("Install Android debug APK to connected device")]
install-android:
    {{ if os_family() == "windows" { "cd /d android && gradlew.bat installDebug" } else { "cd android && ./gradlew installDebug" } }}

[doc("Generate only the Android App Bundle (AAB)")]
build-android-bundle:
    {{ if os_family() == "windows" { "cd /d android && gradlew.bat bundleRelease" } else { "cd android && ./gradlew bundleRelease" } }}

[doc("Lint check Android release build")]
lint-android:
    {{ if os_family() == "windows" { "cd /d android && gradlew.bat lintRelease" } else { "cd android && ./gradlew lintRelease" } }}

# Android Play Store release
[doc("Build Android release for Play Store (APK + AAB)")]
build-android-playstore:
    {{ if os_family() == "windows" { "cd /d android && powershell -NoProfile -ExecutionPolicy Bypass -File build_release.ps1" } else { "cd android && bash ./build_release.sh" } }}

# Release preparation
[doc("Prepare for release (version bump, changelog)")]
prepare-release VERSION:
    echo "Preparing release {{VERSION}}..."
    git cliff --bump --tag {{VERSION}} --output CHANGELOG.md
    @echo "✅ Review CHANGELOG.md and Cargo.toml changes, then commit"

# Docker-based development (if needed later)
[doc("Run commands in Docker (for consistent environment)")]
docker-shell:
    docker run --rm -it -v %CD%:/workspace -w /workspace rust:latest cmd /c echo Docker shell ready

[doc("Cleans up WebView2 localized folders to prevent Microsoft Store ghost languages")]
clean-webview-locales PROFILE="release":
    @echo "Removing WebView2 ghost languages..."
    {{ if os_family() == "windows" { \
        "powershell -NoProfile -Command \"Get-ChildItem -Path target\\" + PROFILE + "\\ -Directory | Where-Object { $_.Name -match '^[a-z]{2}(-[A-Za-z]+)?$' } | Remove-Item -Recurse -Force\"" \
    } else { \
        "for dir in target/" + PROFILE + "/*/; do if [ -f \"$${dir}WebView2Loader.dll.mui\" ] || [ -f \"$${dir}Microsoft.Web.WebView2.Core.dll.mui\" ]; then echo \"Deleting $${dir}...\"; rm -rf \"$${dir}\"; fi; done" \
    } }}
