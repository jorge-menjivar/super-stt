<div align="center">

<img src=".github/assets/super-stt-icon.svg" width="128" height="128" alt="Super STT">

# Super STT

**High-performance speech-to-text service for Linux**

*Easy to install • State-of-the-art Voice Models • Built in Rust • GPU Acceleration*

</div>

https://github.com/user-attachments/assets/bbbe20c3-6802-4797-afc8-aa81d1b48415


## ✨ Features

- **✅ Easy installation** with automated installer
- **⚡ Real-time transcription** built to be executed with a shortcut
- **⌨️ Auto-typing** transcribed text
- **🎯 Automatic GPU acceleration** when CUDA is present. Supports cuDNN for optimal performance as well
- **⭐ State-of-the-art Voxtral models support** for optimal performance
- **🔄 Model switching** on the fly from the app
- **🎵 Real-time audio visualization** for COSMIC Desktop with the COSMIC Desktop applet
- **🔠 Live preview** of audio input (beta, experimental)

## 🤖 Supported Models
- voxtral-mini (SOTA) **Recommended with GPU**
- voxtral-small (SOTA)
- whisper-tiny **Default**
- whisper-base **Recommended with CPU**
- whisper-small
- whisper-medium
- whisper-large-v3

## 🚀 Installation

### Quick Install (Recommended)

Install with our automated installer that detects your system and downloads pre-built binaries:

```bash
curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash
```

The installer will show a menu where you can choose which components to install:
- **All** [DEFAULT] - Everything (daemon, app, applet if on COSMIC, systemd service)
- **Daemon + CLI** - Core functionality + systemd service
- **Desktop App** - GUI for configuration only
- **COSMIC Applet** - Panel integration only

The installer automatically:
- Detects your system architecture (x86_64/ARM64) to download the optimal pre-built binary
- Installs components to `~/.local/bin`
- **Daemon**: Detects CUDA/cuDNN for automatic GPU acceleration
- **Daemon**: Creates the `stt` group and adds the current user to it **(requires sudo)**
- **Daemon**: Sets up systemd user service
- **Daemon**: Creates required directories for logs and sockets
- **COSMIC Desktop**: Offers to configure Super+Space keyboard shortcut automatically

## Screenshots
### Multiple Visualization Styles
![Centered Bars Visualization](.github/assets/visualization-centered-bars.png)
![Equalizer Visualization](.github/assets/visualization-equalizer.png)
![Waveform Visualization](.github/assets/visualization-waveforms.png)

### Custom Colors
![System Theme](.github/assets/color-options-system-theme.png)
![Green](.github/assets/color-options-green.png)

## ⌨️ Setting Up Keyboard Shortcuts

For the best experience, add a keyboard shortcut to your desktop environment:

### COSMIC Desktop

**Automatic Setup**:
- **Web Installer**: The installer automatically offers to configure Super+Space shortcut during daemon installation. Just answer "Y" when prompted!
- **Building from source**:
  - Run `just install-daemon` - it will offer to set up the shortcut automatically
  - Or run `just setup-cosmic-shortcut` anytime (no separate scripts needed)

**What gets configured**:
- The shortcut uses the full path for reliability: `/home/user/.local/bin/stt record --write`
- Safely extends your existing COSMIC shortcuts without overwriting
- Detects conflicts and warns if Super+Space is already in use

**Manual Setup**:
1. Open COSMIC Settings
2. Navigate to: **Input Devices** → **Keyboard** → **View and customize shortcuts** → **Custom**
3. Click **Add Shortcut**
4. Set command: `/home/user/.local/bin/stt record --write` (replace `/home/user` with your actual home path)
5. Choose your preferred key combination (e.g., `Super+Space`)

### Other Desktop Environments

**GNOME:**
```bash
# Add via command line
gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/']"
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/ name 'Super STT'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/ command 'stt record --write'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/ binding '<Super>space'
```

**KDE Plasma:**
1. Open System Settings → Shortcuts → Custom Shortcuts
2. Add new shortcut with command: `stt record --write`

**Or configure through your desktop environment's settings for custom keyboard shortcuts.**

### Building from Source

```bash
git clone https://github.com/jorge-menjivar/super-stt.git
cd super-stt

# The Desktop application to manage settings and models
just install-app

# (Optional) COSMIC Desktop applet to show visualizations
just install-applet

# The Daemon and CLI tool (offers to set up COSMIC shortcut automatically)
just install-daemon
# (Suggestion) If you have an NVIDIA GPU with CUDA installed, build with CUDA acceleration instead to load models onto the GPU
# just install-daemon --cuda
# (Suggestion) If you have an NVIDIA GPU with CUDA and cuDNN installed, build with cuDNN acceleration instead for better performance
# just install-daemon --cudnn

# Or set up COSMIC keyboard shortcut separately
just setup-cosmic-shortcut
```

## Prerequisites

### Debian/Ubuntu/Pop!_OS
You may need to install the following dependencies:

```sudo apt install build-essential libxkbcommon-dev libasound2-dev pkg-config libssl-dev```

**If you find any missing make a pull request to update this list. Thanks!**

### Fedora
You may need to install the following dependencies:

```sudo dnf install gcc gcc-c++ libxkbcommon-devel alsa-lib-devel pkgconf perl-FindBin perl-IPC-Cmd openssl-devel```

**If you find any missing make a pull request to update this list. Thanks!**

### Arch
You may need to install the following dependencies:

```sudo pacman -S pkgconf openssl```

**If you find any missing make a pull request to update this list. Thanks!**

### CUDA GPU Acceleration

Super STT automatically detects and uses CUDA-enabled GPUs for acceleration. If you have an NVIDIA GPU, but the installation script cannot find the CUDA toolkit, you need to install it manually:

#### Ubuntu/PopOS/Debian
```bash
sudo apt-get install nvidia-cuda-toolkit nvidia-cuda-toolkit-gcc
```

#### Fedora
See [https://rpmfusion.org/Howto/CUDA](https://rpmfusion.org/Howto/CUDA)

#### Arch Linux
```bash
sudo pacman -S cuda
```

**Note**: If you already have the CUDA toolkit installed, but the installation script still cannot find it, please create a new issue. Thanks!

## ⌨️ Shortcuts
Add a shortcut that executes the following command to your desktop environment to start the app.
```sh
stt record --write
```

On COSMIC, you can add the shortcut in COSMIC Settings app: Input Devices -> Keyboard -> View and customize shortcuts -> Custom -> Add Shortcut

> **Note**: On first run, Super STT will automatically download the required AI model (~1-2GB). This may take a few minutes depending on your internet connection.

The following will then happen:

1. `stt` will contact the already running daemon and ask it to use the microphone to transcribe audio.
2. The daemon will then process the audio and start typing a preview of the transcription into whatever application is currently focused. This may be a bit inaccurate.
3. The daemon will automatically stop listening when it no longer detects speech.
4. Once it stops recording, it will process the full transcription one more time, this will produce an accurate transcription.
5. The daemon will automatically replace the preview with the accurate transcription.

### Usage

After installation, manage the daemon with:
```bash
# Start the daemon
systemctl --user start super-stt

# Enable auto-start with user session
systemctl --user enable super-stt

# Check status
systemctl --user status super-stt

# View logs
journalctl --user -u super-stt -f
```

Then use the `stt` command:
```bash
# Record and transcribe
stt record

# Record, transcribe, and auto-type the result
stt record --write
```

### Troubleshooting

#### `stt` command not found
The installer adds `~/.local/bin` to your PATH. Either:
- Restart your terminal, or
- Run: `export PATH="$HOME/.local/bin:$PATH"`

#### "sg: group 'stt' does not exist" error
The `stt` group wasn't created properly. Run:
```bash
sudo groupadd stt
sudo usermod -a -G stt $(whoami)
newgrp stt
```

#### "Operation not permitted" when using stt
You need to be in the `stt` group. Either:
- Log out and back in, or
- Run: `newgrp stt`

#### Daemon not starting
Check the logs for errors:
```bash
journalctl --user -u super-stt -n 50
```

### Model Selection
Open the Super STT app → Settings → Select model

## 🏗️ Architecture

- **`super-stt`** - Background ML service
- **`super-stt-app`** - Desktop configuration app
- **`super-stt-cosmic-applet`** - Panel applet with visualizations
- **`super-stt-shared`** - Common protocols

## 🔒 Security

Super STT implements comprehensive security controls:

- **Group-based Access**: The `stt` group restricts who can connect to the daemon socket
- **Process Authentication**: Keyboard injection requires verification that the client is the legitimate `stt` binary

For detailed security information, see [`docs/SECURITY.md`](docs/SECURITY.md).

### Security Model

**Production Mode** (default):
- Socket permissions: `0660` (owner + stt group only)
- Resource limits and rate limiting active

**Development Mode**:
- Debug builds automatically enable development mode with relaxed permissions
- Use only in secure development environments
- For production security, always use release builds

## 🔧 Development

```bash
# Run the daemon
just run-daemon

# Run the app
just run-app

# Run the applet
just run-applet

# Setup COSMIC keyboard shortcut (interactive)
just setup-cosmic-shortcut

# Install daemon with automatic COSMIC shortcut setup
just install-daemon

# Run security audit
just audit
```

---

<div align="center">

**Jorge Menjivar** • jorge@menjivar.ai

</div>
