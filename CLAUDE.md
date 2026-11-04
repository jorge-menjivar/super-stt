# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Super Voice is a speech-to-text and text-to-speech service built in Rust with a daemon/client architecture. One daemon serves both features: it loads models once in memory so transcription and speech answer at once. Speech to text and text to speech are features of one product, not two products: one socket, one token per client, one config file. Each feature's own routes are under `/v1/stt` and `/v1/tts`; what the daemon has once is at `/v1`.

## Workspace Structure

- **super-voice-daemon**: the one daemon. Speech to text is its top-level modules; text to speech is its `tts` module.
- **super-voice-consent**: the consent dialog the daemon shows when a client asks for a token.
- **super-voice-registry-types**: the one backend manifest spec (`backend.toml`) and the registry index, for both features.
- **super-voice-indexer**: builds the registry index (CI only).
- **super-stt-app**, **super-stt-cli**, **super-stt-shared**: speech to text's settings app, CLI and client library.
- **super-tts-app**, **super-tts-cli**, **super-tts-shared**: text to speech's. The two apps and the two CLIs merge later.
- **super-stt-install**: the installer and self-updater.

The COSMIC applet lives in super-libre/super-cosmic-applet. Super TTS's repository was imported under `tts/`; what is still there is being merged.
