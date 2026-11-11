// SPDX-License-Identifier: GPL-3.0-only

//! Moving a model to a framework whose build is not installed: ask, download
//! the build, then switch.
//!
//! The daemon offers every framework a published build would run on here and
//! refuses a switch to one whose build is not installed. A backend has one
//! build installed at a time, so getting another means installing over it,
//! which keeps the downloaded models. That is a download, so it is asked about
//! every time. The daemon keeps the build an install replaced, so switching
//! back installs it from disk, and that is not asked about.

use crate::core::app::AppModel;
use crate::state::Stage;
use crate::state::device_offers::build_installed;
use crate::state::models_page::PendingBuild;
use crate::ui::messages::{Message, ModelsPageMessage};
use cosmic::prelude::*;

impl AppModel {
    /// A framework picked in `stage`'s Runs on row. One whose build is
    /// installed is chosen at once, and one whose build is on disk is
    /// installed at once; one whose build would be downloaded asks first.
    pub(in crate::core::app) fn pick_device(
        &mut self,
        stage: Stage,
        device: String,
    ) -> Task<cosmic::Action<Message>> {
        if self.build_downloading(stage) {
            log::warn!("A build for stage {stage} is downloading; ignoring the pick");
            return Task::none();
        }
        let Some(source) = self.selected_backend(stage) else {
            return self.choose_device(stage, device);
        };
        let installed = self
            .stage_catalog
            .backend(stage, &source)
            .map_or(&[][..], |b| b.installed_accel.as_slice());
        if build_installed(&device, installed) {
            return self.choose_device(stage, device);
        }
        let on_disk = self
            .registry
            .build(&source, &device)
            .is_some_and(|build| build.on_disk);
        self.models_page.pending_build = Some(PendingBuild {
            stage,
            source,
            framework: device,
            downloading: false,
        });
        if on_disk {
            return self.confirm_build();
        }
        Task::none()
    }

    /// Whether `stage`'s backend is downloading a build the user agreed to.
    pub fn build_downloading(&self, stage: Stage) -> bool {
        self.models_page
            .pending_build
            .as_ref()
            .is_some_and(|p| p.downloading && p.stage == stage)
    }

    /// The user agreed, or there is nothing to download: install the build
    /// for the framework they picked.
    pub(in crate::core::app) fn confirm_build(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(pending) = self
            .models_page
            .pending_build
            .as_mut()
            .filter(|p| !p.downloading)
        else {
            return Task::none();
        };
        pending.downloading = true;
        let (source, framework) = (pending.source.clone(), pending.framework.clone());
        self.registry.install_errors.remove(&source);
        let key = source.clone();
        Task::perform(
            async move { crate::daemon::registry::install_by_source(&key, Some(&framework)).await },
            move |result| {
                cosmic::Action::App(Message::ModelsPage(match result {
                    Ok(accepted) => ModelsPageMessage::InstallAccepted {
                        source: source.clone(),
                        install_id: accepted.install_id,
                    },
                    Err(e) => ModelsPageMessage::InstallFailedToStart {
                        source: source.clone(),
                        error: e.to_string(),
                    },
                }))
            },
        )
    }

    /// The user declined. Nothing changes.
    pub(in crate::core::app) fn cancel_build(&mut self) {
        if self
            .models_page
            .pending_build
            .as_ref()
            .is_some_and(|p| !p.downloading)
        {
            self.models_page.pending_build = None;
        }
    }

    /// `source` finished installing. When that was a build the user asked
    /// for, move the model to its framework: a running model reloads onto it,
    /// one not running has it staged for its Load.
    pub(in crate::core::app) fn finish_build(
        &mut self,
        source: &str,
    ) -> Task<cosmic::Action<Message>> {
        let Some(pending) = self
            .models_page
            .pending_build
            .take_if(|p| p.downloading && p.source == source)
        else {
            return Task::none();
        };
        // The stage moved on to another backend while the build downloaded.
        if self.selected_backend(pending.stage).as_deref() != Some(source) {
            return Task::none();
        }
        self.choose_device(pending.stage, pending.framework)
    }

    /// `source` failed to install. When that was a build the user asked for,
    /// say so on the stage's card. The build that was there is untouched.
    pub(in crate::core::app) fn fail_build(&mut self, source: &str, error: &str) {
        let Some(pending) = self
            .models_page
            .pending_build
            .take_if(|p| p.downloading && p.source == source)
        else {
            return;
        };
        let framework = crate::ui::views::models::accel_name(&pending.framework);
        self.report_stage_error(
            pending.stage,
            &format!("Could not download the {framework} build: {error}"),
        );
    }
}
