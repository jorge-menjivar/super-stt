// SPDX-License-Identifier: GPL-3.0-only
//! Progress reporting: JSON lines on stdout for the app, colored text for
//! humans. The JSON shapes are a wire contract — see the golden tests.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Resolve,
    Download,
    Verify,
    Stage,
    Escalate,
    Install,
    PostInstall,
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event<'a> {
    Phase {
        phase: Phase,
        message: &'a str,
    },
    Progress {
        phase: Phase,
        bytes_done: u64,
        bytes_total: u64,
    },
    Complete {
        installed_version: &'a str,
        components: &'a [String],
    },
    Error {
        code: &'a str,
        message: &'a str,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum Reporter {
    Json,
    Human,
}

impl Reporter {
    pub fn emit(self, event: &Event<'_>) {
        match self {
            Self::Json => {
                if let Ok(line) = serde_json::to_string(event) {
                    println!("{line}");
                }
            }
            Self::Human => match event {
                Event::Phase { message, .. } => println!("\x1b[0;32m[INFO]\x1b[0m {message}"),
                Event::Progress { .. } => {} // byte counts are noise on a TTY
                Event::Complete {
                    installed_version, ..
                } => {
                    println!("\x1b[0;32m[INFO]\x1b[0m Installed Super STT {installed_version}");
                }
                Event::Error { message, .. } => {
                    eprintln!("\x1b[0;31m[ERROR]\x1b[0m {message}");
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, Phase};

    // GOLDEN STRINGS — duplicated in super-stt-app's parser tests (Task 7.5).
    // Change one side and the other MUST change with it.
    #[test]
    fn json_events_serialize_to_documented_shape() {
        let ev = Event::Phase {
            phase: Phase::Download,
            message: "downloading super-stt-x86_64-unknown-linux-gnu-beta.tar.gz",
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"phase","phase":"download","message":"downloading super-stt-x86_64-unknown-linux-gnu-beta.tar.gz"}"#
        );
        let ev = Event::Progress {
            phase: Phase::Download,
            bytes_done: 512,
            bytes_total: 2048,
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"progress","phase":"download","bytes_done":512,"bytes_total":2048}"#
        );
        let comps = vec!["daemon".to_string(), "app".to_string()];
        let ev = Event::Complete {
            installed_version: "v0.2.3-beta.1",
            components: &comps,
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"complete","installed_version":"v0.2.3-beta.1","components":["daemon","app"]}"#
        );
        let ev = Event::Error {
            code: "checksum_mismatch",
            message: "boom",
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"error","code":"checksum_mismatch","message":"boom"}"#
        );
    }
}
