// SPDX-License-Identifier: GPL-3.0-only
//! `/shortcuts`' work: adding and removing a shortcut by the method the
//! desktop has, and binding the portal's again at every start. What the
//! methods are is [`crate::shortcuts`].

use crate::daemon::types::SuperSTTDaemon;
use crate::shortcuts::{self, Error};
use log::{info, warn};
use super_voice_shared::models::shortcuts::{ShortcutId, ShortcutList, ShortcutMethod};

impl SuperSTTDaemon {
    /// Every shortcut and where it stands.
    pub async fn list_shortcuts(&self) -> ShortcutList {
        self.shortcuts.list().await
    }

    /// Add shortcut `id`, then answer the list. Adding one that is set up
    /// already changes nothing and succeeds.
    ///
    /// # Errors
    ///
    /// See [`Error`].
    pub async fn add_shortcut(&self, id: ShortcutId) -> Result<ShortcutList, Error> {
        self.change_shortcut(id, true).await
    }

    /// Remove shortcut `id`, then answer the list. Removing one that is not
    /// set up changes nothing and succeeds.
    ///
    /// # Errors
    ///
    /// See [`Error`].
    pub async fn remove_shortcut(&self, id: ShortcutId) -> Result<ShortcutList, Error> {
        self.change_shortcut(id, false).await
    }

    async fn change_shortcut(&self, id: ShortcutId, add: bool) -> Result<ShortcutList, Error> {
        let _changing = self.shortcuts.changing().await;
        match self.shortcuts.method().await {
            ShortcutMethod::Cosmic => {
                let change = if add {
                    shortcuts::add_cosmic
                } else {
                    shortcuts::remove_cosmic
                };
                tokio::task::spawn_blocking(move || change(id))
                    .await
                    .unwrap_or_else(|e| Err(Error::Write(e.to_string())))?;
            }
            ShortcutMethod::Portal => self.change_portal_shortcut(id, add).await?,
            ShortcutMethod::Manual if add => return Err(Error::Unsupported),
            ShortcutMethod::Manual => {}
        }
        Ok(self.shortcuts.list().await)
    }

    /// Bind the portal's shortcuts again with `id` added or taken out, and
    /// keep what the desktop bound in the config. On KDE Plasma, one taken
    /// out leaves KDE's settings too.
    ///
    /// A bind replaces every shortcut, so one that fails would leave none:
    /// the ones held before are bound again then.
    async fn change_portal_shortcut(&self, id: ShortcutId, add: bool) -> Result<(), Error> {
        let desktop = shortcuts::desktop_name();
        let before = self.config.read().await.shortcuts.portal_on(&desktop);
        let wanted = shortcuts::with(&before, id, add);
        let changed = if add {
            self.shortcuts.bind_added(&wanted, id).await
        } else {
            self.shortcuts.bind_removed(&wanted, id).await
        };
        let bound = match changed {
            Ok(bound) => bound,
            Err(e) => {
                if let Err(again) = self.shortcuts.bind(&before).await {
                    warn!("Could not bind the shortcuts held before: {again:?}");
                }
                return Err(e);
            }
        };
        let kept: Vec<ShortcutId> = wanted
            .into_iter()
            .filter(|s| bound.iter().any(|b| b.id == *s))
            .collect();
        let refused = add && !kept.contains(&id);
        if let Err(e) = self
            .set_config_field(|c| c.shortcuts.set_portal_on(&desktop, kept))
            .await
        {
            warn!("Shortcuts changed but config save failed: {e}");
        }
        if refused {
            return Err(Error::Refused);
        }
        Ok(())
    }

    /// Bind the shortcuts the user added through the portal on this desktop,
    /// as the daemon starts. Nothing is asked of the desktop when there are
    /// none, and ones added on another desktop are left for that one. One
    /// the desktop no longer has, removed in its settings, is forgotten. The
    /// config is left as it is when the bind fails: the portal may just not
    /// be up yet, and the next start tries again.
    pub(crate) async fn bind_saved_shortcuts(&self) {
        let desktop = shortcuts::desktop_name();
        let saved = self.config.read().await.shortcuts.portal_on(&desktop);
        if saved.is_empty() {
            return;
        }
        let _changing = self.shortcuts.changing().await;
        if self.shortcuts.method().await != ShortcutMethod::Portal {
            info!("Shortcuts saved for the portal, but this desktop has none: not bound");
            return;
        }
        match self.shortcuts.bind_saved(&saved).await {
            Ok(kept) if kept == saved => info!("Bound {} saved shortcuts", kept.len()),
            Ok(kept) => {
                // Removed in the desktop's settings: forgotten here too, so
                // the page offers to add them again.
                info!(
                    "Bound {} saved shortcuts; the desktop no longer has the rest",
                    kept.len()
                );
                if let Err(e) = self
                    .set_config_field(|c| c.shortcuts.set_portal_on(&desktop, kept))
                    .await
                {
                    warn!("Shortcuts forgotten but config save failed: {e}");
                }
            }
            Err(e) => warn!("Could not bind the saved shortcuts: {e:?}"),
        }
    }
}
