#!/bin/bash

# Super STT Installation Script
# This script downloads and installs the appropriate pre-built binaries

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
BLINK='\033[5m'
BG_YELLOW='\033[43m'
BG_RED='\033[41m'
NC='\033[0m'

# Default values
INSTALL_PREFIX="$HOME/.local"
GITHUB_REPO="jorge-menjivar/super-stt"
VERSION="latest"
INSTALL_OPTION="all"

# Parse arguments
INTERACTIVE=true

TEMP_DIR=$(mktemp -d)

echo "Temp directory: $TEMP_DIR"

DESKTOP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons"
METAINFO_DIR="$HOME/.local/share/metainfo"
USER_SYSTEMD_DIR="$HOME/.config/systemd/user"

# Print functions
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Detect system architecture
detect_arch() {
    local arch=$(uname -m)
    case "$arch" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) print_error "Unsupported architecture: $arch"; exit 1 ;;
    esac
}

# Detect GPU compute capability
detect_gpu_compute_cap() {
    if ! command -v nvidia-smi &> /dev/null; then
        return
    fi

    # Get GPU name and try to determine compute capability
    local gpu_name=$(nvidia-smi --query-gpu=name --format=csv,noheader,nounits 2>/dev/null | head -1)

    if [ -z "$gpu_name" ]; then
        return
    fi

    print_info "Detected GPU: $gpu_name" >&2

    # Map GPU names to compute capabilities
    case "$gpu_name" in
        # SM 12.0 (Blackwell)
        *"RTX 50"*|*"RTX PRO"*"Blackwell"*) echo "120" ;;

        # SM 10.0 (Blackwell server)
        *"B200"*|*"GB200"*) echo "100" ;;

        # SM 9.0 (Hopper)
        *"H100"*|*"H200"*|*"GH200"*) echo "90" ;;

        # SM 8.9 (Ada Lovelace)
        *"RTX 40"*|*"RTX Ada"*|*"L4"*|*"L40"*) echo "89" ;;

        # SM 8.6 (Ampere consumer)
        *"RTX 30"*|*"RTX A"[0-9]*|*"A40"*|*"A10"*|*"A16"*|*"A2"*) echo "86" ;;

        # SM 8.0 (Ampere datacenter)
        *"A100"*|*"A30"*) echo "80" ;;

        # SM 7.5 (Turing)
        *"RTX 20"*|*"TITAN RTX"*|*"QUADRO RTX"*|*"T4"*|*"T1000"*|*"T1200"*|*"T2000"*|*"T400"*|*"T500"*|*"T600"*|*"GTX 1650 Ti"*) echo "75" ;;

        # Default fallback - use most compatible (SM 7.5)
        *)
            print_warn "Unknown GPU: $gpu_name - using SM 7.5 (most compatible)" >&2
            echo "75"
            ;;
    esac
}

# Check for CUDA toolkit libraries
check_cuda_libraries() {
    # Check for essential CUDA libraries that candle-core needs
    local required_libs=("libcurand.so" "libcublas.so" "libcufft.so" "libcusolver.so")
    local missing_libs=()

    for lib in "${required_libs[@]}"; do
        if ! ldconfig -p 2>/dev/null | grep -q "$lib" && \
           ! find /usr/local/cuda/lib* /usr/lib* /lib* -name "${lib}*" 2>/dev/null | grep -q .; then
            missing_libs+=("$lib")
        fi
    done

    if [ ${#missing_libs[@]} -eq 0 ]; then
        return 0  # All libraries found
    else
        print_warn "Missing CUDA toolkit libraries: ${missing_libs[*]}" >&2
        return 1  # Some libraries missing
    fi
}

# Detect CUDA major version (12 or 13) based on installed libraries
detect_cuda_major_version() {
    # Check for CUDA 13 libraries first (newer)
    if ldconfig -p 2>/dev/null | grep -q "libcublas.so.13" || \
       find /usr/local/cuda/lib* /usr/lib* /lib* -name "libcublas.so.13*" 2>/dev/null | grep -q .; then
        echo "13"
        return
    fi

    # Check for CUDA 12 libraries
    if ldconfig -p 2>/dev/null | grep -q "libcublas.so.12" || \
       find /usr/local/cuda/lib* /usr/lib* /lib* -name "libcublas.so.12*" 2>/dev/null | grep -q .; then
        echo "12"
        return
    fi

    # Fallback: try to detect from nvcc or cuda version file
    if command -v nvcc &> /dev/null; then
        local nvcc_version=$(nvcc --version 2>/dev/null | grep "release" | sed 's/.*release \([0-9]*\)\..*/\1/')
        if [ -n "$nvcc_version" ]; then
            echo "$nvcc_version"
            return
        fi
    fi

    # Check CUDA version file
    if [ -f /usr/local/cuda/version.txt ]; then
        local cuda_ver=$(cat /usr/local/cuda/version.txt | grep -oP 'CUDA Version \K[0-9]+')
        if [ -n "$cuda_ver" ]; then
            echo "$cuda_ver"
            return
        fi
    fi

    # Default to CUDA 12 as it's more widely available
    echo "12"
}

# Detect CUDA/cuDNN availability and compute capability
detect_cuda() {
    if command -v nvidia-smi &> /dev/null; then
        print_info "NVIDIA GPU detected" >&2
        if nvidia-smi --version &> /dev/null && nvidia-smi | grep -q "CUDA Version"; then
            CUDA_VERSION=$(nvidia-smi | grep "CUDA Version" | sed 's/.*CUDA Version: \([0-9.]*\).*/\1/')
            print_info "CUDA runtime $CUDA_VERSION detected" >&2

            # Check if CUDA toolkit libraries are actually available
            if ! check_cuda_libraries; then
                print_warn "CUDA runtime detected but CUDA toolkit libraries missing" >&2
                print_warn "Install CUDA toolkit to use GPU acceleration" >&2
                echo "cpu"
                return
            fi

            print_info "CUDA toolkit libraries found" >&2

            # Detect CUDA major version (12 or 13)
            local cuda_major=$(detect_cuda_major_version)
            print_info "CUDA toolkit major version: $cuda_major" >&2

            # Detect compute capability
            local compute_cap=$(detect_gpu_compute_cap)

            # Check for cuDNN
            CUDNN_FOUND=false
            if [ -f /usr/local/cuda/include/cudnn.h ] || [ -f /usr/include/cudnn.h ] || \
               [ -f /usr/local/include/cudnn.h ] || ldconfig -p 2>/dev/null | grep -q libcudnn || \
               find /usr -name "libcudnn.so*" 2>/dev/null | grep -q .; then
                CUDNN_FOUND=true
            fi

            if [ -n "$compute_cap" ]; then
                if [ "$CUDNN_FOUND" = true ]; then
                    print_info "cuDNN found - using cuda${cuda_major}-cudnn-sm${compute_cap} variant" >&2
                    echo "cuda${cuda_major}-cudnn-sm${compute_cap}"
                else
                    print_info "cuDNN not found - using cuda${cuda_major}-sm${compute_cap} variant" >&2
                    echo "cuda${cuda_major}-sm${compute_cap}"
                fi
            else
                # Fallback to generic CUDA variants
                if [ "$CUDNN_FOUND" = true ]; then
                    print_info "cuDNN found - using generic cuda${cuda_major}-cudnn-sm75 variant" >&2
                    echo "cuda${cuda_major}-cudnn-sm75"
                else
                    print_info "cuDNN not found - using generic cuda${cuda_major}-sm75 variant" >&2
                    echo "cuda${cuda_major}-sm75"
                fi
            fi
        else
            print_warn "NVIDIA GPU found but CUDA runtime not available - using CPU variant" >&2
            echo "cpu"
        fi
    else
        print_info "No NVIDIA GPU detected - using CPU variant" >&2
        echo "cpu"
    fi
}

# Setup stt group for secure daemon access
setup_stt_group() {
    if command -v groupadd &> /dev/null; then
        if ! getent group stt > /dev/null 2>&1; then
            print_info "Creating stt group..."
            sudo groupadd stt || true
        fi
        print_info "Adding current user to stt group..."
        sudo usermod -a -G stt "$(whoami)" || true
    fi
}

# Install daemon and CLI components
install_daemon() {
    # Setup group and directories
    setup_stt_group
    mkdir -p "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/stt"
    mkdir -p "$HOME/.local/share/stt/logs"
    mkdir -p "$HOME/.config/systemd/user"

    # Install daemon binary
    print_info "Installing daemon and CLI..."
    mkdir -p "$INSTALL_PREFIX/bin"

    install -m 755 "$TEMP_DIR/super-stt" "$INSTALL_PREFIX/bin/"

    # Create wrapper script for group switching
    cat > "$TEMP_DIR/stt" << 'EOF'
#!/bin/bash
# Super STT Daemon Wrapper
exec sg stt -c "$INSTALL_PREFIX/bin/super-stt $*"
EOF

    # Replace the placeholder with actual install prefix
    sed -i "s|\$INSTALL_PREFIX|$INSTALL_PREFIX|g" "$TEMP_DIR/stt"

    install -m 755 "$TEMP_DIR/stt" "$INSTALL_PREFIX/bin/"
}

# Install desktop app
install_app() {
    print_info "Installing desktop app..."
    mkdir -p "$INSTALL_PREFIX/bin"

    # Install binary
    install -m 755 "$TEMP_DIR/super-stt-app" "$INSTALL_PREFIX/bin/"

    print_info "Installing desktop integration files..."

    # Install desktop file
    mkdir -p "$DESKTOP_DIR"
    install -m 644 "$TEMP_DIR/resources/super-stt-app.desktop" "$DESKTOP_DIR/super-stt-app.desktop"

    # Install icon
    mkdir -p "$ICON_DIR"
    cp -r "$TEMP_DIR/resources/icons/hicolor/scalable/apps/super-stt-app.svg" "$ICON_DIR/"

    # Install metainfo
    mkdir -p "$METAINFO_DIR"
    install -m 644 "$TEMP_DIR/resources/super-stt-app.metainfo.xml" "$METAINFO_DIR/super-stt-app.metainfo.xml"

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
    fi
}

# Install COSMIC applet
install_applet() {
    if ! command -v cosmic-panel &> /dev/null; then
        print_warn "COSMIC panel not found - skipping applet installation"
        return 0
    fi

    print_info "Installing COSMIC applet..."
    mkdir -p "$INSTALL_PREFIX/bin"

    # Install binary
    install -m 755 "$TEMP_DIR/super-stt-cosmic-applet" "$INSTALL_PREFIX/bin/"

    print_info "Installing COSMIC applet integration files..."

    # Install desktop files
    mkdir -p "$DESKTOP_DIR"
    for desktop_file in "$TEMP_DIR/resources/super-stt-cosmic-applet-"*.desktop; do
        local filename=$(basename "$desktop_file")
        install -m 644 "$desktop_file" "$DESKTOP_DIR/$filename"
    done

    # Install icon
    mkdir -p "$ICON_DIR"
    cp -r "$TEMP_DIR/resources/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg" "$ICON_DIR/"

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
    fi
}

# Install systemd service
install_service() {
    if ! command -v systemctl &> /dev/null; then
        print_warn "Systemd not detected - skipping service installation"
        return 0
    fi

    print_info "Installing systemd service..."

    mkdir -p "$USER_SYSTEMD_DIR"
    cp "$TEMP_DIR/systemd/super-stt.service" "$USER_SYSTEMD_DIR/"
    systemctl --user daemon-reload

    # Enable and start the service
    print_info "Enabling and starting Super STT daemon service..."
    systemctl --user enable super-stt
    if systemctl --user start super-stt; then
        print_info "Super STT daemon service started successfully"
    else
        print_warn "Failed to start Super STT daemon service"
        print_warn "You can start it manually with: systemctl --user start super-stt"
    fi
}

# Update PATH if needed
update_path() {
    local bin_dir="$1"

    if [[ ":$PATH:" != *":$bin_dir:"* ]]; then
        print_warn "Add $bin_dir to your PATH:"
        print_warn "  export PATH=\"$bin_dir:\$PATH\""

        # Try to add to shell config for user installations
        if [[ "$bin_dir" == "$HOME"* ]]; then
            SHELL_CONFIG=""
            if [ -f "$HOME/.bashrc" ]; then
                SHELL_CONFIG="$HOME/.bashrc"
            elif [ -f "$HOME/.zshrc" ]; then
                SHELL_CONFIG="$HOME/.zshrc"
            fi

            if [ -n "$SHELL_CONFIG" ]; then
                if ! grep -q "$bin_dir" "$SHELL_CONFIG"; then
                    echo "export PATH=\"$bin_dir:\$PATH\"" >> "$SHELL_CONFIG"
                    print_info "Added PATH update to $SHELL_CONFIG"
                fi
            fi
        fi
    fi
}

# Configure COSMIC keyboard shortcut
configure_cosmic_shortcut() {
    # Check if we're on COSMIC desktop
    if ! command -v cosmic-panel &> /dev/null; then
        return 0
    fi

    COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
    COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

    # Ask user if they want to add the shortcut
    echo -n "Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: "
    if [ -t 2 ]; then
        read -r add_shortcut < /dev/tty
    else
        # Non-interactive fallback - default to yes
        add_shortcut="y"
        echo "y"
    fi

    if [[ "$add_shortcut" =~ ^[Nn]$ ]]; then
        return 0
    fi

    # Create the shortcuts directory if it doesn't exist
    mkdir -p "$COSMIC_SHORTCUTS_DIR"

    # Use the full path to the stt wrapper for reliability
    local stt_command="$INSTALL_PREFIX/bin/stt record --write"

    # Check if shortcuts file exists and has content
    if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
        # File exists with content, check if our shortcut is already there
        if grep -q "Super STT" "$COSMIC_SHORTCUTS_FILE"; then
            return 0
        fi

        # Check if Super+Space is already used
        if grep -q 'key: "space"' "$COSMIC_SHORTCUTS_FILE" && grep -A5 -B5 'key: "space"' "$COSMIC_SHORTCUTS_FILE" | grep -q 'Super'; then
            return 0
        fi

        # Create a backup
        cp "$COSMIC_SHORTCUTS_FILE" "$COSMIC_SHORTCUTS_FILE.backup"

        # Create a temporary file with the new shortcut entry
        local temp_file=$(mktemp)

        # Check if the file is empty (just {}) and handle accordingly
        if grep -q '^{}$' "$COSMIC_SHORTCUTS_FILE"; then
            # File is empty, replace entirely
            echo '{' > "$temp_file"
        else
            # File has content, remove the closing brace and add our shortcut
            head -n -1 "$COSMIC_SHORTCUTS_FILE" > "$temp_file"
        fi

        cat >> "$temp_file" << EOFSHORTCUT
    (
        modifiers: [
            Super,
        ],
        key: "space",
        description: Some("Super STT"),
    ): Spawn("$stt_command"),
}
EOFSHORTCUT

        # Replace the original file
        mv "$temp_file" "$COSMIC_SHORTCUTS_FILE"

        # Verify the file is valid by checking it has proper structure
        if ! grep -q '^{' "$COSMIC_SHORTCUTS_FILE" || ! grep -q '^}' "$COSMIC_SHORTCUTS_FILE"; then
            mv "$COSMIC_SHORTCUTS_FILE.backup" "$COSMIC_SHORTCUTS_FILE"
            return 1
        fi

        # Remove backup if successful
        rm -f "$COSMIC_SHORTCUTS_FILE.backup"
    else
        # Create new shortcuts file with our shortcut

        cat > "$COSMIC_SHORTCUTS_FILE" << EOF
{
    (
        modifiers: [
            Super,
        ],
        key: "space",
        description: Some("Super STT"),
    ): Spawn("$stt_command"),
}
EOF
    fi

}

for arg in "$@"; do
    case $arg in
        --non-interactive)
            INTERACTIVE=false
            shift
            ;;
        --version=*)
            VERSION="${arg#*=}"
            shift
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
    esac
done

# Interactive menu function
show_menu() {
    echo "============================================="
    echo "         Super STT Installation Menu"
    echo "============================================="
    echo ""
    echo "Detected system:"
    echo "  Architecture: $ARCH"
    echo "  Optimal variant: $VARIANT"
    echo ""
    echo "What would you like to install?"
    echo ""
    echo "1. All $([ ! command -v cosmic-panel &> /dev/null ] && echo "(skip COSMIC applet)" || echo "(includes COSMIC applet)") [DEFAULT]"
    echo "2. Daemon + CLI only"
    echo "3. Desktop App only"
    echo "4. COSMIC Applet only $([ ! command -v cosmic-panel &> /dev/null ] && echo "(not available)")"
    echo ""
    echo "q. Quit"
    echo ""
    echo "============================================="
}

handle_menu() {
    while true; do
        show_menu
        echo -n "Select option [1-4, q] (default: 1): "
        read -r choice

        # Default to option 1 if empty
        if [ -z "$choice" ]; then
            choice="1"
        fi

        case $choice in
            1)
                INSTALL_OPTION="all"
                break
                ;;
            2)
                INSTALL_OPTION="daemon"
                break
                ;;
            3)
                INSTALL_OPTION="app"
                break
                ;;
            4)
                if ! command -v cosmic-panel &> /dev/null; then
                    print_warn "COSMIC panel not found - applet not available"
                    sleep 1
                else
                    INSTALL_OPTION="applet"
                    break
                fi
                ;;
            q|Q)
                print_info "Installation cancelled"
                exit 0
                ;;
            *)
                print_warn "Invalid option. Please try again."
                sleep 1
                ;;
        esac
    done
}

# Show interactive menu if in interactive mode
# Check if we have a controlling terminal (works better with piped input)
if [ "$INTERACTIVE" = true ] && [ -t 2 ]; then
    # Do minimal detection for menu display
    ARCH=$(detect_arch)
    VARIANT=$(detect_cuda)

    # Save current stdin and redirect to terminal for menu
    exec 3<&0
    exec < /dev/tty
    handle_menu
    clear
    # Restore original stdin
    exec 0<&3
    exec 3<&-
fi

# Check if we need sudo and prompt early for daemon installations
if [ "$INSTALL_OPTION" = "all" ] || [ "$INSTALL_OPTION" = "daemon" ]; then
    print_info "Daemon installation requires sudo to create the stt group"
    print_info "You will be prompted for your password..."
    # Test sudo access early
    if ! sudo -v; then
        print_error "Sudo access required for daemon installation"
        exit 1
    fi
fi

# Check CUDA availability and warn user if needed
check_cuda_availability() {
    if [ "$1" = "all" ] || [ "$1" = "daemon" ]; then
        if command -v nvidia-smi &> /dev/null; then
            # NVIDIA GPU detected, check if CUDA toolkit is available
            if nvidia-smi --version &> /dev/null && nvidia-smi | grep -q "CUDA Version"; then
                local cuda_version=$(nvidia-smi | grep "CUDA Version" | sed 's/.*CUDA Version: \([0-9.]*\).*/\1/')

                # Check for CUDA toolkit libraries
                if ! check_cuda_libraries; then
                    echo ""
                    echo -e "${BLINK}${BOLD}${BG_YELLOW}${RED}  ⚠️  WARNING ⚠️  ${NC}"
                    echo -e "${BOLD}${YELLOW}══════════════════════════════════════════════════════════${NC}"
                    echo -e "${BOLD}${YELLOW}${NC}  ${BOLD}${RED}🚨🚨🚨 CUDA toolkit missing! 🚨🚨🚨${NC}  ${BOLD}${YELLOW}${NC}"
                    echo -e "${BOLD}${YELLOW}══════════════════════════════════════════════════════════${NC}"
                    print_warn ""
                    print_warn "An NVIDIA GPU was detected, but no CUDA toolkit was found!"
                    print_warn "Please install the CUDA toolkit to enable GPU acceleration:"
                    print_warn ""


                    # Detect distribution and show appropriate command
                    if command -v apt-get &> /dev/null; then
                        print_warn "  Ubuntu/PopOS/Debian:"
                        print_warn "    sudo apt-get install nvidia-cuda-toolkit nvidia-cuda-toolkit-gcc"
                    elif command -v dnf &> /dev/null; then
                        print_warn "  Fedora:"
                        print_warn "    sudo dnf install cuda cuda-devel"
                    elif command -v pacman &> /dev/null; then
                        print_warn "  Arch:"
                        print_warn "    sudo pacman -S cuda cuda-tools"
                    else
                        print_warn "  Install CUDA toolkit for your distribution"
                    fi
                    print_warn ""
                    print_warn "After installing the CUDA toolkit, run this script again to enable GPU acceleration."

                    print_warn ""
                    print_warn "Your system has:"
                    print_warn "  ✅ NVIDIA GPU with driver"
                    print_warn "  ✅ CUDA runtime $cuda_version"
                    print_warn "  ❌ CUDA toolkit libraries (needed for GPU acceleration)"
                    print_warn ""
                    print_warn "Documentation: https://github.com/jorge-menjivar/super-stt#cuda-gpu-acceleration"
                    print_warn ""

                    if [ "$INTERACTIVE" = true ] && [ -t 2 ]; then
                        echo -n "Continue with CPU-only installation? [y/N]: "
                        read -r continue_cpu < /dev/tty
                        if [[ ! "$continue_cpu" =~ ^[Yy]$ ]]; then
                            print_info "Installation aborted. Install CUDA toolkit and run installer again for GPU acceleration."
                            exit 0
                        fi
                        print_info "Continuing with CPU-only installation..."
                    else
                        print_warn "Non-interactive mode: continuing with CPU-only installation"
                    fi
                    print_warn ""
                fi
            fi
        fi
    fi
}

# Detect what we need based on install option
ARCH=$(detect_arch)
print_info "Detected architecture: $ARCH"

# Check CUDA availability and warn user before proceeding
check_cuda_availability "$INSTALL_OPTION"

if [ "$INSTALL_OPTION" = "all" ] || [ "$INSTALL_OPTION" = "daemon" ]; then
    # Need optimal daemon variant
    VARIANT=$(detect_cuda)
    print_info "Using variant: $VARIANT"
else
    # For app/applet-only, just use CPU variant (app/applet are identical in all variants)
    VARIANT="cpu"
fi

# Get the latest release version if not specified
if [ "$VERSION" = "latest" ]; then
    print_info "Fetching latest release version..."
    VERSION=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

    if [ -z "$VERSION" ]; then
        print_error "Failed to fetch latest version"
        exit 1
    fi
fi

print_info "Installing Super STT $VERSION"

# Construct download URL
TARBALL_NAME="super-stt-${ARCH}-${VARIANT}.tar.gz"
DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$TARBALL_NAME"

print_info "Downloading from: $DOWNLOAD_URL"

# Create temporary directory
# trap "rm -rf $TEMP_DIR" EXIT

# Download the tarball with fallback support
download_with_fallback() {
    local variant="$1"
    local arch="$2"
    local tarball_name="super-stt-${arch}-${variant}.tar.gz"
    local download_url="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$tarball_name"

    print_info "Trying to download: $tarball_name" >&2

    if curl -L -f -o "$TEMP_DIR/$tarball_name" "$download_url" 2>/dev/null; then
        echo "$tarball_name"
        return 0
    fi

    # If specific compute capability failed, try fallbacks
    # Variant format: cuda{12,13}[-cudnn]-sm{75,80,86,89,90,100,120}
    if [[ "$variant" =~ ^cuda([0-9]+)(-cudnn)?-sm([0-9]+)$ ]]; then
        local cuda_ver="${BASH_REMATCH[1]}"
        local cudnn_part="${BASH_REMATCH[2]}"  # Either "-cudnn" or empty
        local sm_cap="${BASH_REMATCH[3]}"

        # Try fallback to sm75 with same CUDA version
        local base_variant="cuda${cuda_ver}${cudnn_part}-sm75"
        if [ "$variant" != "$base_variant" ]; then
            print_warn "Specific compute capability variant not found, trying $base_variant" >&2
            tarball_name="super-stt-${arch}-${base_variant}.tar.gz"
            download_url="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$tarball_name"

            if curl -L -f -o "$TEMP_DIR/$tarball_name" "$download_url" 2>/dev/null; then
                echo "$tarball_name"
                return 0
            fi
        fi

        # Try alternative CUDA version (12 <-> 13)
        local alt_cuda_ver
        if [ "$cuda_ver" = "13" ]; then
            alt_cuda_ver="12"
        else
            alt_cuda_ver="13"
        fi

        local alt_variant="cuda${alt_cuda_ver}${cudnn_part}-sm75"
        print_warn "CUDA $cuda_ver variants not found, trying CUDA $alt_cuda_ver ($alt_variant)" >&2
        tarball_name="super-stt-${arch}-${alt_variant}.tar.gz"
        download_url="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$tarball_name"

        if curl -L -f -o "$TEMP_DIR/$tarball_name" "$download_url" 2>/dev/null; then
            print_warn "Using CUDA $alt_cuda_ver build - may have compatibility issues" >&2
            echo "$tarball_name"
            return 0
        fi

        # Last resort: try CPU variant
        print_warn "CUDA variants not found, falling back to CPU variant" >&2
        tarball_name="super-stt-${arch}-cpu.tar.gz"
        download_url="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$tarball_name"

        if curl -L -f -o "$TEMP_DIR/$tarball_name" "$download_url" 2>/dev/null; then
            echo "$tarball_name"
            return 0
        fi
    fi

    return 1
}

# Try to download with fallback support
DOWNLOADED_TARBALL=$(download_with_fallback "$VARIANT" "$ARCH")

if [ -z "$DOWNLOADED_TARBALL" ]; then
    print_error "Failed to download any compatible variant"
    print_error "Tried URLs:"
    print_error "  - https://github.com/$GITHUB_REPO/releases/download/$VERSION/super-stt-${ARCH}-${VARIANT}.tar.gz"
    if [[ "$VARIANT" =~ ^cuda([0-9]+)(-cudnn)?-sm([0-9]+)$ ]]; then
        err_cuda_ver="${BASH_REMATCH[1]}"
        err_cudnn_part="${BASH_REMATCH[2]}"
        err_alt_cuda_ver=$([[ "$err_cuda_ver" == "13" ]] && echo "12" || echo "13")
        print_error "  - https://github.com/$GITHUB_REPO/releases/download/$VERSION/super-stt-${ARCH}-cuda${err_cuda_ver}${err_cudnn_part}-sm75.tar.gz"
        print_error "  - https://github.com/$GITHUB_REPO/releases/download/$VERSION/super-stt-${ARCH}-cuda${err_alt_cuda_ver}${err_cudnn_part}-sm75.tar.gz"
        print_error "  - https://github.com/$GITHUB_REPO/releases/download/$VERSION/super-stt-${ARCH}-cpu.tar.gz"
    fi
    exit 1
fi

print_info "Successfully downloaded: $DOWNLOADED_TARBALL"

# Extract the tarball
print_info "Extracting binaries..."
tar -xzf "$TEMP_DIR/$DOWNLOADED_TARBALL" -C "$TEMP_DIR"

# Install components based on selection
case $INSTALL_OPTION in
    "all")
        # Install everything (skip applet if COSMIC not available)
        install_daemon
        install_app
        if command -v cosmic-panel &> /dev/null; then
            install_applet
        fi
        install_service
        ;;
    "daemon")
        # Install daemon + CLI + service
        install_daemon
        install_service
        ;;
    "app")
        # Install app only
        install_app
        ;;
    "applet")
        # Install applet only
        install_applet
        ;;
esac

# Add to PATH if installing to non-standard location
if [ "$INSTALL_PREFIX" != "/usr/local" ] && [ "$INSTALL_PREFIX" != "/usr" ]; then
    update_path "$INSTALL_PREFIX/bin"
fi

# Configure COSMIC shortcut if daemon was installed and in interactive mode
if [ "$INTERACTIVE" = true ] && ([ "$INSTALL_OPTION" = "all" ] || [ "$INSTALL_OPTION" = "daemon" ]); then
    configure_cosmic_shortcut "$INSTALL_PREFIX"
fi

print_info ""
print_info "Installation complete!"
print_info ""
print_info "Installed components:"

# Determine the actual variant that was installed
INSTALLED_VARIANT=$(echo "$DOWNLOADED_TARBALL" | sed 's/super-stt-.*-\(.*\)\.tar\.gz/\1/')

case $INSTALL_OPTION in
    "all")
        print_info "  ✅ super-stt (daemon + CLI) [$INSTALLED_VARIANT variant]"
        print_info "  ✅ stt (convenience wrapper)"
        [ -f "$INSTALL_PREFIX/bin/super-stt-app" ] && print_info "  ✅ super-stt-app (desktop app)"
        [ -f "$INSTALL_PREFIX/bin/super-stt-cosmic-applet" ] && print_info "  ✅ super-stt-cosmic-applet (COSMIC applet)"
        print_info "  ✅ systemd user service"
        print_info ""
        print_info "Remember to log out and back in, or run: newgrp stt"
        print_info ""
        print_info "Then run 'stt record --write' to get started"
        ;;
    "daemon")
        print_info "  ✅ super-stt (daemon + CLI) [$INSTALLED_VARIANT variant]"
        print_info "  ✅ stt (convenience wrapper)"
        print_info "  ✅ systemd user service"
        print_info ""
        print_info "Remember to log out and back in, or run: newgrp stt"
        print_info ""
        print_info "Then run 'stt record --write' to get started"
        ;;
    "app")
        [ -f "$INSTALL_PREFIX/bin/super-stt-app" ] && print_info "  ✅ super-stt-app (desktop app)"
        print_info ""
        print_info "Desktop app installed. Note: You'll need the daemon to use Super STT functionality."
        ;;
    "applet")
        [ -f "$INSTALL_PREFIX/bin/super-stt-cosmic-applet" ] && print_info "  ✅ super-stt-cosmic-applet (COSMIC applet)"
        print_info ""
        print_info "COSMIC applet installed. Note: You'll need the daemon to use Super STT functionality."
        ;;
esac
