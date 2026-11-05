// SPDX-License-Identifier: GPL-3.0-only
//! The Contexts page: what the user is dictating, so a model can hear it.
//!
//! Two surfaces. [`page`] is the list — one card per context, one of them
//! active — and [`editor_sheet`] is the right-side drawer that edits one.
//!
//! The split follows the Models page's: a list stays visible behind a sheet, so
//! picking the next thing to edit is one click rather than a close and a
//! re-find.

mod editor;
mod list;
mod surface;

pub use editor::{editor_sheet, editor_title};
pub use list::page;
