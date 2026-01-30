# Justfile - Task runner for Connected
# Install with: cargo install just
# Usage: just <recipe>

set shell := ["bash", "-c"]
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


# Pre-commit check (fast - runs on every commit)
[doc("Pre-commit checks (fast)")]
pre-commit:
    cargo fmt -- --check
    cargo clippy --workspace -- -D warnings
    typos
    @echo "✅ Pre-commit checks passed"

# Pre-push check (slower - runs before push)
[doc("Pre-push checks (includes tests)")]
pre-push:
    just pre-commit
    cargo test --workspace
    @echo "✅ Pre-push checks passed"

# Full CI simulation (slowest)
[doc("Full CI simulation (slow)")]
ci:
    just pre-push
    cargo build --workspace --release
    cargo audit
    @./scripts/check-versions.sh
    @echo "✅ All CI checks passed"

# Clean build artifacts
[doc("Clean build artifacts")]
clean:
    cargo clean
    rm -rf target/
    @echo "✅ Clean complete"

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

# Install pre-commit hooks
[doc("Install pre-commit hooks")]
install-hooks:
    pre-commit install
    pre-commit install --hook-type commit-msg
    @echo "✅ Hooks installed"

# Run pre-commit on all files
[doc("Run hooks on all files")]
run-hooks:
    pre-commit run --all-files

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
    cd android && ./gradlew assembleDebug

[doc("Build Android release APK")]
build-android-release:
    cd android && ./gradlew assembleRelease

[doc("Install Android debug APK to connected device")]
install-android:
    cd android && ./gradlew installDebug

# Release preparation
[doc("Prepare for release (version bump, changelog)")]
prepare-release VERSION:
    echo "Preparing release {{VERSION}}..."
    git cliff --bump --tag {{VERSION}} --output CHANGELOG.md
    @echo "✅ Review CHANGELOG.md and Cargo.toml changes, then commit"

# Docker-based development (if needed later)
[doc("Run commands in Docker (for consistent environment)")]
docker-shell:
    docker run --rm -it -v $(pwd):/workspace -w /workspace rust:latest bash
