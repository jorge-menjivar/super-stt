# Super Voice Security Model

## Overview

Super Voice implements a comprehensive defense-in-depth security model to protect against unauthorized access while maintaining usability. The system uses multiple security layers including a Unix domain socket restricted to the user running the daemon, process authentication for access to the keyboard and the speakers, and input validation throughout.

## Security Architecture Summary

Based on comprehensive security reviews completed in August 2025, Super Voice demonstrates **excellent security posture** with:

- ✅ **Process Authentication**: Robust authentication for every privileged operation, keyboard injection and speech included
- ✅ **Input Validation**: Comprehensive framework with DoS protection and attack detection
- ✅ **Memory Safety**: Excellent use of Rust's safety guarantees
- ✅ **Network Security**: Nothing listens off this machine — a per-user Unix socket, and a TCP port bound to loopback only
- ✅ **Resource Management**: Connection limits and rate limiting prevent abuse
- ✅ **Path Security**: Comprehensive validation prevents directory traversal attacks
- ✅ **Tamper-Resistant Install**: Binaries and the systemd user unit are installed root-owned in system paths; the daemon itself runs unprivileged in the user session

**Security Assessment**: Production-ready with excellent security controls implemented.

## Socket Access Control

The daemon listens on a Unix domain socket at
`<runtime dir>/voice/super-voice-http.sock`. Access is restricted to the user
running the daemon — there is **no** shared group and no group membership
to configure. Two independent layers enforce this:

1. **Per-user runtime directory.** The directory is created by the system with
   `0700` permissions, so no other local user can even traverse into it to
   reach the socket. On Linux that is `$XDG_RUNTIME_DIR` (typically
   `/run/user/<uid>`); on macOS it is the Darwin per-user temp directory
   (`/var/folders/<xx>/<hash>/T`, what `getconf DARWIN_USER_TEMP_DIR` prints),
   which the OS likewise creates `0700` and owned by the user. The daemon
   reads the macOS one from `confstr(_CS_DARWIN_USER_TEMP_DIR)` rather than
   `$TMPDIR` so an inherited or overridden environment variable cannot move
   where it binds.
2. **Same-UID peer check.** Every request is checked against the connecting
   peer's UID, which the kernel supplies (`SO_PEERCRED` on Linux,
   `LOCAL_PEERCRED` on macOS); a peer whose UID differs from the daemon's is
   rejected. A process that somehow reached the socket still cannot use it
   unless it runs as the same user.

### Socket permissions

| Build | Mode | Notes |
| --- | --- | --- |
| Release (default) | `0660` | Owner + the daemon user's primary group. The socket is **not** chgrp'd to any shared group. |
| Debug (`cargo build`) | `0666` | Local development only; the `0700` runtime dir still keeps it unreachable by other users. |

### Authorization

Reaching the socket is not the same as being authorized. Privileged operations
(keyboard injection, speaking, the voice library, settings, secrets) require a per-request session token
carrying user-approved scopes, minted through a one-time consent prompt and
bound to the client's executable path. See
[`protocol/auth.md`](protocol/auth.md) for the token and scope model.

The executable path itself comes from the kernel, not from the caller:
`/proc/<pid>/exe` on Linux, `proc_pidpath` on macOS. **One guarantee is
weaker on macOS.** Linux renders a replaced-on-disk binary as
`/path/to/exe (deleted)`, so a token bound to it stops matching and is
revoked; `proc_pidpath` reports only the path, and a path whose file was
swapped still resolves. Swapping it requires write access to the install
directory — the same access needed to replace the daemon itself — so this
opens no new door, but the exe-change revocation is a Linux-only additional
check rather than a cross-platform one.

### Verification
```bash
ls -la "${XDG_RUNTIME_DIR:-$(getconf DARWIN_USER_TEMP_DIR)}/voice/super-voice-http.sock"
# srw-rw---- ... <you> <your-primary-group> ... super-voice-http.sock
```

## Security Features

### 1. Keyboard Input Protection
- **Consent-gated authorization**: Auto-typing is the per-request `write_mode` flag on `POST /stt/transcribe` (or the daemon's configured write mode). Reaching that endpoint at all requires a consent-minted session token with the `transcribe` scope — so a client can only type after the user approved it through the one-time consent prompt. There is no separate keyboard/write scope; it is the same `transcribe` scope dictation uses. See [Authorization](#authorization) and [`protocol/auth.md`](protocol/auth.md)
- **Peer identity for the prompt**: the kernel's peer credentials plus its record of the peer's executable (`SO_PEERCRED` + `/proc/<pid>/exe` on Linux, `LOCAL_PEERCRED` + `proc_pidpath` on macOS) tell the consent prompt *which binary* is asking and reject cross-UID peers; this identifies the caller, it is not by itself the authorization
- **Debug-only test bypass**: The consent auto-approval used by tests/CI (`SUPER_VOICE_AUTO_APPROVE`) is compiled out of release builds
- **Output sanitization**: Backend transcription output is untrusted, so before it is typed the daemon strips non-whitespace control codes (ESC/BEL/NUL/backspace), Unicode bidi overrides, and zero-width characters — a terminal escape sequence or a bidi spoof in a transcript can't reach the focused window
- **Limited scope**: Only types actual transcription results

### 2. Audio Output Protection
- **Consent-gated authorization**: Making the machine speak requires a consent-minted session token with the `speak` scope — so a client can only reach the speakers after the user approved it through the one-time consent prompt. See [Authorization](#authorization) and [`protocol/auth.md`](protocol/auth.md)
- **Interruption is part of the grant**: one utterance plays at a time and the newest wins, so a `speak`-scoped client can cut off whatever else is being read aloud, and can silence it with `/tts/speak/stop`. The consent prompt says so in those words ("Interrupt or stop speech this or another app started") rather than leaving it to be discovered
- **Untrusted backend audio**: a synthesis backend's framed response is checked as it arrives — its declared format, and the size caps under [Input Validation](#6-input-validation-and-dos-protection) — and a response that fails is refused rather than played

### 3. Voice Library Privacy
- **A scope of its own**: the cloned-voice library — recordings of the user's own voice — is reachable only with the `voices` scope. It is separate from `speak`, so an app that only talks never sees the recordings, and the consent prompt names it in those terms ("List, play back, and delete your saved voice recordings")
- **Owner-only files**: each clip is written to `$XDG_DATA_HOME/super-voice/voices` with mode `0600`
- **Stays on the machine**: a clip goes nowhere except to the loaded synthesis backend, which is local unless the user selected an online model. See [`protocol/scopes/voices.md`](protocol/scopes/voices.md)

### 4. Network Isolation
- **No network-facing listener**: The daemon listens on a per-user Unix domain socket under the runtime directory's `voice/`, and — for browser clients, on by default — on `127.0.0.1:7300`. It binds no other address and no UDP port, so no other host on the network can connect to it. A TCP client has no peer credentials, so it is identified by its origin instead; see [`protocol/auth.md` — TCP-bound clients](protocol/auth.md#tcp-bound-clients). `http.tcp` turns the TCP listener off
- **Local visualization**: Audio-visualization frames are delivered as Server-Sent Events over that same Unix socket, not broadcast on the network
- **Outbound only for backend installs**: The daemon reaches the network solely to fetch backends over HTTPS from the registry / GitHub — see [Daemon outbound network surface](#daemon-outbound-network-surface) below

### 5. Consent &amp; Peer Identity
- **Session tokens + scopes**: Every request other than `/auth/request` requires a `Bearer` session token; each token carries the user-approved scopes that gate what it may do (`transcribe`, `speak`, `voices`, `settings`, `secrets`, `status`, and the event-topic scopes, `playback_events` among them — see [`protocol/auth.md`](protocol/auth.md) for the full catalog)
- **One-time consent**: Tokens are minted only after a popup naming the requesting binary (resolved from the kernel's peer credentials) and the scopes it asks for, which the user approves or denies. The popup is the `super-voice-consent` helper on Linux and an `osascript` dialog on macOS, where the helper's Wayland layer-shell surface has no counterpart; both render the same scope descriptions, from `super_stt_shared::consent`
- **Same-UID only**: A peer whose UID differs from the daemon's is rejected before any prompt
- See [`protocol/auth.md`](protocol/auth.md) for the full token, scope, and consent contract

### 6. Input Validation and DoS Protection
- **Comprehensive validation**: All external inputs validated with strict limits
- **Audio data protection**: Maximum 30 minutes of audio at 16kHz to prevent memory exhaustion
- **Sample rate validation**: Must be between 8kHz and 96kHz
- **Text length cap**: `POST /tts/speak` accepts at most 50 000 characters, and so does one `/tts/speak/stream` session in total. A model's `max_input_chars` bounds each request the daemon sends its backend; the daemon splits the text on sentence boundaries to stay under it
- **Synthesis response caps**: a backend's framed audio response is bounded at 1 MiB per frame and 32 MiB per utterance, so a backend cannot exhaust memory by streaming without end
- **Voice upload caps**: a cloned-voice recording is refused above 48 MiB, before it is read, or above 120 seconds; its label is capped at 128 characters
- **JSON protection**: Size (1MB) and nesting depth (10 levels) limits prevent JSON bomb attacks
- **Suspicious pattern detection**: Detects potential padding attacks in audio data
- **Control character filtering**: Prevents injection of dangerous characters

### 7. Resource Management and Rate Limiting
- **Connection limits**: At most 40 concurrent connections, in every build
- **Rate limiting**: Per-client request limits with sliding window tracking: 600 requests a minute and 14,400 an hour, in every build. One daemon serves both features, so it leaves room for two apps, two CLIs and an applet
- **Connection timeouts**: Automatic cleanup of inactive connections (3 minutes production, 10 minutes development)
- **Memory protection**: Resource limits prevent memory exhaustion attacks

### 8. Process Isolation
- **User service**: Runs under user account, not root
- **Per-user socket**: An owner-only runtime directory plus a same-UID peer check — no cross-user access, no shared group
- **Socket permissions**: Enforced at filesystem level
- **Peer identity**: The kernel resolves the peer UID (same-UID enforced) and exe path (shown in the consent prompt) — the authorization itself is the consent-minted session token and its scopes

## Daemon outbound network surface

Prior to the backend registry (see
`docs/superpowers/specs/2026-05-29-backend-registry-design.md`), the daemon
process made no outbound network connections; only wasm-backend modules
issued HTTPS via `wasi:http`, constrained by per-backend `allowed_hosts`.

After the registry work, the daemon process itself opens HTTPS to:

- `jorge-menjivar.github.io` — registry index (`index.json`).
- `api.github.com` — Custom-repo install flow only (latest-release lookup,
  `backend.toml` fetch).
- `objects.githubusercontent.com` and `github.com` — release asset
  downloads.

No secrets cross these boundaries. Secrets live in a file only the user can
read (mode `0600`, in the daemon's state directory) and are read at model load
time only, not at install time.

The integrity story for installed assets is SHA-256 verification against the
registry's index, computed by the indexer at index-build time. A compromised
Pages host could serve a malicious index, but cannot bypass the wasm
sandbox or recover the user's secrets through the install path. Custom-repo
installs are explicitly unverified — only TLS protects them — and the daemon
surfaces this to the client.

## Best Practices

### For Single-User Systems
The default configuration is secure for personal desktop use:
```bash
just install-daemon
```

### For Multi-User Systems
The daemon is strictly per-user: its socket lives in the owner's runtime
directory and every request is rejected unless the peer's UID matches
the daemon's. Each user runs their own daemon instance — there is no shared
access to configure and no shared group.

### For Development
Debug builds automatically use relaxed security for development convenience:
```bash
# Debug build - automatically skips process authentication
cargo run --bin super-voice-daemon

# Release build - enforces full security model
cargo run --release --bin super-voice-daemon
```

## Threat Model

### Protected Against
- ✅ Unauthorized local users accessing the daemon (owner-only socket + same-UID peer check)
- ✅ Remote network attacks (nothing listens off loopback — a per-user Unix socket, and a TCP port bound to `127.0.0.1` for browser clients)
- ✅ Unauthorized keyboard injection (reachable only with a consent-minted `transcribe`-scope token; auto-typing itself is the per-request `write_mode` flag)
- ✅ Arbitrary command injection via keyboard
- ✅ Unauthorized use of the speakers (reachable only with a consent-minted `speak`-scope token)
- ✅ Unauthorized access to the user's voice recordings (a separate `voices` scope; clips are owner-only files)
- ✅ Privilege escalation (runs as an unprivileged user service)
- ✅ Memory exhaustion attacks (comprehensive input validation)
- ✅ Connection flooding attacks (rate limiting and connection limits)
- ✅ JSON bomb attacks (size and depth validation)
- ✅ Directory traversal attacks (path validation)
- ✅ Audio data injection attacks (sample validation and pattern detection)
- ✅ Malformed backend audio (frame-size, total-size, and format validation on every synthesis response)
- ✅ Cross-user access (same-UID peer check; the consent prompt names the caller's exe)
- ✅ Silent replacement of the daemon binary or unit file by user-level processes (both are root-owned in system paths; modifying them requires privilege escalation)

### Assumptions
- Physical access to the machine is trusted
- The user running the daemon is trusted (the socket is same-user only)
- System has standard Unix permission enforcement
- `~/.config/systemd/user/` is not silently populated: a unit placed there overrides the packaged one (systemd precedence), so unit tampering at user level is detectable (`systemd-delta --user`) rather than impossible
- Development builds are only used in secure development environments

## Incident Response

### Suspicious Activity
Check daemon logs for unauthorized access attempts:
```bash
journalctl --user -u super-voice -f
```

### Revoke Access
Authorization is per-client consent tokens, not group membership. Forget the
cached token (forcing the client to re-request consent on its next call) with:
```bash
voice logout
```
Or stop the daemon (below) to drop all in-memory sessions.

### Emergency Shutdown
```bash
systemctl --user stop super-voice
systemctl --user disable super-voice
```

## Security Testing and Verification

### Security Reviews Completed
- **August 2025**: Comprehensive security review of all components
- **Verification**: All critical findings verified and addressed
- **Assessment**: Production-ready security posture confirmed

### Security Testing Coverage
- ✅ Input validation boundary testing
- ✅ Authentication bypass attempt testing
- ✅ Resource exhaustion protection testing
- ✅ Path traversal attack testing
- ✅ Memory safety analysis
- ✅ Consent-token / scope authorization verification

### Continuous Security
- Regular dependency security audits via `cargo audit`
- Comprehensive test coverage for security-critical functions
- Static analysis with Rust's built-in safety guarantees
- Input fuzzing for protocol handlers

### Running Security Audits
Check for known vulnerabilities in dependencies:
```bash
# Install cargo-audit (one-time setup)
cargo install cargo-audit

# Run security audit
cargo audit

# Update dependencies and re-audit
cargo update
cargo audit
```

## Deployment Security Checklist

### Production Deployment Requirements
- [ ] Use release builds (`cargo build --release`)
- [ ] Verify the socket is same-user (owner-only runtime directory, mode 0660)
- [ ] Review systemd service hardening settings
- [ ] Monitor logs for authentication failures

### Recommended Systemd Hardening
```ini
[Unit]
Description=Super Voice daemon - speech to text and text to speech
After=sound.target

[Service]
Type=simple
ExecStart=super-voice-daemon
Restart=on-failure
RestartSec=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
RestrictAddressFamilies=AF_UNIX AF_INET
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true
RestrictRealtime=true
RestrictSUIDSGID=true

# Resource limits
LimitNOFILE=1024
LimitNPROC=100

[Install]
WantedBy=default.target
```

## Compliance

The security model follows standard Unix security practices:
- Principle of least privilege
- Defense in depth
- Per-user access control
- Input validation and sanitization
- Resource limits and rate limiting
- Process authentication and authorization
- Comprehensive audit logging via systemd journal