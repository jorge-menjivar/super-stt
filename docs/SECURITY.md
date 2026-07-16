# Super STT Security Model

## Overview

Super STT implements a comprehensive defense-in-depth security model to protect against unauthorized access while maintaining usability. The system uses multiple security layers including Unix domain sockets with group-based access control, process authentication for keyboard access, and input validation throughout.

## Security Architecture Summary

Based on comprehensive security reviews completed in August 2025, Super STT demonstrates **excellent security posture** with:

- ✅ **Process Authentication**: Robust authentication for keyboard injection operations
- ✅ **Input Validation**: Comprehensive framework with DoS protection and attack detection
- ✅ **Memory Safety**: Excellent use of Rust's safety guarantees
- ✅ **Network Security**: Localhost-only UDP binding prevents remote attacks
- ✅ **Resource Management**: Connection limits and rate limiting prevent abuse
- ✅ **Path Security**: Comprehensive validation prevents directory traversal attacks

**Security Assessment**: Production-ready with excellent security controls implemented.

## Socket Access Control

The daemon listens on a Unix domain socket at
`$XDG_RUNTIME_DIR/stt/super-stt-http.sock`. Access is restricted to the user
running the daemon — there is **no** shared `stt` group and no group membership
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
(keyboard injection, settings, secrets) require a per-request session token
carrying user-approved scopes, minted through a one-time consent prompt and
bound to the client's executable path. See
[`protocol/auth.md`](protocol/auth.md) for the token and scope model.

### Verification
```bash
ls -la "$XDG_RUNTIME_DIR/stt/super-stt-http.sock"
# srw-rw---- ... <you> <your-primary-group> ... super-stt-http.sock
```

## Security Features

### 1. Keyboard Input Protection
- **Process authentication**: Write mode requires verification that the client is the legitimate stt binary
- **Unix socket credentials**: Uses peer credential verification to authenticate processes
- **Debug/release separation**: Authentication automatically disabled in debug builds for development
- **Limited scope**: Only types actual transcription results

### 2. Network Isolation  
- **UDP localhost-only**: Audio streaming bound to `127.0.0.1`
- **No remote access**: Network connections impossible
- **Read-only broadcast**: Only sends visualization data

### 3. Process Authentication
- **Unix peer credentials**: Verifies connecting process PID, UID, and executable path
- **Binary verification**: Ensures only the legitimate stt binary can trigger write mode
- **Compile-time security**: Uses `cfg!(debug_assertions)` to automatically disable in debug builds
- **Fallback checks**: Validates process name if path verification fails

### 4. Input Validation and DoS Protection
- **Comprehensive validation**: All external inputs validated with strict limits
- **Audio data protection**: Maximum 30 minutes of audio at 16kHz to prevent memory exhaustion
- **Sample rate validation**: Must be between 8kHz and 96kHz
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
- **Process authentication**: SO_PEERCRED verification for privileged operations

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
access to configure and no `stt` group.

### For Development
Debug builds automatically use relaxed security for development convenience:
```bash
# Debug build - automatically skips process authentication
cargo run --bin super-stt-daemon

# Release build - enforces full security model
cargo run --release --bin super-stt-daemon
```

## Threat Model

### Protected Against
- ✅ Unauthorized local users accessing the daemon
- ✅ Remote network attacks (localhost-only UDP binding)
- ✅ Unauthorized keyboard injection (process authentication with SO_PEERCRED)
- ✅ Arbitrary command injection via keyboard
- ✅ Privilege escalation (runs as an unprivileged user service)
- ✅ Memory exhaustion attacks (comprehensive input validation)
- ✅ Connection flooding attacks (rate limiting and connection limits)
- ✅ JSON bomb attacks (size and depth validation)
- ✅ Directory traversal attacks (path validation)
- ✅ Audio data injection attacks (sample validation and pattern detection)
- ✅ Process impersonation (binary path and name verification)

### Assumptions
- Physical access to the machine is trusted
- The user running the daemon is trusted (the socket is same-user only)
- System has standard Unix permission enforcement
- Development builds are only used in secure development environments

## Incident Response

### Suspicious Activity
Check daemon logs for unauthorized access attempts:
```bash
journalctl --user -u super-stt -f
```

### Revoke Access
Authorization is per-client consent tokens, not group membership. Forget the
cached token (forcing the client to re-request consent on its next call) with:
```bash
stt logout
```
Or stop the daemon (below) to drop all in-memory sessions.

### Emergency Shutdown
```bash
systemctl --user stop super-stt
systemctl --user disable super-stt
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
- ✅ Process authentication verification

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
Description=Super STT Speech-to-Text Daemon
After=sound.target

[Service]
Type=simple
ExecStart=%h/.local/bin/super-stt-daemon
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