// SPDX-License-Identifier: GPL-3.0-only
//! Contexts page state: the list the daemon owns, and the editor open over it.
//!
//! The list is a mirror, never a source of truth — every field here is replaced
//! wholesale by what a daemon write answered with, per the confirm-then-apply
//! rule in `ui/messages.rs`. The [`ContextDraft`] is the one exception, and is
//! the reason this module exists: an open editor is local, unsaved work, and a
//! refetch landing under it must not overwrite what the user is typing.

use crate::daemon::client::BackendContext;
use cosmic::widget;
use std::collections::HashMap;
use super_stt_shared::models::contexts::DictationContext;

/// Everything the Contexts page holds.
#[derive(Debug, Default)]
pub struct ContextsState {
    /// The contexts the daemon has, in the order the user arranged them.
    pub items: Vec<DictationContext>,
    /// The id in force by default, or `None`.
    pub active: Option<String>,
    /// The editor open over the list, if any.
    pub draft: Option<ContextDraft>,
    /// The id of a context whose delete is awaiting a second press, so a
    /// mis-click does not throw away a vocabulary someone spent time on.
    pub confirming_delete: Option<String>,
    /// Whether a list fetch is in flight, so the page can say "loading" rather
    /// than "you have no contexts".
    pub loading: bool,
    /// Whether the first fetch has landed. Until it has, an empty `items` means
    /// "not asked yet" and not "none exist" — the difference between a spinner
    /// and an empty state.
    pub loaded: bool,
    /// What each backend uses, keyed by `source`, as the daemon last reported.
    ///
    /// Read on demand when a Configure sheet opens rather than published with
    /// the catalog: it is one line of one sheet, and the catalog is refetched
    /// on every navigation to two pages that never show it.
    pub backend_contexts: HashMap<String, BackendContext>,
}

impl ContextsState {
    /// Replace the list with what the daemon reported.
    ///
    /// Deliberately does not touch [`draft`](Self::draft): a refetch triggered
    /// by another app's edit must not reach into an editor this user has open
    /// and rewrite the sentence they are halfway through.
    pub fn replace(&mut self, items: Vec<DictationContext>, active: Option<String>) {
        self.items = items;
        self.active = active;
        self.loading = false;
        self.loaded = true;
        // A pending delete naming a row no longer in the list is pointing at
        // nothing.
        if let Some(id) = &self.confirming_delete
            && !self.items.iter().any(|c| &c.id == id)
        {
            self.confirming_delete = None;
        }
    }

    /// One context by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&DictationContext> {
        self.items.iter().find(|c| c.id == id)
    }

    /// Fold one stored context into the list, replacing the row with its id or
    /// appending it.
    ///
    /// So a save updates the row in the frame the editor closes, rather than
    /// after the refetch that follows. Not a substitute for that refetch: this
    /// knows about one context, and a save can move the active selection too.
    pub fn upsert(&mut self, context: DictationContext) {
        match self.items.iter_mut().find(|c| c.id == context.id) {
            Some(existing) => *existing = context,
            None => self.items.push(context),
        }
    }

    /// An id no existing context has, for a context the user is creating.
    ///
    /// The id is a slug the user never sees — they name the context, and the
    /// name is free text. Minting it here rather than asking for one is what
    /// keeps "New context" a single click; the daemon's rules (lowercase,
    /// digits, `-`, `_`, and not `active` or `list`) are satisfied by
    /// construction.
    #[must_use]
    pub fn next_id(&self) -> String {
        // Bounded rather than an open range: one more than the list can hold is
        // enough to guarantee a free slot, and an unbounded search here would
        // be a loop with no reason to stop if `get` ever went wrong.
        (1..=self.items.len() + 1)
            .map(|n| format!("context-{n}"))
            .find(|id| self.get(id).is_none())
            .unwrap_or_else(|| "context-1".to_string())
    }

    /// The contexts as `(id, name)` pairs, for a picker.
    #[must_use]
    pub fn choices(&self) -> Vec<(String, String)> {
        self.items
            .iter()
            .map(|c| (c.id.clone(), c.display_name()))
            .collect()
    }
}

/// An open editor over one context: what the user has typed, not what is
/// stored.
///
/// Held apart from the list because it outlives a refetch. Saving is what
/// reconciles the two, and until then the daemon's copy and this one are
/// allowed to differ.
#[derive(Debug, Clone)]
pub struct ContextDraft {
    /// The id being edited. Fixed for the life of the draft — a rename edits
    /// [`name`](Self::name), never this, because the id is what the active
    /// selection and every per-backend pin point at.
    pub id: String,
    /// Whether this id is new, so the editor can say "New context" and the
    /// cancel path knows there is nothing to go back to.
    pub is_new: bool,
    pub name: String,
    /// The prompt, in the multi-line editor's own buffer.
    ///
    /// `text_editor::Content` rather than a `String` because the editor owns
    /// the cursor and the selection; re-seeding it on every refetch is what
    /// makes a cursor jump to the start mid-sentence.
    pub prompt: widget::text_editor::Content,
    /// One row per vocabulary term, each with a stable widget id so the row
    /// created by pressing Enter can be focused.
    ///
    /// A `Vec<(Id, String)>` rather than a `Vec<String>` for exactly that: the
    /// `Task` returned by `text_input::focus` needs an id that already belongs
    /// to a rendered row.
    pub terms: Vec<(widget::Id, String)>,
    /// A validation message from the daemon's last refused save, shown in the
    /// editor rather than as a page-level banner — it is about this form.
    pub error: Option<String>,
    /// Whether a save is in flight, so the button can say so and not be pressed
    /// twice.
    pub saving: bool,
}

impl ContextDraft {
    /// An editor over an existing context.
    #[must_use]
    pub fn editing(context: &DictationContext) -> Self {
        Self {
            id: context.id.clone(),
            is_new: false,
            name: context.name.clone(),
            prompt: widget::text_editor::Content::with_text(&context.prompt),
            terms: Self::rows(&context.vocabulary),
            error: None,
            saving: false,
        }
    }

    /// An editor over a context that does not exist yet.
    #[must_use]
    pub fn creating(id: String) -> Self {
        Self {
            id,
            is_new: true,
            name: String::new(),
            prompt: widget::text_editor::Content::new(),
            terms: Self::rows(&[]),
            error: None,
            saving: false,
        }
    }

    /// Term rows for `vocabulary`, plus the trailing empty row the next term
    /// goes in.
    ///
    /// The trailing row costs nothing, because blanks are dropped on save. It
    /// is what makes "add a term" a thing you type into rather than a button
    /// you have to find first.
    fn rows(vocabulary: &[String]) -> Vec<(widget::Id, String)> {
        vocabulary
            .iter()
            .map(|term| (widget::Id::unique(), term.clone()))
            .chain(std::iter::once((widget::Id::unique(), String::new())))
            .collect()
    }

    /// Insert an empty row after `index` and return its widget id, so the
    /// caller can focus it.
    pub fn insert_row_after(&mut self, index: usize) -> widget::Id {
        let id = widget::Id::unique();
        let at = (index + 1).min(self.terms.len());
        self.terms.insert(at, (id.clone(), String::new()));
        id
    }

    /// Remove row `index`, returning the id of the row to focus instead.
    ///
    /// Never removes the last remaining row: an editor with no rows at all has
    /// nowhere to type the next term, and the user would have to close and
    /// reopen it to get one back.
    pub fn remove_row(&mut self, index: usize) -> Option<widget::Id> {
        if self.terms.len() <= 1 || index >= self.terms.len() {
            return None;
        }
        self.terms.remove(index);
        let focus = index.saturating_sub(1);
        self.terms.get(focus).map(|(id, _)| id.clone())
    }

    /// Replace row `index` with `value`, splitting a pasted multi-line blob
    /// across rows.
    ///
    /// The split is the one real advantage a textarea had, kept: pasting a list
    /// someone had in a document should land as a list, not as one term with
    /// newlines in it.
    pub fn set_row(&mut self, index: usize, value: &str) {
        if index >= self.terms.len() {
            return;
        }
        let mut lines = value.split(['\n', '\r']).filter(|l| !l.trim().is_empty());
        let Some(first) = lines.next() else {
            // Cleared, or pasted whitespace: the row is now empty, which is
            // allowed — it is the trailing row's normal state.
            self.terms[index].1 = value.trim().to_string();
            return;
        };
        self.terms[index].1 = first.to_string();
        let rest: Vec<_> = lines
            .map(|line| (widget::Id::unique(), line.trim().to_string()))
            .collect();
        if !rest.is_empty() {
            let at = index + 1;
            self.terms.splice(at..at, rest);
        }
    }

    /// The context this draft would save, with blank rows dropped.
    #[must_use]
    pub fn to_context(&self) -> DictationContext {
        DictationContext {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            prompt: self.prompt.text(),
            vocabulary: DictationContext::clean_vocabulary(
                &self
                    .terms
                    .iter()
                    .map(|(_, term)| term.clone())
                    .collect::<Vec<_>>(),
            ),
        }
    }
}
