<div align="center">

<img src=".github/assets/super-stt-icon.svg" width="128" height="128" alt="Super Voice">

# Super Voice

**Speak into any app on Linux. Your words appear as text.**

*One shortcut to dictate anywhere • Any model from a growing library • An open protocol any app can build on • Built in Rust*

[![coverage](https://img.shields.io/endpoint?url=https://jorge-menjivar.github.io/super-voice/coverage/coverage.json)](https://jorge-menjivar.github.io/super-voice/coverage/)

</div>

https://github.com/user-attachments/assets/bbbe20c3-6802-4797-afc8-aa81d1b48415

## 🚀 Installation

**Quick install** - detects your system and downloads pre-built binaries:

```bash
curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-voice/main/install.sh | bash
```

Append `-s -- --beta` for the latest beta.

## What is Super Voice?
Super Voice is speech-to-text and text-to-speech for Linux, with any model you want. Dictate into any app, hear text read aloud, and swap models anytime, local or cloud, on CPU or GPU.

Under the hood it's two things:

- **A model-agnostic engine.** A background daemon installs speech models from a **library** of backends (local or cloud), loads one, and keeps it warm for instant transcription. You pick the model that fits your hardware and swap it whenever you like.
- **An open protocol.** The daemon speaks a documented HTTP protocol over a local socket, so *any* app, in any language, can request transcriptions, stream live audio visualizations, or drive recording, with per-app consent. Super Voice's own desktop apps, CLI, and COSMIC applet are just the first clients. See [Developers](#-developers).

## Features?
- Speech-to-Text support.
- Text-to-Speech support.
- High performance. Built in Rust. Supports GPU acceleration for both STT and TTS.
- Native support for Linux.
- Bind a shortcut (Super+Space by convention), speak, and your words are typed straight into whatever app is focused, e.g. your editor, browser, chat, terminal. No copy-paste, no fiddling.
- It allows you to speak text (synthesize) high quality audio using state-of-the-art models without any extra setup.
- Clone your own voice and use it to speak text.
- You can choose the model to use from a growing library of backends. You can [create and use your own backend](./docs/protocol/README.md#add-your-own-model) too.


## ⌨️ Using it

Open **Super Voice** from your app launcher. Everything starts there:

1. **Models:** pick a model for speech to text and one for text to speech, or install one from **Library → Browse**.
2. **Speech to text → Recording:** press **Test Recording**, speak, and the transcription appears in the app.
3. **Text to speech → Speech:** type something and press **Speak** to hear it.
4. **Text to speech → Voices:** record or import a sample to clone your own voice.
5. **Speech to text → Contexts:** say what you're dictating and list the words it should get right, like names and product terms.

### Dictate anywhere with a shortcut

Press **Super+Space**, speak, and your words are typed into whatever app is focused: your editor, browser, chat or terminal. The installer binds it for you on COSMIC. On other desktops, add a custom shortcut for `voice record --write`.

1. The shortcut asks the daemon to start transcribing from your mic.
2. You speak, and the daemon transcribes your audio.
3. When you stop, on silence or by pressing the shortcut again, it types the transcription into the focused app.

### From the command line

The shortcut runs the `voice` command, which you can also use directly:

```bash
voice record --write   # Record and transcribe. Types the result after silence, or when you run it again.
voice speak "Hello"    # Speak text with the loaded model. `voice speak stop` stops it.
```

Manage the daemon with the usual systemd controls:

```bash
systemctl --user start super-voice      # or: enable / status / restart
journalctl --user -u super-voice -f     # follow logs
```

## 🤖 Models

Models come from a **library** of backends you install on demand. Open the app, go to **Library → Browse**, install a backend, and it appears in the model selector. Some run **locally** (your audio never leaves your machine); others are **online** providers you reach with your own API key.

> **GPU acceleration** is a property of the model, not a separate build of the app. Install a GPU-capable backend (like Voxtral or Qwen3-ASR) and the daemon downloads the build matched to your GPU automatically. You just need an up-to-date driver.

Want a model that isn't there? Anyone can [publish one](./docs/protocol/README.md#add-your-own-model).

## 🖼️ Screenshots

**Models.** Select a backend, pick a model, and choose where it runs.

<table>
<tr>
<td align="center"><strong>Clean start</strong><br><img src=".github/assets/screenshots/app-1.png" width="270"></td>
<td align="center"><strong>Select a backend</strong><br><img src=".github/assets/screenshots/app-2.png" width="270"></td>
<td align="center"><strong>Pick a model</strong><br><img src=".github/assets/screenshots/app-3.png" width="270"></td>
</tr>
<tr>
<td align="center"><strong>Choose where it runs</strong><br><img src=".github/assets/screenshots/app-4.png" width="270"></td>
<td align="center"><strong>Select a language</strong><br><img src=".github/assets/screenshots/app-5.png" width="270"></td>
<td></td>
</tr>
</table>

**Library.** Browse the catalog and manage what's installed.

<table>
<tr>
<td align="center"><strong>Installed backends</strong><br><img src=".github/assets/screenshots/app-6.png" width="400"></td>
<td align="center"><strong>Browse the catalog</strong><br><img src=".github/assets/screenshots/app-7.png" width="400"></td>
</tr>
</table>

**Speech to text.** Set how recording stops, how text is typed, and what you are dictating.

<table>
<tr>
<td align="center"><strong>Recording</strong><br><img src=".github/assets/screenshots/app-8.png" width="400"></td>
<td align="center"><strong>Input simulation</strong><br><img src=".github/assets/screenshots/app-9.png" width="400"></td>
</tr>
<tr>
<td align="center"><strong>Contexts</strong><br><img src=".github/assets/screenshots/app-10.png" width="400"></td>
<td align="center"><strong>Edit a context</strong><br><img src=".github/assets/screenshots/app-11.png" width="400"></td>
</tr>
</table>

**Text to speech.** Hear a test line, and make voices from your own recordings.

<table>
<tr>
<td align="center"><strong>Speech</strong><br><img src=".github/assets/screenshots/app-12.png" width="400"></td>
<td align="center"><strong>Voices</strong><br><img src=".github/assets/screenshots/app-13.png" width="400"></td>
</tr>
</table>

**Settings.** Tune audio feedback, check the connection, and stay up to date.

<table>
<tr>
<td align="center"><strong>Customization</strong><br><img src=".github/assets/screenshots/app-14.png" width="270"></td>
<td align="center"><strong>Connection</strong><br><img src=".github/assets/screenshots/app-15.png" width="270"></td>
<td align="center"><strong>Updates</strong><br><img src=".github/assets/screenshots/app-16.png" width="270"></td>
</tr>
</table>

**Panel visualizer.** The COSMIC applet shows your mic input live in the panel, in three styles.

<p align="center"><strong>Waveforms</strong><br><img src=".github/assets/screenshots/visualization-waveforms.png" width="100%"></p>
<p align="center"><strong>Equalizer</strong><br><img src=".github/assets/screenshots/visualization-equalizer.png" width="100%"></p>
<p align="center"><strong>Centered bars</strong><br><img src=".github/assets/screenshots/visualization-centered-bars.png" width="100%"></p>

## 🩺 Troubleshooting

- **`voice: command not found`**: binaries are installed system-wide and are on `PATH` by default — restart your terminal so it rehashes, and check the installer finished without errors.
- **Daemon won't start / misbehaves**: check `journalctl --user -u super-voice -n 50`.
- **Transcriptions seem less accurate than they should be**: your microphone input volume may be set too high or too low. Adjust the mic volume in your system sound settings and try again.
- **Typing doesn't work in some apps**: [`ydotool`](https://github.com/ReimuNotMoe/ydotool) types reliably across virtually all apps. Install it via your package manager, then try it out first by running `sudo ydotoold --socket-path="$HOME/.ydotool_socket" --socket-own="$(id -u):$(id -g)"` in a terminal and setting the write method to **ydotool** in Settings. If that fixes typing, make it permanent by enabling the service (e.g. `systemctl --user enable --now ydotool`).

## 🧑‍💻 Developers

Super Voice is built to be built on. The details live in developer-facing docs:

- **Build a client** — get transcriptions, event streams, or recording control into your own app, in any language, over the documented HTTP protocol → **[docs/protocol/](./docs/protocol/)**, with every endpoint in the **[API reference](https://jorge-menjivar.github.io/super-voice/protocol/)**
- **Add your own model** — package a speech model as a backend the daemon can install and run, then publish it to the catalog → **[docs/protocol/](./docs/protocol/)** and **[registry/README.md](./registry/README.md)**
- **Contribute** — build from source, workspace layout, and the PR workflow → **[CONTRIBUTING.md](./CONTRIBUTING.md)**

Architecture and the security model live in **[docs/](./docs/)**.

---

<div align="center">

**Jorge Menjivar** • jorge@menjivar.ai

</div>
