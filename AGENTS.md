# Repository Guidelines

## Project Structure & Module Organization
- Workspace root (`Cargo.toml`) with members:
  - `super-stt`: speech-to-text daemon (ML, audio, D-Bus, model mgmt).
  - `super-stt-app`: desktop UI (COSMIC/iced).
  - `super-stt-cosmic-applet`: panel/applets and COSMIC extension.
  - `super-stt-shared`: shared models, protocol, utils.
- Tests/tooling: Python scripts at repo root (e.g., `test_download_progress.py`).
- Assets: app `i18n/`, `resources/`; applet `data/` (desktop entries, icons).

## Build, Test, and Development Commands
- Build release: `just build-release` (or `cargo build --release`).
- Lint (clippy, pedantic): `just check` (or `cargo clippy --all-features`).
- Run UI app: `just run-app`.
- Run daemon: `just run-daemon --model whisper-small` (flags: `--cuda`, `--cudnn`).
- Run COSMIC applets: `just run-applets`.
- Install locally: `just install-daemon`, `just install-app`, `just install-applets`.
- Status/logs: `just status`, `just logs-daemon`.
- Offline/vendor build: `just build-vendored`.

## Coding Style & Naming Conventions
- Rust edition `2024`; 4-space indent.
- Format before pushing: `cargo fmt --all`.
- Lint before pushing: `just check`.
- Modules use snake_case; crates/binaries use kebab-case (e.g., `super-stt-daemon`).
- Format TOML with Taplo (`taplo.toml`) when editing manifests.

## Testing Guidelines
- Runtime/UX tests: Python against daemon socket at `/run/user/$UID/stt/super-stt-http.sock`.
  - Example: `python3 test_download_progress.py` (ensure daemon is running).
- Rust unit tests live next to code (`#[cfg(test)]`) or in `tests/` per crate.
- For protocol changes, add tests in `super-stt-shared` first.

## Commit & Pull Request Guidelines
- Commits: prefer `type(scope): summary` (e.g., `feat(daemon): track model download progress`).
  - Short WIP tags (e.g., `WIP-116`) may appear in history.
- PRs include purpose, linked issues, testing steps/commands, and UI screenshots (app/applets). Mention affected crates and update docs/`justfile` if behavior changes.

## Security & Configuration Tips
- Daemon runs as a user service (`systemd --user`) and exposes a Unix socket under `/run/user/$UID/stt/`.
- Some ML deps may reference local paths (e.g., `../candle-2/*`); document deviations in PRs.
- Do not commit large model files; use daemon-managed downloads.
