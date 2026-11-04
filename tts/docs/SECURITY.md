# Super TTS Security Model

## Overview

Super TTS implements a comprehensive defense-in-depth security model to protect against unauthorized access while maintaining usability. The system uses multiple security layers including Unix domain sockets with group-based access control, process authentication for access to the speakers, and input validation throughout.

## Security Architecture Summary

Based on comprehensive security reviews completed in August 2025, Super TTS demonstrates **excellent security posture** with:

- ✅ **Process Authentication**: Robust authentication for every privileged operation
- ✅ **Input Validation**: Comprehensive framework with DoS protection and attack detection
- ✅ **Memory Safety**: Excellent use of Rust's safety guarantees
- ✅ **Network Security**: No inbound listener at all — a per-user Unix socket, no TCP or UDP port
- ✅ **Resource Management**: Connection limits and rate limiting prevent abuse
- ✅ **Path Security**: Comprehensive validation prevents directory traversal attacks
- ✅ **Tamper-Resistant Install**: Binaries and the systemd user unit are installed root-owned in system paths; the daemon itself runs unprivileged in the user session

**Security Assessment**: Production-ready with excellent security controls implemented.

## Socket Access Control

The daemon listens on a Unix domain socket at
`$XDG_RUNTIME_DIR/tts/super-tts-http.sock`. Access is restricted to the user
running the daemon — there is **no** shared `tts` group and no group membership
to configure. Two independent layers enforce this:

1. **Per-user runtime directory.** `$XDG_RUNTIME_DIR` (typically
   `/run/user/<uid>`) is created by the system with `0700` permissions, so no
   other local user can even traverse into it to reach the socket.
2. **Same-UID peer check.** Every request is checked against the connecting
   peer's UID (`SO_PEERCRED`); a peer whose UID differs from the daemon's is
   rejected. A process that somehow reached the socket still cannot use it
   unless it runs as the same user.

### Socket permissions

| Build | Mode | Notes |
| --- | --- | --- |
| Release (default) | `0660` | Owner + the daemon user's primary group. The socket is **not** chgrp'd to any shared group. |
| Debug (`cargo build`) | `0666` | Local development only; the `0700` runtime dir still keeps it unreachable by other users. |

### Authorization

Reaching the socket is not the same as being authorized. Privileged operations
(speaking, settings, secrets) require a per-request session token
carrying user-approved scopes, minted through a one-time consent prompt and
bound to the client's executable path. See
[`protocol/auth.md`](protocol/auth.md) for the token and scope model.

### Verification
```bash
ls -la "$XDG_RUNTIME_DIR/tts/super-tts-http.sock"
# srw-rw---- ... <you> <your-primary-group> ... super-tts-http.sock
```

## Security Features

### 1. Audio Output Protection
- **Consent-gated authorization**: Making the machine speak requires a consent-minted session token with the `speak` scope — so a client can only reach the speakers after the user approved it through the one-time consent prompt. See [Authorization](#authorization) and [`protocol/auth.md`](protocol/auth.md)
- **Interruption is part of the grant**: one utterance plays at a time and the newest wins, so a `speak`-scoped client can cut off whatever else is being read aloud, and can silence it with `/speak/stop`. The consent prompt says so in those words rather than leaving it to be discovered
- **Peer identity for the prompt**: `SO_PEERCRED` + `/proc/<pid>/exe` tell the consent prompt *which binary* is asking and reject cross-UID peers; this identifies the caller, it is not by itself the authorization
- **Debug-only test bypass**: The consent auto-approval used by tests/CI (`SUPER_TTS_AUTO_APPROVE`) is compiled out of release builds
- **No keyboard surface at all**: this daemon does not simulate keystrokes. The STT build did, to type transcriptions into the focused window; that entire subsystem — keyboard backends, the typing sanitizer, the `write_mode` flag — is gone rather than disabled, so there is no path to re-enable it by configuration

### 2. Network Isolation
- **No inbound network surface**: The daemon listens only on a per-user Unix domain socket under `$XDG_RUNTIME_DIR/tts/` — there is no TCP or UDP listener, so no host on the network can connect to it
- **Local visualization**: Audio-visualization frames are delivered as Server-Sent Events over that same Unix socket, not broadcast on the network
- **Outbound only for backend installs**: The daemon reaches the network solely to fetch backends over HTTPS from the registry / GitHub — see [Daemon outbound network surface](#daemon-outbound-network-surface) below

### 3. Consent &amp; Peer Identity
- **Session tokens + scopes**: Every request other than `/auth/request` requires a `Bearer` session token; each token carries the user-approved scopes that gate what it may do (`speak`, `settings`, `secrets`, `status`, and the event-topic scopes — see [`protocol/auth.md`](protocol/auth.md) for the full catalog)
- **One-time consent**: Tokens are minted by the `super-tts-consent` helper — a popup naming the requesting binary (resolved via `SO_PEERCRED`) and the scopes it asks for, which the user approves or denies
- **Same-UID only**: A peer whose UID differs from the daemon's is rejected before any prompt
- See [`protocol/auth.md`](protocol/auth.md) for the full token, scope, and consent contract

### 4. Input Validation and DoS Protection
- **Comprehensive validation**: All external inputs validated with strict limits
- **Text length cap**: `POST /speak` accepts at most 50 000 characters, and a model may declare a lower per-request cap in its manifest which the daemon applies on top
- **Synthesis response caps**: a backend's framed audio response is bounded at 1 MiB per frame and 32 MiB per utterance, so a backend cannot exhaust memory by streaming without end
- **JSON protection**: Size (1MB) and nesting depth (10 levels) limits prevent JSON bomb attacks
- **Suspicious pattern detection**: Detects potential padding attacks in audio data
- **Control character filtering**: Prevents injection of dangerous characters

### 5. Resource Management and Rate Limiting
- **Connection limits**: Maximum concurrent connections enforced (20 in production, 50 in development)
- **Rate limiting**: Per-client request limits with sliding window tracking
  - Production: 60 requests/minute, 1800 requests/hour
  - Development: 300 requests/minute, 7200 requests/hour
- **Connection timeouts**: Automatic cleanup of inactive connections (3 minutes production, 10 minutes development)
- **Memory protection**: Resource limits prevent memory exhaustion attacks

### 6. Process Isolation
- **User service**: Runs under user account, not root
- **Per-user socket**: Owner-only `$XDG_RUNTIME_DIR` plus a same-UID peer check — no cross-user access, no shared group
- **Socket permissions**: Enforced at filesystem level
- **Peer identity**: `SO_PEERCRED` resolves the peer UID (same-UID enforced) and exe path (shown in the consent prompt) — the authorization itself is the consent-minted session token and its scopes

## Daemon outbound network surface

Prior to the backend registry, the daemon
process made no outbound network connections; only wasm-backend modules
issued HTTPS via `wasi:http`, constrained by per-backend `allowed_hosts`.

After the registry work, the daemon process itself opens HTTPS to:

- `jorge-menjivar.github.io` — registry index (`index.json`).
- `api.github.com` — Custom-repo install flow only (latest-release lookup,
  `backend.toml` fetch).
- `objects.githubusercontent.com` and `github.com` — release asset
  downloads.

No keyring secrets cross these boundaries. Secrets are read at model load
time only, not at install time.

The integrity story for installed assets is SHA-256 verification against the
registry's index, computed by the indexer at index-build time. A compromised
Pages host could serve a malicious index, but cannot bypass the wasm
sandbox or recover keyring secrets through the install path. Custom-repo
installs are explicitly unverified — only TLS protects them — and the daemon
surfaces this to the client.

## Best Practices

### For Single-User Systems
The default configuration is secure for personal desktop use:
```bash
just install-daemon
```

### For Multi-User Systems
The daemon is strictly per-user: its socket lives in the owner's
`$XDG_RUNTIME_DIR` and every request is rejected unless the peer's UID matches
the daemon's. Each user runs their own daemon instance — there is no shared
access to configure and no `tts` group.

### For Development
Debug builds automatically use relaxed security for development convenience:
```bash
# Debug build - automatically skips process authentication
cargo run --bin super-tts-daemon

# Release build - enforces full security model
cargo run --release --bin super-tts-daemon
```

## Threat Model

### Protected Against
- ✅ Unauthorized local users accessing the daemon (owner-only socket + same-UID peer check)
- ✅ Remote network attacks (no inbound listener — Unix-socket only, no TCP/UDP port)
- ✅ Unauthorized use of the speakers (reachable only with a consent-minted `speak`-scope token)
- ✅ Privilege escalation (runs as an unprivileged user service)
- ✅ Memory exhaustion attacks (comprehensive input validation)
- ✅ Connection flooding attacks (rate limiting and connection limits)
- ✅ JSON bomb attacks (size and depth validation)
- ✅ Directory traversal attacks (path validation)
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
journalctl --user -u super-tts -f
```

### Revoke Access
Authorization is per-client consent tokens, not group membership. Forget the
cached token (forcing the client to re-request consent on its next call) with:
```bash
tts logout
```
Or stop the daemon (below) to drop all in-memory sessions.

### Emergency Shutdown
```bash
systemctl --user stop super-tts
systemctl --user disable super-tts
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
- [ ] Verify the socket is same-user (owner-only `$XDG_RUNTIME_DIR`, mode 0660)
- [ ] Review systemd service hardening settings
- [ ] Monitor logs for authentication failures

### Recommended Systemd Hardening
```ini
[Unit]
Description=Super TTS Text-to-Speech Daemon
After=sound.target

[Service]
Type=simple
ExecStart=super-tts-daemon
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
- Group-based access control
- Input validation and sanitization
- Resource limits and rate limiting
- Process authentication and authorization
- Comprehensive audit logging via systemd journal