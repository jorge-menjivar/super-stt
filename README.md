<div align="center">

<img src=".github/assets/super-stt-icon.svg" width="128" height="128" alt="Super STT">

# Super STT

**Speak into any app on Linux. Your words appear as text.**

*One shortcut to dictate anywhere • Any model from a growing library • An open protocol any app can build on • Built in Rust*

[![coverage](https://img.shields.io/endpoint?url=https://jorge-menjivar.github.io/super-stt/coverage/coverage.json)](https://jorge-menjivar.github.io/super-stt/coverage/)

</div>

https://github.com/user-attachments/assets/bbbe20c3-6802-4797-afc8-aa81d1b48415


## What is Super STT?

Super STT makes speech-to-text with automatic text input **trivial on Linux, for everyone**. Bind a shortcut (Super+Space by convention), speak, and your words are typed straight into whatever app is focused, e.g. your editor, browser, chat, terminal. No copy-paste, no fiddling.

Under the hood it's two things:

- **A model-agnostic engine.** A background daemon installs speech models from a **library** of backends (local or cloud), loads one, and keeps it warm for instant transcription. You pick the model that fits your hardware and swap it whenever you like.
- **An open protocol.** The daemon speaks a documented HTTP protocol over a local socket, so *any* app, in any language, can request transcriptions, stream live audio visualizations, or drive recording, with per-app consent. Super STT's own desktop app, CLI, and COSMIC applet are just the first clients. See [Developers](#-developers).

## 🚀 Installation

**Quick install** - detects your system and downloads pre-built binaries:

```bash
curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash
```

Append `-s -- --beta` for the latest beta.

**Build from source** - for the very latest changes (needs the [build prerequisites](./CONTRIBUTING.md#prerequisites)):

```bash
git clone https://github.com/jorge-menjivar/super-stt.git
cd super-stt
just install
```

Either way you get the daemon, the `stt` CLI, a consent helper, the desktop app, and (on COSMIC) the panel applet, wired up as a `systemctl --user` service. On COSMIC the installer offers to bind Super+Space → `stt record --write` for you. GPU acceleration comes from the model you run (see [Models](#-models)).

## ⌨️ Using it

```bash
stt record --write # Record and transcribes. Types the result after silence is detected or you run the command again.
```

Bind `stt record --write` to a key combo. Super+Space is the convention (the COSMIC installer does this for you; on other desktops add a custom shortcut for `~/.local/bin/stt record --write`). When you trigger it:

1. `stt` asks the running daemon to start transcribing from your mic.
2. You speak; the daemon transcribes your audio.
3. When you stop (on silence or a second trigger), it types the transcription into the focused app.

Manage the daemon with the usual systemd controls:

```bash
systemctl --user start super-stt      # or: enable / status / restart
journalctl --user -u super-stt -f     # follow logs
```

### Recording modes

Two settings shape a session. Both live in the desktop app under **Settings** (or the daemon config); stop mode can also be set per-recording on the CLI.

**Stop mode** - how a recording ends:

| Mode                           | Behavior                                              |
|--------------------------------|-------------------------------------------------------|
| **Silence + Manual** (default) | Stops on silence detection or a second shortcut press |
| **Silence Only**               | Stops only when silence is detected                   |
| **Manual Only**                | Stops only on a second shortcut press                 |

```bash
stt record --write --stop-mode manual_only
```

**Write method** - how text is injected: **Auto** (default) tries the XDG Desktop Portal, then ydotool, then direct Wayland input. Force a specific one in Settings if auto-detection picks wrong.

## 🤖 Models

Models come from a **library** of backends you install on demand. Open the app, go to **Library → Browse**, install a backend, and it appears in the model selector. Some run **locally** (your audio never leaves your machine); others are **online** providers you reach with your own API, which is stored securely in your system keyring (GNOME Keyring, KWallet, …).

### Recommended models

**Local** - everything stays on your device.

| Model              | Best for                                              |
|--------------------|-------------------------------------------------------|
| **Voxtral** (mini / small) | High accuracy; needs an **NVIDIA GPU** (CUDA). |
| **Qwen3-ASR** (0.6b / 1.7b) | Fast, multilingual; runs on **CPU or an NVIDIA GPU** (CUDA). |
| **Whisper** (tiny → large) | Versatile and battle-tested; `tiny`/`base` are great **CPU** defaults. |

**Online** - bring your own API key.

| Provider   | Notable models                                       |
|------------|------------------------------------------------------|
| **Mistral**  | `voxtral-mini-latest`, plus a realtime Voxtral model |
| **OpenAI**   | `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, `whisper-1` |
| **Deepgram** | `nova-3`                                              |

> **GPU acceleration** is a property of the model, not a separate build of the app. Install a GPU-capable backend (like Voxtral or Qwen3-ASR) and the daemon downloads the build matched to your NVIDIA GPU automatically. You just need an up-to-date driver.

This is a snapshot. The app always shows the current catalog, published live at [`jorge-menjivar.github.io/super-stt/index.json`](https://jorge-menjivar.github.io/super-stt/index.json). Want a model that isn't there? Anyone can [publish one](./docs/protocol/README.md#add-your-own-model).

<table>
<tr>
<td align="center"><strong>Model selection</strong><br><img src=".github/assets/models-selection.png" width="320"></td>
<td align="center"><strong>Online providers</strong><br><img src=".github/assets/online-models.png" width="320"></td>
</tr>
</table>

## 🩺 Troubleshooting

- **`stt: command not found`**: restart your terminal or run `export PATH="$HOME/.local/bin:$PATH"`.
- **Daemon won't start / misbehaves**: check `journalctl --user -u super-stt -n 49`.
- **A stale legacy build is interfering** (auth popup shows `Path: <unknown>`, or errors mention an `stt` group): remove the old binaries and reinstall: `just uninstall` (or `rm -f ~/.local/bin/super-stt*`), then `just install`.


## 🧑‍💻 Developers

Super STT is built to be built on. The details live in developer-facing docs:

- **Build a client** — get transcriptions, event streams, or recording control into your own app, in any language, over the documented HTTP protocol → **[docs/protocol/](./docs/protocol/)**
- **Add your own model** — package a speech model as a backend the daemon can install and run, then publish it to the catalog → **[docs/protocol/](./docs/protocol/)** and **[registry/README.md](./registry/README.md)**
- **Contribute** — build from source, workspace layout, and the PR workflow → **[CONTRIBUTING.md](./CONTRIBUTING.md)**

Architecture and the security model live in **[docs/](./docs/)**.

---

<div align="center">

**Jorge Menjivar** • jorge@menjivar.ai

</div>
