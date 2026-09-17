// SPDX-License-Identifier: GPL-3.0-only
//! Reading a flatpak sandbox's identity out of its `.flatpak-info`.

/// Pull the application id out of a `.flatpak-info`: `name=` under the
/// `[Application]` section.
///
/// Section-scoped on purpose. `name=` appears under other sections too, and
/// the wrong one would silently become an app's identity. Lives here rather
/// than beside either caller because both the daemon (reading a *peer's*
/// `/proc/<pid>/root/.flatpak-info`) and a client (reading its own) have to
/// agree on what the file says.
#[must_use]
pub fn app_id_from_info(contents: &str) -> Option<String> {
    let mut in_application = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_application = line == "[Application]";
            continue;
        }
        if in_application
            && let Some(value) = line.strip_prefix("name=")
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// This process's own flatpak application id, or `None` outside a sandbox.
///
/// Read from `/.flatpak-info` rather than `FLATPAK_ID` for the same reason
/// [`in_flatpak`] is: the file is written by flatpak, the variable is just an
/// environment variable.
#[must_use]
pub fn own_app_id() -> Option<String> {
    let contents = std::fs::read_to_string("/.flatpak-info").ok()?;
    app_id_from_info(&contents)
}

#[cfg(test)]
mod tests {
    use super::app_id_from_info;

    /// The shape flatpak actually writes.
    #[test]
    fn reads_the_application_name() {
        let info = "[Application]\nname=ai.menjivar.SuperSTT\nruntime=runtime/org.freedesktop.Platform/x86_64/25.08\n";
        assert_eq!(
            app_id_from_info(info).as_deref(),
            Some("ai.menjivar.SuperSTT")
        );
    }

    /// A `name=` outside `[Application]` must never be mistaken for the app
    /// id — that is another key entirely, and taking it would label the
    /// caller with something that is not its identity.
    #[test]
    fn ignores_name_in_other_sections() {
        let info = "[Instance]\nname=not-the-app\n[Context]\nsockets=wayland\n";
        assert_eq!(app_id_from_info(info), None);

        let info = "[Instance]\nname=not-the-app\n[Application]\nname=org.example.Real\n";
        assert_eq!(app_id_from_info(info).as_deref(), Some("org.example.Real"));
    }

    #[test]
    fn empty_or_missing_name_is_none() {
        assert_eq!(app_id_from_info("[Application]\nname=\n"), None);
        assert_eq!(app_id_from_info("[Application]\nruntime=x\n"), None);
        assert_eq!(app_id_from_info(""), None);
    }
}
