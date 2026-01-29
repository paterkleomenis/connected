#!/bin/bash
# Setup script for Connected development environment
# Run this after cloning the repository

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}🦀 Setting up Connected development environment...${NC}"

# Check Rust installation
if ! command -v rustc &> /dev/null; then
    echo -e "${RED}❌ Rust is not installed. Please install Rust first:${NC}"
    echo "   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

RUST_VERSION=$(rustc --version | cut -d' ' -f2)
echo -e "${GREEN}✓ Rust found: $RUST_VERSION${NC}"

# Install required Rust components
echo -e "${BLUE}📦 Installing Rust components...${NC}"
rustup component add rustfmt clippy 2>/dev/null || true

# Install cargo tools
echo -e "${BLUE}📦 Installing cargo tools...${NC}"

# Check if cargo-binstall is available for faster installs
if command -v cargo-binstall &> /dev/null; then
    echo -e "${GREEN}✓ cargo-binstall found, using for faster installs${NC}"
    BINSTALL="cargo binstall -y"
else
    BINSTALL="cargo install"
fi

# Install or update tools
TOOLS=(
    "cargo-deny"
    "cargo-audit"
    "typos-cli"
    "taplo-cli"
    "just"
)

for tool in "${TOOLS[@]}"; do
    if command -v "$tool" &> /dev/null; then
        echo -e "${GREEN}✓ $tool already installed${NC}"
    else
        echo -e "${YELLOW}⬇ Installing $tool...${NC}"
        $BINSTALL "$tool" || cargo install "$tool"
    fi
done

# Optional tools (don't fail if they don't install)
OPTIONAL_TOOLS=(
    "cargo-outdated"
    "cargo-watch"
    "cargo-tarpaulin"
    "cargo-machete"
    "cargo-udeps"
)

echo -e "${BLUE}📦 Installing optional tools...${NC}"
for tool in "${OPTIONAL_TOOLS[@]}"; do
    if command -v "$tool" &> /dev/null; then
        echo -e "${GREEN}✓ $tool already installed${NC}"
    else
        echo -e "${YELLOW}⬇ Installing $tool (optional)...${NC}"
        $BINSTALL "$tool" || cargo install "$tool" || echo -e "${YELLOW}⚠ $tool installation failed (optional)${NC}"
    fi
done

# Install pre-commit (handles various distros)
install_precommit() {
    echo -e "${BLUE}📦 Installing pre-commit...${NC}"

    # Check if already installed
    if command -v pre-commit &> /dev/null; then
        echo -e "${GREEN}✓ pre-commit already installed${NC}"
        return 0
    fi

    # Try pipx first (recommended for CLI tools on Arch/Fedora)
    if command -v pipx &> /dev/null; then
        echo -e "${YELLOW}⬇ Installing pre-commit via pipx...${NC}"
        pipx install pre-commit
        return 0
    fi

    # Try system package managers
    if command -v pacman &> /dev/null; then
        # Arch Linux
        echo -e "${YELLOW}⬇ Installing pre-commit via pacman...${NC}"
        sudo pacman -S --needed python-pre-commit || {
            echo -e "${YELLOW}⚠ Package not found in repos, trying pipx...${NC}"
            sudo pacman -S --needed python-pipx
            pipx install pre-commit
        }
        return 0
    fi

    if command -v apt-get &> /dev/null; then
        # Debian/Ubuntu
        echo -e "${YELLOW}⬇ Installing pre-commit via apt...${NC}"
        sudo apt-get update
        sudo apt-get install -y pre-commit || {
            echo -e "${YELLOW}⚠ Package not found, falling back to pip...${NC}"
            pip3 install --user pre-commit
        }
        return 0
    fi

    if command -v dnf &> /dev/null; then
        # Fedora
        echo -e "${YELLOW}⬇ Installing pre-commit via dnf...${NC}"
        sudo dnf install -y pre-commit || {
            echo -e "${YELLOW}⚠ Package not found, trying pipx...${NC}"
            sudo dnf install -y pipx
            pipx install pre-commit
        }
        return 0
    fi

    if command -v brew &> /dev/null; then
        # macOS/Homebrew
        echo -e "${YELLOW}⬇ Installing pre-commit via Homebrew...${NC}"
        brew install pre-commit
        return 0
    fi

    # Fallback: try pip with --user
    if command -v pip3 &> /dev/null; then
        echo -e "${YELLOW}⬇ Installing pre-commit via pip3 --user...${NC}"
        pip3 install --user pre-commit || {
            echo -e "${RED}❌ Failed to install pre-commit automatically.${NC}"
            echo ""
            echo -e "${YELLOW}Please install pre-commit manually:${NC}"
            echo "  - Arch:    sudo pacman -S python-pipx && pipx install pre-commit"
            echo "  - Fedora:  sudo dnf install pre-commit"
            echo "  - Ubuntu:  sudo apt install pre-commit"
            echo "  - Generic: pipx install pre-commit  (install pipx first)"
            return 1
        }
        return 0
    fi

    echo -e "${RED}❌ Could not find a way to install pre-commit.${NC}"
    return 1
}

install_precommit || {
    echo -e "${YELLOW}⚠ Pre-commit installation skipped. You can install it later.${NC}"
}

# Install pre-commit hooks (only if pre-commit is available)
if command -v pre-commit &> /dev/null; then
    echo -e "${BLUE}🔗 Installing pre-commit hooks...${NC}"
    pre-commit install
    pre-commit install --hook-type commit-msg
else
    echo -e "${YELLOW}⚠ pre-commit not available, skipping hook installation.${NC}"
    echo "  Install pre-commit and run: pre-commit install && pre-commit install --hook-type commit-msg"
fi

# Install Android tools if Android SDK is available
if [ -d "$ANDROID_SDK_ROOT" ] || [ -d "$ANDROID_HOME" ]; then
    echo -e "${BLUE}📱 Android SDK detected, installing cargo-ndk...${NC}"
    if ! command -v cargo-ndk &> /dev/null; then
        $BINSTALL cargo-ndk || cargo install cargo-ndk
    else
        echo -e "${GREEN}✓ cargo-ndk already installed${NC}"
    fi
else
    echo -e "${YELLOW}⚠ Android SDK not detected. Set ANDROID_SDK_ROOT or ANDROID_HOME to install Android tools.${NC}"
fi

# Check system dependencies for Linux
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo -e "${BLUE}🐧 Checking Linux system dependencies...${NC}"

    MISSING_DEPS=()

    # Check for required libraries
    if ! pkg-config --exists dbus-1 2>/dev/null; then
        MISSING_DEPS+=("libdbus-1-dev")
    fi

    if ! pkg-config --exists alsa 2>/dev/null; then
        MISSING_DEPS+=("libasound2-dev")
    fi

    if ! pkg-config --exists libudev 2>/dev/null; then
        MISSING_DEPS+=("libudev-dev")
    fi

    if [ ${#MISSING_DEPS[@]} -ne 0 ]; then
        echo -e "${YELLOW}⚠ Missing system dependencies. Install with:${NC}"
        if command -v apt-get &> /dev/null; then
            echo "   sudo apt-get install ${MISSING_DEPS[*]}"
        elif command -v pacman &> /dev/null; then
            echo "   sudo pacman -S dbus alsa-lib systemd-libs"
        elif command -v dnf &> /dev/null; then
            echo "   sudo dnf install dbus-devel alsa-lib-devel systemd-devel"
        else
            echo "   (install: ${MISSING_DEPS[*]})"
        fi
    else
        echo -e "${GREEN}✓ All system dependencies found${NC}"
    fi
fi

# Verify installation
echo -e "${BLUE}🧪 Verifying installation...${NC}"
cargo fmt -- --help > /dev/null 2>&1 && echo -e "${GREEN}✓ cargo fmt${NC}" || echo -e "${YELLOW}⚠ cargo fmt check failed${NC}"
cargo clippy -- --help > /dev/null 2>&1 && echo -e "${GREEN}✓ cargo clippy${NC}" || echo -e "${YELLOW}⚠ cargo clippy check failed${NC}"
command -v cargo-deny &> /dev/null && echo -e "${GREEN}✓ cargo deny${NC}" || echo -e "${YELLOW}⚠ cargo deny not found${NC}"

# First run of pre-commit on all files (optional, can be slow)
if command -v pre-commit &> /dev/null; then
    read -p "Run pre-commit on all files now? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${BLUE}🏃 Running pre-commit on all files...${NC}"
        pre-commit run --all-files || echo -e "${YELLOW}⚠ Some checks failed, but setup is complete${NC}"
    fi
else
    echo ""
    echo -e "${YELLOW}⚠ pre-commit not installed. Without it, you'll need to run checks manually:${NC}"
    echo "   just lint"
    echo "   just test"
fi

echo ""
echo -e "${GREEN}✅ Setup complete!${NC}"
echo ""
echo -e "${BLUE}Available commands:${NC}"
echo "  just          - Show all available tasks"
echo "  just fmt      - Format all code"
echo "  just lint     - Run all linters"
echo "  just test     - Run tests"
echo "  just ci       - Full CI simulation"
echo ""

if command -v pre-commit &> /dev/null; then
    echo -e "${BLUE}Pre-commit hooks are now active.${NC}"
    echo "  - They will run automatically on each commit"
    echo "  - Use 'git commit --no-verify' to skip (not recommended)"
else
    echo -e "${YELLOW}Pre-commit not installed.${NC}"
    echo "  Run checks manually with: just ci"
    echo "  Or install pre-commit: pipx install pre-commit && pre-commit install"
fi

echo ""
echo -e "${GREEN}Happy coding! 🚀${NC}"
