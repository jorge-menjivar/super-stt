# Contributing to Super STT

Thank you for your interest in contributing to Super STT! This document provides guidelines for contributing to the project.

## Development Setup

### Prerequisites

- A recent stable Rust toolchain (edition 2024).
- The [`just`](https://github.com/casey/just) task runner: `cargo install just`.
- System build dependencies, by distro:

  ```bash
  # Debian/Ubuntu/Pop!_OS
  sudo apt install build-essential libxkbcommon-dev libasound2-dev pkg-config libssl-dev
  # Fedora
  sudo dnf install gcc gcc-c++ libxkbcommon-devel alsa-lib-devel pkgconf perl-FindBin perl-IPC-Cmd openssl-devel
  # Arch
  sudo pacman -S pkgconf openssl
  ```

  If a dependency is missing for your distro, a PR to update this list is welcome.

- On **macOS**, the Xcode command line tools (`xcode-select --install`) are the
  only system dependency — CoreAudio, CoreGraphics and the Security framework
  all ship with the OS.

### Clone and build

```bash
git clone https://github.com/jorge-menjivar/super-stt.git
cd super-stt

just install            # build and install everything, wired to systemd
# …or one piece at a time:
just install-daemon
just install-app
just install-applet     # COSMIC only
```

#### On macOS

Everything builds here except the two Linux shell components. The panel
applet needs libcosmic's `applet` feature, which reaches
`cosmic-panel-config` -> `smithay-client-toolkit` -> `xkbcommon` via
pkg-config, and `cosmic-panel-config` is the one crate in that chain with no
`target_vendor = "apple"` gate. The consent helper takes `wayland` and is a
layer-shell overlay, which has no macOS counterpart; the daemon uses
`osascript` there instead, so whether it would still compile is untested and
moot.

libcosmic itself is portable, which is easy to miss: its vendored iced fork
puts every Wayland crate behind that same Apple gate, and has a
`cfg(target_os = "windows")` table beside it. The settings app builds and
runs on macOS once its libcosmic feature list drops `applet` and the
Wayland/D-Bus features — see the two `[target.…]` tables in
`super-stt-app/Cargo.toml`.

So a plain `cargo build`/`cargo test` over the workspace still fails on macOS
by design — use the platform-scoped recipes, which select the members that do
build:

```bash
just install-daemon     # daemon + CLI, installed as a LaunchAgent
just check-macos        # clippy over the members that build here
just test-macos         # their tests
just doctest-macos      # their doctests
just ci-macos           # all of the above, the way CI runs it
```

`just check`, `just test`, `just doctest` and `just ci` remain the
whole-workspace (Linux) gates. CI runs both sets, so a change that builds on
one platform and not the other is caught before merge.

The recipes that build a COSMIC shell component stop with an explanation here
rather than failing deep inside a Wayland dependency's build script, and
`just install-app` stops separately because the app builds but has no `.app`
bundle to install into. Recipes that cover both platforms — `just
run-daemon`, `just install` — quietly do the right thing instead: they skip
the parts that have no macOS counterpart.

##### The global shortcut

`stt hotkey` (`super-stt-cli/src/hotkey.rs`) is the macOS stand-in for a
desktop environment's custom shortcut, and `just install-daemon` runs it as a
second LaunchAgent. To try a change without reinstalling:

```bash
just run-cli hotkey --key ctrl+alt+shift+F19
```

Pick a binding the installed listener is not already holding, or stop it
first with `launchctl bootout gui/$(id -u)/ai.menjivar.super-stt.hotkey`.

Two constraints shape that module. The crate's macOS backend needs an event
loop on the main thread, so `hotkey` is dispatched before `main` builds a
tokio runtime, and the runtime lives on worker threads instead. And presses
are handled strictly one at a time: `cmd_record` decides start-or-stop from
the daemon's `busy` flag, so two presses in flight together would both see
"idle" and both try to start.

To exercise it without a daemon, a microphone or your Keychain, point it at a
socket that does not exist and use the in-memory keyring — a press then logs
`pressed` followed by a connection error:

```bash
SUPER_STT_KEYRING_MOCK=1 RUST_LOG=super_stt_cli=debug \
  ./target/debug/super-stt-cli --socket /tmp/nope.sock hotkey --key ctrl+alt+shift+F19
```

##### Sign your local builds, or macOS will forget every permission

Set this up before you spend an afternoon fighting permissions:

```bash
export SUPER_STT_SIGN_IDENTITY="Super STT Dev"
```

macOS ties Accessibility and Microphone grants, and Keychain ACLs, to a
binary's *designated requirement*. The signature the Rust linker applies is
ad-hoc, and an ad-hoc requirement is the content hash — so every `cargo
build` produces what macOS considers a different program from the one you
granted. What you actually see:

- The Accessibility pane lists `super-stt-daemon` with its switch **on**
  while the daemon insists it is not allowed to control the keyboard. The row
  is keyed by path, so it survives; the requirement behind it does not.
- The Keychain re-prompts no matter how many times you click Always Allow.
- The daemon's `exe_changed` session revocation fires on every rebuild, and
  the client retries against a session it can never get back.

None of that is a bug in Super STT, and none of it is fixable in code.

With the variable set, every `just run-daemon`, `just run-app`, `just
run-cli` and `just build-*` re-signs its output with a stable identity, and
grants stick across rebuilds. Leave it unset and nothing changes — the step
is a no-op off macOS and when it is empty, so Linux and CI are unaffected.

Why it works, in one comparison. `codesign -d --requirements -` on the same
binary, signed each way:

```
ad-hoc (what the linker gives you)
  designated => cdhash H"828ac4ed4175c86a6aa18c6b618d923a22eafca5"

Developer ID
  designated => identifier "ai.menjivar.super-stt-daemon"
                and anchor apple generic
                and certificate leaf[subject.OU] = "8JDTBFQ3F5"
```

The ad-hoc requirement *is* the content hash, which is why it dies on every
build. The signed one names an identifier and a team, and holds.

**Use a Developer ID if you have one.** It is already trusted, and a grant
you establish with it carries over to real signed builds instead of having
to be redone:

```bash
security find-identity -v -p codesigning
export SUPER_STT_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
```

Without one, a self-signed certificate works for local development —
Keychain Access -> Certificate Assistant -> Create a Certificate, type "Code
Signing". One trap: it will not appear under `find-identity -v` until you
mark it trusted (double-click the certificate -> Trust -> Code Signing ->
Always Trust). Until then `find-identity` without `-v` reports it as
`CSSMERR_TP_NOT_TRUSTED` and signing fails.

Either way this is the development loop only. Neither signature is
notarized, so neither says anything to Gatekeeper on someone else's machine;
shipping is separate work.

After signing for the first time, remove the stale Accessibility entry with
`-` and re-add the binary — the recorded requirement is only rewritten on
re-add, not on toggling the switch.

### Development commands

```bash
just run-daemon         # run the daemon in the foreground
just run-app            # run the settings app
just run-applet         # run the COSMIC applet
just audit              # security audit (cargo audit)
```

## Workspace layout

Super STT is a Rust workspace:

| Crate                      | Role                                                              |
|----------------------------|------------------------------------------------------------------|
| `super-stt-daemon`         | The engine: installs backends, loads models, serves the protocol |
| `super-stt-app`            | Desktop settings & management app                                |
| `super-stt-cli`            | The `stt` command-line client                                    |
| `super-stt-cosmic-applet`  | COSMIC panel applet with visualizations                          |
| `super-stt-consent`        | Consent-popup helper for the auth handshake                      |
| `super-stt-shared`         | Common types, protocol definitions, validation                   |
| `super-stt-registry-types` | Shared backend registry / manifest types                         |
| `super-stt-forge`          | Git-forge release sourcing for the registry                      |
| `super-stt-indexer`        | CI tool that builds the published registry `index.json`          |

The protocol and backend contract that clients and backend authors build
against live in [`docs/protocol/`](./docs/protocol/).

## Code Style and Standards

- **Rust**: Follow standard Rust conventions and use `cargo fmt`
- **Security**: All external inputs must be validated using the shared validation framework
- **Testing**: Add tests for new functionality, especially security-critical code
- **Documentation**: Document public APIs and security-relevant functions

## Security Guidelines

- Never bypass the process authentication system
- All network communication must validate inputs
- Use the shared validation framework in `super-stt-shared/src/validation/`
- Follow the development vs production security model (debug vs release builds)
- Run security audits before proposing changes: `cargo audit`

## Pull Request Process

1. **Before submitting**:
   - Run `cargo test` to ensure all tests pass
   - Run `cargo fmt` to format code
   - Run `cargo clippy` to check for warnings
   - Run `cargo audit` to check for security vulnerabilities
   - Test on both debug and release builds

2. **Pull Request Requirements**:
   - Clear description of changes
   - Reference any related issues
   - Include tests for new functionality
   - Update documentation if needed

3. **Review Process**:
   - All PRs require review
   - Security-related changes require additional scrutiny
   - CI must pass before merging

## Reporting Security Issues

If you discover a security vulnerability, please:

1. **Do not** open a public issue
2. Email security concerns to: jorge@menjivar.ai
3. Include detailed reproduction steps
4. Allow reasonable time for response before public disclosure

## Code of Conduct

- Be respectful and inclusive
- Focus on constructive feedback
- Help maintain a welcoming environment for all contributors

## License

By contributing to Super STT, you agree that your contributions will be licensed under the GPL-3.0-only license.

## Questions?

- Open an issue for feature requests or bugs
- Join discussions in existing issues
- Contact: jorge@menjivar.ai
