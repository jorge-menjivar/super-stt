<div align="center">

<img src=".github/assets/super-stt-icon.svg" width="128" height="128" alt="Super STT">

# Super STT

**High-performance speech-to-text service for Linux**

*Easy to install • State-of-the-art Voice Models • Built in Rust • GPU Acceleration*

[![coverage](https://img.shields.io/endpoint?url=https://jorge-menjivar.github.io/super-stt/coverage/coverage.json)](https://jorge-menjivar.github.io/super-stt/coverage/)

</div>

https://github.com/user-attachments/assets/bbbe20c3-6802-4797-afc8-aa81d1b48415


## 🚀 Installation

### Quick Install (Recommended)

Install with our automated installer that detects your system and downloads pre-built binaries:

```bash
curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash
```

You can also append `-s -- --beta` to the command to install the latest beta version.

### Build from source

If you want the very latest changes, build locally:

```bash
git clone https://github.com/jorge-menjivar/super-stt.git
cd super-stt
just install                # CPU
just install --cuda         # NVIDIA GPU
```

`just install` builds and installs the daemon, CLI, consent helper, desktop app, and (on COSMIC) the panel applet, then enables the user systemd service. Run `just install-daemon`, `just install-app`, or `just install-applet` to install just one piece.

### What gets installed

The install script does the following:

- Drop binaries in `~/.local/bin` (`super-stt-daemon`, `super-stt-cli`, `super-stt-app`, plus `super-stt-consent` and `super-stt-cosmic-applet` where applicable)
- Set up a `systemctl --user` service for the daemon
- Detect CUDA/cuDNN and pick the right GPU build automatically
- Offer to wire up Super+Space → `stt record --write` on COSMIC

The installer's menu lets you pick a subset (daemon only, app only, applet only).

> For protocol, auth, and security details, see [docs/](./docs/).


## Recording Modes

### Stop Mode

Controls how a recording session ends. Configurable via the app, CLI (`--stop-mode`), or daemon config. Assuming you have a shortcut mapped to execute `stt record --write`:

| Mode                           | Behavior                                              |
|--------------------------------|-------------------------------------------------------|
| **Silence + Manual** (default) | Stops on silence detection or a second shortcut press |
| **Silence Only**               | Stops only when silence is detected                   |
| **Manual Only**                | Stops only on a second shortcut press                 |

### Write Method

Controls how transcribed text is typed into the focused application. Auto-detection tries each method in order.

| Method                         | Description                                                               |
|--------------------------------|---------------------------------------------------------------------------|
| **Auto** (default)             | Tries XDG Desktop Portal, then ydotool, then Wayland protocol             |
| **XDG Desktop Portal**         | Uses the desktop's RemoteDesktop portal (requires one-time authorization) |
| **ydotool**                    | Uses ydotool virtual input (requires ydotoold running)                    |
| **Wayland Protocol**           | Direct Wayland input simulation via the compositor                        |

Both settings can be changed in the desktop app under Settings, or per-recording via CLI flags:

```bash
stt record --write --stop-mode manual --write-method ydotool
```

## 🤖 Supported Models

### Local Models
All processing happens on your device. No audio data leaves your machine.

| Model                 | Notes                                      |
|-----------------------|--------------------------------------------|
| voxtral-mini          | State-of-the-art. **Recommended with GPU** |
| voxtral-small         | State-of-the-art                           |
| whisper-tiny          | **Default**                                |
| whisper-base          | **Recommended with CPU**                   |
| whisper-small         |                                            |
| whisper-medium        |                                            |
| whisper-large-v3      |                                            |

### Online Models

If your computer does not have enough resources for local models, you can send your audio to third-party providers for transcription. Online models are **disabled by default** and require you to explicitly enable them.

| Provider | Models                                               |
|----------|------------------------------------------------------|
| Mistral  | voxtral-mini-latest                                  |
| OpenAI   | whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe |
| Deepgram | nova-3                                               |

#### Enabling Online Models

1. Open the Super STT app
2. Navigate to **Online Models** in the sidebar
3. Enable the **Allow Online Models** toggle
4. Enter your API key for the provider you want to use (keys are stored in your system keyring)
5. Go to **Models**, open the model selector, and choose an online model

API keys are stored securely in your system keyring (GNOME Keyring, KWallet, etc.) 

<table>
<tr>
<td align="center"><strong>Online Models</strong><br><img src=".github/assets/online-models.png" width="320"></td>
<td align="center"><strong>Model Selection</strong><br><img src=".github/assets/models-selection.png" width="320"></td>
</tr>
</table>

## Screenshots
### Multiple Visualization Styles
<table>
<tr>
<td align="center"><strong>Centered Bars</strong><br><img src=".github/assets/visualization-centered-bars.png" width="320"></td>
<td align="center"><strong>Equalizer</strong><br><img src=".github/assets/visualization-equalizer.png" width="320"></td>
<td align="center"><strong>Waveform</strong><br><img src=".github/assets/visualization-waveforms.png" width="320"></td>
</tr>
</table>

### Custom Colors
<table>
<tr>
<td align="center"><strong>System Theme</strong><br><img src=".github/assets/color-options-system-theme.png" width="320"></td>
<td align="center"><strong>Green</strong><br><img src=".github/assets/color-options-green.png" width="320"></td>
</tr>
</table>

## ⌨️ Keyboard Shortcuts

Bind `stt record --write` to a key combo (Super+Space is the convention). On COSMIC the installer offers to do this for you. For other desktops, add a custom shortcut through Settings (the full command is `~/.local/bin/stt record --write`).

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

## How it works

> **Note**: On first run, Super STT downloads the required AI model (~1-2GB). This may take a few minutes.

When you press the shortcut:

1. `stt` asks the running daemon to start transcribing from your mic.
2. While you speak, the daemon types a live preview into whatever app is focused.
3. When you stop speaking (or press the shortcut again, depending on stop mode), the daemon does a final pass for accuracy and replaces the preview with the final text.

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
The installer adds `~/.local/bin` to your PATH. Restart your terminal, or run `export PATH="$HOME/.local/bin:$PATH"`.

#### Authorization popup shows "Path: <unknown>" or never appears
A previously installed legacy build is shadowing the daemon. Run `just uninstall` from the source tree (or remove `~/.local/bin/super-stt*` manually) and reinstall.

#### Daemon not starting
```bash
journalctl --user -u super-stt -n 50
```

#### "sg: group 'stt' does not exist" / "Operation not permitted"
This comes from an older install. The current daemon does not use an `stt`
group — the socket lives in your per-user `$XDG_RUNTIME_DIR` and is same-user
only. Remove the stale build and reinstall:
```bash
just uninstall   # or: rm -f ~/.local/bin/super-stt*
just install
```

## 🔧 Development

```bash
just run-daemon         # run the daemon in the foreground
just run-app            # run the settings app
just run-applet         # run the COSMIC applet
just setup-cosmic-shortcut  # add Super+Space binding on COSMIC
just audit              # security audit (cargo audit)
```

Architecture, protocol design, and security model live in [docs/](./docs/).

---

<div align="center">

**Jorge Menjivar** • jorge@menjivar.ai

</div>
