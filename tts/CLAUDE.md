# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Super TTS is a high-performance text-to-speech service built in Rust with a daemon/client architecture. The system loads AI models once in memory so speech starts immediately.

## Workspace Structure

This is a Rust workspace with 4 main crates:

- **super-tts-app**: Desktop application used to configure and manage Super TTS.
- **super-tts-applet**: COSMIC desktop environment extension/applet that has visualization capabilities.
- **super-tts-daemon**: Background service that loads and runs ML models
- **super-tts-shared**: Common types, protocol definitions, and utilities
