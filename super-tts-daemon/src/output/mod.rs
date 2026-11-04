// SPDX-License-Identifier: GPL-3.0-only

// Desktop notifications are all that is left here: the STT build also owned
// keyboard simulation, because its output was text typed into the focused
// window. This one's output is audio.
pub(crate) mod notice;
pub mod notification;
