// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use enigo::{Direction, Enigo, Key, Keyboard, NewConError, Settings};

pub struct EnigoBackend {
    typing_chunk: usize,
    backspace_batch_size: usize,
    enigo: Enigo,
}

impl EnigoBackend {
    /// Open a keyboard connection for a recording.
    ///
    /// # Errors
    /// When the platform will not hand over a keyboard: no compositor
    /// exposing `zwp_virtual_keyboard_manager_v1` on Linux, no Accessibility
    /// grant or no window server on macOS.
    pub fn new() -> Result<Self> {
        // `false`: never put the macOS Accessibility prompt on screen from
        // here. This runs on the recording path, where the user has just
        // finished speaking and is waiting for text — a modal dialog stealing
        // focus at that moment lands in whatever window they were dictating
        // into. The prompt belongs at startup, where nothing is in flight;
        // that is what `probe_accessibility_permission` is for. On Linux the
        // flag is ignored (enigo documents it as macOS-only).
        Self::build(false)
    }

    /// Open a connection purely to find out whether macOS will allow it,
    /// letting the system's Accessibility prompt appear if it will not.
    ///
    /// # Errors
    /// As [`Self::new`].
    #[cfg(target_os = "macos")]
    pub fn probe_accessibility_permission() -> Result<()> {
        Self::build(true).map(drop)
    }

    fn build(open_prompt_to_get_permissions: bool) -> Result<Self> {
        let settings = Settings {
            open_prompt_to_get_permissions,
            ..Settings::default()
        };
        let enigo = Enigo::new(&settings).map_err(describe_connection_failure)?;

        Ok(Self {
            typing_chunk: 64,
            backspace_batch_size: 20,
            enigo,
        })
    }

    /// Type `text` into the focused window.
    ///
    /// The two platforms need opposite choices here.
    ///
    /// **Linux: one key event per character, deliberately not `enigo.text()`.**
    /// When the focused application speaks the input-method protocol,
    /// `text()` commits the text in one piece, and a terminal hands such a
    /// commit to the shell as a paste. The shell may then rewrite it: fish
    /// escapes a `'` pasted inside an open quoted string, so `OpenAI's … I'm`
    /// landed as `OpenAI's … I\'m`. It also puts the text on a different
    /// channel from the backspaces that later erase it. Key events keep
    /// everything on one channel and are never a paste.
    ///
    /// **macOS: `enigo.text()`, because per-character key events lose Shift.**
    /// For `Key::Unicode(c)` the macOS backend searches the keyboard layout for
    /// a keycode that produces `c` with no modifier *or* with Shift, returns
    /// only the keycode, and posts it with no Shift flag. So `I` arrives as
    /// `i`, `?` as `/`, `!` as `1`, `:` as `;` — every shifted character
    /// loses its Shift — and a character the layout reaches only through ⌥ or
    /// a dead key (`é`, `¿`) is not found at all. `text()` on macOS attaches
    /// the literal string to the event (`CGEventKeyboardSetUnicodeString`)
    /// and needs no layout lookup. Neither Linux objection applies: it is an
    /// ordinary keyboard event posted to the same HID event tap as the
    /// backspaces, not an input-method commit, and no terminal treats it as a
    /// paste.
    ///
    /// Synchronous: enigo's handle holds raw xkbcommon pointers (`!Send`) and
    /// the work blocks (per-chunk sleeps). The [`Simulator`](super::Simulator)
    /// enum runs this under `block_in_place` so the handle never crosses an
    /// `.await` point — no thread migration mid-type (audit Tier 3 #35).
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(self.typing_chunk) {
            #[cfg(target_os = "macos")]
            {
                let chunk: String = chunk.iter().collect();
                self.enigo
                    .text(&chunk)
                    .map_err(|e| anyhow::anyhow!("Failed to type {chunk:?}: {e}"))?;
            }
            #[cfg(not(target_os = "macos"))]
            for &c in chunk {
                self.enigo
                    .key(Key::Unicode(c), Direction::Click)
                    .map_err(|e| anyhow::anyhow!("Failed to type {c:?}: {e}"))?;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(())
    }

    /// Press Backspace `n` times.
    ///
    /// # Errors
    /// The first click the backend refuses. Nothing can read the screen back
    /// to check that a keystroke landed, so a refusal is the one signal there
    /// is; swallowing it left a surviving character unexplained in the log.
    pub fn backspace_n(&mut self, n: usize) -> Result<()> {
        let mut sent = 0;
        while sent < n {
            let batch_size = (n - sent).min(self.backspace_batch_size);
            for _ in 0..batch_size {
                self.enigo
                    .key(Key::Backspace, Direction::Click)
                    .map_err(|e| anyhow::anyhow!("Backspace {} of {n} failed: {e}", sent + 1))?;
                sent += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }
}

/// Turn enigo's connection error into something the user can act on.
///
/// [`NewConError::NoPermission`] gets its own message because it is the one
/// failure here that is not a broken system but an unticked checkbox, and
/// because it is the overwhelmingly common first-run experience on macOS.
/// How to restart this process after granting Accessibility, as precisely as
/// can be known from inside it.
///
/// Granting and then restarting is the sequence known to work; whether a
/// running process picks up a new grant on its own is not something to lean
/// on. Run as the `LaunchAgent` there is an exact command, and launchd says
/// which job this is: it puts the job's label in `XPC_SERVICE_NAME`, where a
/// process started from a shell has `0` or nothing.
#[cfg(target_os = "macos")]
fn restart_instruction() -> String {
    match std::env::var("XPC_SERVICE_NAME") {
        Ok(label) if !label.is_empty() && label != "0" => {
            // SAFETY: `getuid` has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            format!("restart it: launchctl kickstart -k gui/{uid}/{label}")
        }
        _ => "restart the daemon".to_string(),
    }
}

/// enigo's own text — "the application does not have the permission to
/// simulate input" — says what is wrong and not one word about where to fix
/// it, and this string is what reaches the user: it is surfaced as the
/// recording-failure notice, so it is their whole explanation.
fn describe_connection_failure(e: NewConError) -> anyhow::Error {
    if matches!(e, NewConError::NoPermission) {
        let exe = std::env::current_exe().map_or_else(
            |_| "the Super STT daemon".to_string(),
            |p| p.display().to_string(),
        );
        #[cfg(target_os = "macos")]
        return anyhow::anyhow!(
            "Super STT is not allowed to control your keyboard. Add {exe} under \
             System Settings › Privacy & Security › Accessibility, then {}.",
            restart_instruction()
        );
        #[cfg(not(target_os = "macos"))]
        return anyhow::anyhow!("{exe} was refused permission to simulate input: {e}");
    }
    anyhow::anyhow!("Failed to initialize enigo: {e}")
}
