#!/bin/bash
# Build script for Play Store release
# Usage: ./build_release.sh

set -e

echo "🔨 Building Connected App for Play Store..."
echo ""

# Load .env file if it exists (for signing configuration)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/.env"

if [ -f "$ENV_FILE" ]; then
    echo "🔑 Loading signing configuration from .env..."
    # Export variables from .env (skip comments and empty lines)
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
    echo "✅ .env loaded"
else
    echo "⚠️  No .env file found at $ENV_FILE"
    echo "   Signing with debug key (create .env from .env.example for release signing)"
fi

echo ""

# Check if we're in the right directory
if [ ! -f "gradlew" ]; then
    echo "❌ Error: Run this script from the android/ directory"
    exit 1
fi

# Check for Rust toolchain
if ! command -v cargo &> /dev/null; then
    echo "❌ Error: Rust toolchain (cargo) not found"
    exit 1
fi

# Check for NDK
if [ -z "$ANDROID_NDK_HOME" ] && [ ! -f "local.properties" ]; then
    echo "⚠️  Warning: ANDROID_NDK_HOME not set and no local.properties found"
fi

# Check if JAVA_HOME is set, if not try to find it
if [ -z "$JAVA_HOME" ]; then
    echo "🔍 JAVA_HOME not set, searching for a JDK..."
    # Check common Android Studio locations
    for studio_path in "/opt/android-studio" "$HOME/android-studio" "/Applications/Android Studio.app/Contents"; do
        if [ -d "$studio_path/jbr" ]; then
            export JAVA_HOME="$studio_path/jbr"
            export PATH="$JAVA_HOME/bin:$PATH"
            echo "✅ Found JDK in $studio_path/jbr"
            break
        elif [ -d "$studio_path/jre" ]; then
            export JAVA_HOME="$studio_path/jre"
            export PATH="$JAVA_HOME/bin:$PATH"
            echo "✅ Found JDK in $studio_path/jre"
            break
        fi
    done
fi

# Check if Java is available now
if ! command -v java &> /dev/null; then
    echo "❌ Error: Java not found. Please install a JDK and set JAVA_HOME."
    exit 1
fi

echo "✅ Prerequisites check passed"
echo ""

# Check signing configuration
if [ -n "$ANDROID_KEYSTORE_PASSWORD" ]; then
    echo "✅ Release signing configured"
else
    echo "⚠️  No release signing configured — will use DEBUG key"
fi
echo ""

# Clean previous builds
echo "🧹 Cleaning previous builds..."
./gradlew clean
echo ""

# Build Rust library for release
echo "🦀 Building Rust library for release..."
./gradlew :app:buildRustRelease
echo ""

# Generate UniFFI bindings
echo "🔗 Generating UniFFI bindings..."
./gradlew :app:generateBindingsRelease
echo ""

# Compile release build
echo "🚀 Compiling release build..."
./gradlew assembleRelease
echo ""

# Check if build was successful
if [ -f "app/build/outputs/apk/release/app-release.apk" ]; then
    echo "✅ Release APK built successfully!"
    echo "📁 Location: app/build/outputs/apk/release/app-release.apk"
    echo ""

    # Show file size
    APK_SIZE=$(find app/build/outputs/apk/release -name "app-release.apk" -exec ls -lh {} \; | awk '{print $5}')
    echo "📦 APK Size: $APK_SIZE"
    echo ""

    # Try to build AAB
    echo "📱 Building Android App Bundle (AAB)..."
    ./gradlew bundleRelease

    if [ -f "app/build/outputs/bundle/release/app-release.aab" ]; then
        echo "✅ Release AAB built successfully!"
        echo "📁 Location: app/build/outputs/bundle/release/app-release.aab"

        AAB_SIZE=$(find app/build/outputs/bundle/release -name "app-release.aab" -exec ls -lh {} \; | awk '{print $5}')
        echo "📦 AAB Size: $AAB_SIZE"
    else
        echo "❌ AAB build failed"
        exit 1
    fi
else
    echo "❌ Release build failed"
    exit 1
fi

echo ""
echo "🎉 Build complete!"
echo ""
echo "Next steps:"
echo "1. Test the APK on a device: adb install -r app/build/outputs/apk/release/app-release.apk"
echo "2. Upload the AAB to Play Console"
echo "3. See PLAY_STORE_GUIDE.md for detailed upload instructions"
