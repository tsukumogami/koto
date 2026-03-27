#!/bin/bash
set -euo pipefail

# koto installer
# Downloads and installs a koto release (latest by default, or pinned with --version)

# Parse arguments
MODIFY_PATH=true
REQUESTED_VERSION=""
for arg in "$@"; do
    case "$arg" in
        --no-modify-path)
            MODIFY_PATH=false
            ;;
        --version=*)
            REQUESTED_VERSION="${arg#--version=}"
            ;;
    esac
done

REPO="tsukumogami/koto"
API_URL="https://api.github.com/repos/tsukumogami/koto/releases/latest"
# Default install: ~/.koto/bin/koto, ~/.koto/env
INSTALL_DIR="${KOTO_INSTALL_DIR:-$HOME/.koto}"
BIN_DIR="$INSTALL_DIR/bin"
ENV_FILE="$INSTALL_DIR/env"

# Detect OS
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$OS" in
    linux|darwin) ;;
    *)
        echo "Unsupported OS: $OS" >&2
        exit 1
        ;;
esac

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

echo "Detected platform: ${OS}-${ARCH}"

# Resolve version
if [ -n "$REQUESTED_VERSION" ]; then
    # Use the pinned version (add v prefix if missing)
    case "$REQUESTED_VERSION" in
        v*) VERSION="$REQUESTED_VERSION" ;;
        *)  VERSION="v${REQUESTED_VERSION}" ;;
    esac
    echo "Installing koto ${VERSION} (pinned)"
else
    # Fetch latest release
    echo "Fetching latest release..."
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        VERSION=$(curl -fsSL -H "Authorization: token $GITHUB_TOKEN" "$API_URL" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')
    else
        VERSION=$(curl -fsSL "$API_URL" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')
    fi

    if [ -z "$VERSION" ]; then
        echo "Failed to determine latest version" >&2
        exit 1
    fi
    echo "Installing koto ${VERSION}"
fi

# Binary naming matches GoReleaser convention: koto-{os}-{arch}
BINARY_NAME="koto-${OS}-${ARCH}"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY_NAME}"
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${VERSION}/checksums.txt"

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "Downloading ${BINARY_NAME}..."
curl -fsSL -o "$TEMP_DIR/koto" "$DOWNLOAD_URL"
curl -fsSL -o "$TEMP_DIR/checksums.txt" "$CHECKSUM_URL"

# Verify checksum
echo "Verifying checksum..."
cd "$TEMP_DIR"
EXPECTED_CHECKSUM=$(grep "${BINARY_NAME}$" checksums.txt | awk '{print $1}')
if [ -z "$EXPECTED_CHECKSUM" ]; then
    echo "Error: Could not find checksum for ${BINARY_NAME}" >&2
    exit 1
fi

if command -v sha256sum &>/dev/null; then
    echo "${EXPECTED_CHECKSUM}  koto" | sha256sum -c - >/dev/null
elif command -v shasum &>/dev/null; then
    echo "${EXPECTED_CHECKSUM}  koto" | shasum -a 256 -c - >/dev/null
else
    echo "Warning: Could not verify checksum (sha256sum/shasum not found)" >&2
fi

# Install
echo "Installing to ${BIN_DIR}..."
mkdir -p "$BIN_DIR"
chmod +x "$TEMP_DIR/koto"
mv "$TEMP_DIR/koto" "$BIN_DIR/koto"

echo ""
echo "koto ${VERSION} installed successfully!"
echo ""

# Create env file with PATH export
cat > "$ENV_FILE" << 'ENVEOF'
# koto shell configuration
export PATH="${KOTO_HOME:-$HOME/.koto}/bin:$PATH"
ENVEOF

# Configure shell if requested
if [ "$MODIFY_PATH" = true ]; then
    # Determine shell config file based on $SHELL
    SHELL_NAME=$(basename "$SHELL")

    # Helper function to add source line to a config file (idempotent)
    add_to_config() {
        local config_file="$1"
        local source_line=". \"$ENV_FILE\""

        if [ -f "$config_file" ] && grep -qF "$ENV_FILE" "$config_file" 2>/dev/null; then
            echo "  Already configured: $config_file"
            return 0
        fi

        # Append source line
        {
            echo ""
            echo "# koto"
            echo "$source_line"
        } >> "$config_file"
        echo "  Configured: $config_file"
    }

    case "$SHELL_NAME" in
        bash)
            echo "Configuring bash..."

            # .bashrc for interactive non-login shells (most Linux terminals)
            if [ -f "$HOME/.bashrc" ]; then
                add_to_config "$HOME/.bashrc"
            fi

            # .bash_profile or .profile for login shells (macOS Terminal, SSH)
            if [ -f "$HOME/.bash_profile" ]; then
                add_to_config "$HOME/.bash_profile"
            elif [ -f "$HOME/.profile" ]; then
                add_to_config "$HOME/.profile"
            else
                # Create .bash_profile if neither exists
                add_to_config "$HOME/.bash_profile"
            fi
            ;;
        zsh)
            echo "Configuring zsh..."
            # .zshenv is always sourced (login and non-login shells)
            add_to_config "$HOME/.zshenv"
            ;;
        *)
            echo "Unknown shell: $SHELL_NAME"
            echo "Add this to your shell config to use koto:"
            echo ""
            echo "  . \"$ENV_FILE\""
            echo ""
            ;;
    esac

    if [ "$SHELL_NAME" = "bash" ] || [ "$SHELL_NAME" = "zsh" ]; then
        echo ""
        echo "Restart your shell or run:"
        echo "  source \"$ENV_FILE\""
    fi
else
    echo "Skipped shell configuration (--no-modify-path)"
    echo ""
    echo "To use koto, add this to your shell config:"
    echo "  . \"$ENV_FILE\""
    echo ""
fi
