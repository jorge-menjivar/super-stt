// SPDX-License-Identifier: GPL-3.0-only

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tokio::net::UnixStream;

/// Authentication for write-mode operations using process verification
#[derive(Clone)]
pub struct ProcessAuth {
    /// Expected path to the legitimate stt binary
    expected_stt_paths: Vec<PathBuf>,
}

impl ProcessAuth {
    /// Create a new process authenticator
    #[must_use]
    pub fn new() -> Self {
        // Common installation paths for the stt binary and wrapper
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let expected_stt_paths = vec![
            // Main daemon binary
            PathBuf::from(format!("{home}/.local/bin/super-stt")),
            PathBuf::from("/usr/local/bin/super-stt"),
            PathBuf::from("/usr/bin/super-stt"),
            // Wrapper script
            PathBuf::from(format!("{home}/.local/bin/stt")),
            PathBuf::from("/usr/local/bin/stt"),
            PathBuf::from("/usr/bin/stt"),
            // Add development paths
            PathBuf::from("target/debug/super-stt"),
            PathBuf::from("target/release/super-stt"),
        ];

        Self { expected_stt_paths }
    }

    /// Verify that the connecting client is a legitimate stt binary (super-stt or stt wrapper)
    pub fn verify_write_permission(&self, stream: &UnixStream) -> bool {
        // In debug builds, skip verification for development
        if cfg!(debug_assertions) {
            log::debug!("Debug build: Skipping write permission verification");
            return true;
        }

        match self.verify_peer_process(stream) {
            Ok(is_valid) => {
                if is_valid {
                    log::info!("✓ Write permission granted - verified legitimate stt client");
                } else {
                    log::warn!("✗ Write permission denied - unverified client process");
                }
                is_valid
            }
            Err(e) => {
                log::error!("Write permission verification failed: {e}");
                false
            }
        }
    }

    /// Verify the peer process is a legitimate stt binary (super-stt or stt wrapper)
    fn verify_peer_process(&self, stream: &UnixStream) -> Result<bool> {
        // Get peer credentials
        let peer_cred = stream
            .peer_cred()
            .context("Failed to get peer credentials")?;

        let peer_pid = peer_cred
            .pid()
            .ok_or_else(|| anyhow::anyhow!("No peer PID available"))?;

        log::info!("Verifying peer process PID: {peer_pid}");

        // Get the executable path of the peer process
        let exe_link = PathBuf::from(format!("/proc/{peer_pid}/exe"));
        let peer_exe_path = match fs::read_link(&exe_link) {
            Ok(path) => path,
            Err(e) => {
                log::error!("Could not read peer exe path: {e}");
                return Ok(false);
            }
        };

        log::info!("Peer executable path: {}", peer_exe_path.display());

        // Check if it matches any of our expected paths
        for expected_path in &self.expected_stt_paths {
            // Resolve expected path to absolute path
            let Ok(resolved_expected) = expected_path.canonicalize() else {
                continue;
            };

            // Compare the resolved paths
            if peer_exe_path == resolved_expected {
                return Ok(true);
            }
        }

        // Also check if the process name matches any expected name as a fallback
        let comm_path = PathBuf::from(format!("/proc/{peer_pid}/comm"));
        if let Ok(comm) = fs::read_to_string(&comm_path) {
            let process_name = comm.trim();
            if process_name == "super-stt" || process_name == "stt" {
                return Ok(true);
            }
            log::info!(
                "Process name '{process_name}' does not match expected names (super-stt, stt)"
            );
        }

        Ok(false)
    }

    /// Add a custom path to the list of expected stt binaries
    pub fn add_expected_path(&mut self, path: PathBuf) {
        self.expected_stt_paths.push(path);
    }
}

impl Default for ProcessAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
mod unix_ext {
    use tokio::net::UnixStream;

    pub trait PeerCredentials {
        /// Returns peer credentials for this Unix stream.
        ///
        /// # Errors
        /// Returns an error if retrieving the credentials from the socket
        /// fails (e.g., the platform does not support `SO_PEERCRED`, or the
        /// underlying `getsockopt` call fails).
        fn peer_cred(&self) -> std::io::Result<PeerCred>;
    }

    pub struct PeerCred {
        uid: u32,
        gid: u32,
        pid: Option<u32>,
    }

    impl PeerCred {
        pub fn pid(&self) -> Option<u32> {
            self.pid
        }

        #[allow(dead_code)]
        pub fn uid(&self) -> u32 {
            self.uid
        }

        #[allow(dead_code)]
        pub fn gid(&self) -> u32 {
            self.gid
        }
    }

    impl PeerCredentials for UnixStream {
        fn peer_cred(&self) -> std::io::Result<PeerCred> {
            use std::os::unix::io::AsRawFd;

            let fd = self.as_raw_fd();
            get_peer_cred(fd)
        }
    }

    #[cfg(target_os = "linux")]
    fn get_peer_cred(fd: std::os::fd::RawFd) -> std::io::Result<PeerCred> {
        use std::convert::TryFrom;
        use std::io::Error;
        use std::mem;
        use std::os::raw::c_uint;

        #[repr(C)]
        struct UCred {
            pid: c_uint,
            uid: c_uint,
            gid: c_uint,
        }

        let mut ucred: UCred = unsafe { mem::zeroed() };
        let mut ucred_size: libc::socklen_t = libc::socklen_t::try_from(mem::size_of::<UCred>())
            .map_err(|_| Error::other("socklen_t overflow"))?;

        let ret = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut ucred).cast::<libc::c_void>(),
                &raw mut ucred_size,
            )
        };

        if ret == 0 {
            Ok(PeerCred {
                uid: ucred.uid,
                gid: ucred.gid,
                pid: Some(ucred.pid),
            })
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn get_peer_cred(_fd: std::os::fd::RawFd) -> std::io::Result<PeerCred> {
        // For non-Linux systems, return minimal cred without PID
        Ok(PeerCred {
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            pid: None,
        })
    }
}

#[cfg(unix)]
pub use unix_ext::PeerCredentials;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expected_paths_include_wrapper() {
        let auth = ProcessAuth::new();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());

        // Verify that both super-stt and stt paths are included
        let expected_super_stt = format!("{home}/.local/bin/super-stt");
        let expected_stt_wrapper = format!("{home}/.local/bin/stt");

        let paths: Vec<String> = auth
            .expected_stt_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        assert!(
            paths.contains(&expected_super_stt),
            "Expected paths should include super-stt binary: {expected_super_stt}"
        );
        assert!(
            paths.contains(&expected_stt_wrapper),
            "Expected paths should include stt wrapper: {expected_stt_wrapper}"
        );

        // Verify system paths for both
        assert!(paths.contains(&"/usr/local/bin/super-stt".to_string()));
        assert!(paths.contains(&"/usr/local/bin/stt".to_string()));
        assert!(paths.contains(&"/usr/bin/super-stt".to_string()));
        assert!(paths.contains(&"/usr/bin/stt".to_string()));
    }

    #[test]
    fn test_add_custom_path() {
        let mut auth = ProcessAuth::new();
        let custom_path = PathBuf::from("/custom/path/to/stt");

        auth.add_expected_path(custom_path.clone());

        assert!(auth.expected_stt_paths.contains(&custom_path));
    }
}
