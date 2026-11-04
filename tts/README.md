<div align="center">

<img src=".github/assets/super-tts-icon.svg" width="128" height="128" alt="Super TTS">

# Super TTS

**Give any app on Linux a voice.**

*Speak text from anywhere • Any model from a growing library • An open protocol any app can build on • Built in Rust*

[![coverage](https://img.shields.io/endpoint?url=https://super-libre.github.io/super-tts/coverage/coverage.json)](https://super-libre.github.io/super-tts/coverage/)

</div>

## What is Super TTS?

Super TTS makes text-to-speech **trivial on Linux, for everyone**. Send it text and it speaks — from a shell pipeline, a keyboard shortcut, or an app that streams an LLM's reply as it is generated.

Under the hood it's two things:

- **A model-agnostic engine.** A background daemon installs voice models from a **library** of backends (local or cloud), loads one, and keeps it warm so speech starts immediately. You pick the model that fits your hardware and swap it whenever you like.
- **An open protocol.** The daemon speaks a documented HTTP protocol over a local socket, so *any* app, in any language, can make the machine talk, follow playback, or stream live audio visualizations — with per-app consent. Super TTS's own desktop app, CLI, and COSMIC applet are just the first clients. See [Developers](#-developers).

## 🚀 Installation

**Quick install** - detects your system and downloads pre-built binaries:

```bash
curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-tts/main/install.sh | bash
```

Append `-s -- --beta` for the latest beta.

**Build from source** - for the very latest changes (needs the [build prerequisites](./CONTRIBUTING.md#prerequisites)):

```bash
git clone https://github.com/jorge-menjivar/super-tts.git
cd super-tts
just install
```

Either way you get the daemon, the `tts` CLI, a consent helper, the desktop app, and (on COSMIC) the panel applet, wired up as a `systemctl --user` service. Everything installs system-wide (root-owned, so the installer asks for sudo), while the daemon itself runs unprivileged in your user session. GPU acceleration comes from the model you run (see [Models](#-models)).

## ⌨️ Using it

```bash
tts speak "The kettle is boiling."   # speak some text
echo "Hello there." | tts speak      # …or pipe it in
tts stop                             # fall silent
```

`tts speak` returns as soon as the audio is queued — the daemon keeps talking after the command exits. A second `speak` interrupts the first, which is what "say this instead" means; `tts stop` silences it outright.

Both are worth binding to a key combo, `tts stop` especially.

Manage the daemon with the usual systemd controls:

```bash
systemctl --user start super-tts      # or: enable / status / restart
journalctl --user -u super-tts -f     # follow logs
```

### Choosing how it sounds

Speaking and configuring are separate. `tts speak` takes the words and nothing
else; which voice and which language the machine uses are settings, chosen once
in the app (or over the protocol under the `settings` scope) and used by every
utterance afterwards — a keyboard shortcut, the applet, your own client.

That is a deliberate split: an app you allow to make the machine talk does not
thereby get to pick the voice it talks in.

### Long text and streaming

Long text is split on sentence boundaries so playback starts before the whole thing is synthesized. Markup that would otherwise be read out literally — emphasis markers, code fences, link targets, table pipes — is stripped first; numbers, dates, and currency are left exactly as written, because every model in scope reads them correctly and a wrong number-to-words is worse than none.

An app generating text can stream it in as it arrives over a WebSocket, and the daemon speaks each sentence as it completes — so an LLM's opening line is heard while it is still writing the third. See [`/speak/stream`](./docs/protocol/endpoints/v1/speak/stream.md).

## 🎙️ Cloned voices

Models that support it can speak in a voice you supply. Open the app's **Voices** page, record a sample or import a WAV, name it, and it joins your library:

Select it on that page and everything the daemon says afterwards is in your
voice — `tts speak "Read this in my voice."`, the applet, and any app you have
allowed to speak.

The page offers a script to read while it records, in whichever language the daemon is set to speak, so there is nothing to improvise and nothing to type: the words you read become the clip's transcript. Each language's default is sized to fit inside the loaded model's reference budget, so the clip and its transcript describe the same audio; longer scripts are offered too, and say so when the model cannot hear all of one. One of them covers every sound in English. Pick "No script" to record in your own words instead.

A voice is stored once and independently of any model — each model takes as much of the recording as it declares it can use, so switching models never means recording again. Samples live in `~/.local/share/super-tts/voices`, readable only by you, and reach nothing but the loaded model. Apps ask for them under a scope of their own: an app allowed to change every setting still cannot read your recordings unless you approve that separately.

See [`/voices`](./docs/protocol/endpoints/v1/voice.md) for the endpoints, and the [`voices` scope](./docs/protocol/scopes/voices.md) for what granting it means.

## 🤖 Models

Models come from a **library** of backends you install on demand. Open the app, go to **Library → Browse**, install a backend, and it appears in the model selector. Some run **locally** (your text never leaves your machine); others are **online** providers you reach with your own API key, which is stored securely in your system keyring (GNOME Keyring, KWallet, …).

> **The catalog is empty right now.** Super TTS is a new engine and no voice backends have been published to it yet. The engine, the protocol, and the installer all work end to end — there is simply nothing in the library to install until the first backends ship. If you want to be first, anyone can [publish one](./docs/protocol/README.md#add-your-own-model).

> **GPU acceleration** is a property of the model, not a separate build of the app. Install a GPU-capable backend and the daemon downloads the build matched to your GPU automatically. You just need an up-to-date driver.

The app always shows the current catalog, published live at [`jorge-menjivar.github.io/super-tts/index.json`](https://jorge-menjivar.github.io/super-tts/index.json).

## 🖥️ The desktop app

The app is where you manage everything the CLI doesn't ask about. Its pages:

| Page | What it's for |
|------|---------------|
| **Models** | Pick the model to keep loaded, choose CPU or GPU, and set its language. Shows what's warm right now. |
| **Library** | Browse the catalog, install and remove backends, and enter API keys for online providers. |
| **Voices** | Record a reference clip against a built-in script (or import a WAV) and manage your cloned voices. |
| **Speech** | Playback volume and the audio cues the daemon plays. |
| **Customization** | Appearance, and the applet's visualization style. |
| **Connection** | Which apps hold a session token, what they may do, and revoking them. |
| **Updates** | Check for a new release and opt into betas. |

On COSMIC, the panel applet adds a live visualizer driven by whatever is currently
being spoken — **waveform**, **centered equalizer**, **bottom equalizer**, or
**pulse** — plus a working animation while a model is loading. It can sit on
either side of the panel, or span it in full.

## 🩺 Troubleshooting

- **`tts: command not found`**: binaries are installed system-wide and are on `PATH` by default — restart your terminal so it rehashes, and check the installer finished without errors.
- **Daemon won't start / misbehaves**: check `journalctl --user -u super-tts -n 50`.
- **`tts speak` succeeds but nothing is heard**: the command returns as soon as the audio is queued, so a silent success means either no model is loaded (`tts status` will say so) or the output device is muted. Check your system volume and the daemon log.
- **Speech is choppy on a remote model**: the daemon starts playing as soon as it has enough audio buffered, so a slow provider can underrun. A local model removes the round trip.

## 🧑‍💻 Developers

Super TTS is built to be built on. The details live in developer-facing docs:

- **Build a client** — make the machine talk, follow playback, or stream text in from your own app, in any language, over the documented HTTP protocol → **[docs/protocol/](./docs/protocol/)**
- **Add your own model** — package a voice model as a backend the daemon can install and run, then publish it to the catalog → **[docs/protocol/](./docs/protocol/)** and **[registry/README.md](./registry/README.md)**
- **Contribute** — build from source, workspace layout, and the PR workflow → **[CONTRIBUTING.md](./CONTRIBUTING.md)**

Architecture and the security model live in **[docs/](./docs/)**.

---

<div align="center">

**Jorge Menjivar** • jorge@menjivar.ai

</div>
