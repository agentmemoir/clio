// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the review preview `clio setup` draws before
//! anything is written: the new/unchanged/removed model, the read model the
//! renderer draws, and the block layout the plan's transcripts show.
//!
//! ## Owns
//! - `Change`, `ChangeState`, and `changes`, the new/unchanged/removed model
//! - `PreviewView`, the read model the renderer draws
//! - `preview`, the block layout and the `Will start`/`Tested` lines
//! - `display_name`, the wording for a config path
//!
//! ## Does not own
//! - The post-preview questions (see `setup_report_amend`)
//! - Producing the entry vector (see `setup_preview`)
//! - Reading the current effective values (the caller passes them in)
//! - Writing the config (see `setup_config`)
//!
//! ## Boundary
//!
//! Changes to the preview's shape or wording should normally require changes
//! here. Changes to what produces a value should not require changes here.
//!
//! Keep this module focused on showing what will happen.
//!
//! # Why the preview is built from the entry vector
//!
//! The plan names this as the failure it is written against: a preview built
//! from a parallel summary is a summary that eventually disagrees with what gets
//! written, and the disagreement is invisible until the config is wrong. So
//! [`changes`] takes the same `Vec<(String, Value)>` the apply path writes, and
//! this module holds no second derivation of any value.
//!
//! # Why secrets never appear
//!
//! A preview gets pasted into issues. So a secret is shown as present or absent
//! and never as characters: the caller masks it before the value reaches
//! [`changes`], so there is no path by which this module could print one.

use crate::cli_style::Palette;
use crate::setup_style::{confirm, title, warn};
use serde_json::Value;
use std::io::Write;

/// What will happen to one watched field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeState {
    /// The run adds it or changes it.
    New,
    /// The run writes the value it already holds.
    Unchanged,
    /// The run drops it, because the new choice no longer wants it.
    Removed,
}

impl ChangeState {
    /// The word the preview prints.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Unchanged => "unchanged",
            Self::Removed => "removed",
        }
    }
}

/// One field, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// The display name, e.g. `Database host`.
    pub path: String,
    /// The value in display form. A secret is already masked here.
    pub value: String,
    /// What the run will do to it.
    pub state: ChangeState,
    /// A second column: `not shown` for a secret, `not tested` for a service.
    pub note: Option<String>,
}

/// Group the planned entries against what the config holds today.
///
/// `current` answers "does the deployment file set this path, and to what". It is
/// a lookup rather than a document on purpose: the merged effective document
/// carries every schema default, so comparing against it would report a fresh
/// install as `unchanged` for every field, and an operator reading a preview
/// that claims nothing changed would agree to a write they never saw. Only the
/// file's own values count, which is the same rule `setup_config` uses when it
/// reports what is already configured.
///
/// Every planned entry becomes [`ChangeState::New`] or [`ChangeState::Unchanged`]
/// by comparing against it, and every watched path the run does not plan to
/// write becomes [`ChangeState::Removed`]. That last part is why `watched` is a
/// parameter: the set of fields a run cares about is the wizard's, not this
/// module's.
#[must_use]
pub(crate) fn changes(
    planned: &[(String, Value)],
    watched: &[&str],
    current: &dyn Fn(&str) -> Option<Value>,
    display: &dyn Fn(&str, &Value) -> String,
) -> Vec<Change> {
    let mut out: Vec<Change> = planned
        .iter()
        .map(|(path, value)| Change {
            path: display_name(path),
            value: display(path, value),
            state: match current(path) {
                Some(existing) if existing == *value => ChangeState::Unchanged,
                _ => ChangeState::New,
            },
            note: None,
        })
        .collect();
    for path in watched {
        let path = (*path).to_owned();
        if planned.iter().any(|(planned, _)| *planned == path) {
            continue;
        }
        if current(&path).is_none() {
            continue;
        }
        out.push(Change {
            path: display_name(&path),
            value: String::new(),
            state: ChangeState::Removed,
            note: Some("will be removed".to_owned()),
        });
    }
    out
}

/// Read a dotted config path out of a document.
///
/// `embed.url` is two levels of nesting, so `Value::get` on the whole string
/// would always miss. A test that wants a document-backed lookup builds one of
/// these; a production caller asks the runtime instead, because only the runtime
/// knows which values the file itself set, and this reads whatever a document
/// happens to hold.
#[cfg(test)]
#[must_use]
pub(crate) fn value_at(document: &Value, path: &str) -> Option<Value> {
    let mut cursor = document;
    for key in path.split('.') {
        cursor = cursor.get(key)?;
    }
    Some(cursor.clone())
}

/// Turn a dotted config path into the name the preview prints.
///
/// `store.database_url` reads as `Database address` in the preview, because the
/// preview presents the address as its parts (the plan's count rule depends on
/// the two can never disagree silently). The mapping is a lookup rather than
/// string surgery so an unmapped path falls through to something readable
/// instead of silently vanishing.
#[must_use]
pub(crate) fn display_name(path: &str) -> String {
    match path {
        "backend" => "Database backend".to_owned(),
        "store.database_url" => "Database address".to_owned(),
        "store.sqlite_path" => "SQLite file".to_owned(),
        other => other.replace(['.', '_'], " "),
    }
}

/// One block of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    /// The section name, drawn dim.
    pub name: &'static str,
    /// The fields in this section, in the order they are printed.
    pub changes: Vec<Change>,
}

/// One line of `Tested`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tested {
    /// Which thing was tested.
    pub what: &'static str,
    /// What came back.
    pub detail: String,
    /// False when the run could not test it, which prints a distinct marker.
    ///
    /// Named for what it records rather than for the struct: `tested` reads as
    /// though it were the test's own name, and the view already has a `tested`
    /// list, so the pair would be two different things under one word.
    #[allow(
        clippy::struct_field_names,
        reason = "the field records a test, the struct is a test's result"
    )]
    pub was_tested: bool,
}

/// The whole preview, as a read model.
///
/// Built from the entry vector and the current effective values, plus the two
/// facts the caller already knows: what the apply backs up, and what it
/// downloads. Both arrive as values rather than being derived here, so the
/// renderer draws a preview without reading a file or a directory. The renderer
/// draws this and never looks at an answer, so a value cannot appear in the
/// preview that was not in the vector the apply path writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewView {
    /// The blocks, in the order the plan prints them.
    pub blocks: Vec<Block>,
    /// What `Will fetch` will say, or `None` when the run downloads nothing.
    ///
    /// Drawn immediately before `Will start`, because a download happens before
    /// anything is written and the operator is agreeing to it at this moment.
    pub will_fetch: Option<String>,
    /// What `Will stop` will say, or `None` when the run goes on to write.
    ///
    /// Set when the run will refuse before it writes the config: the reason is
    /// printed here instead of promising a start that will not happen.
    pub will_stop: Option<String>,
    /// What `Will start` will say, when the run reaches a start at all.
    pub will_start: Vec<&'static str>,
    /// What `Will remove` will say.
    pub will_remove: Vec<String>,
    /// The backup path, or `None` on a fresh install.
    pub will_back_up: Option<String>,
    /// One line per test the run performed.
    pub tested: Vec<Tested>,
}

/// Draw the preview.
///
/// The layout is the plan's: a title, one block per configured section, then
/// `Will fetch`, `Will start`, `Will remove`, `Will back up`, and `Tested`. The
/// `Will fetch` line appears only when there is a download to announce, and a
/// run that needed none has no line rather than an empty one. Only configured
/// sections appear, so a run that skipped reranking has no `Reranking` block
/// rather than an empty one.
///
/// # Errors
/// The output stream could not be written.
pub(crate) fn preview(
    out: &mut dyn Write,
    palette: Palette,
    view: &PreviewView,
) -> Result<(), String> {
    let mut line = |text: String| -> Result<(), String> {
        writeln!(out, "  {text}").map_err(|e| format!("failed to write the preview: {e}"))
    };
    line(title(palette))?;
    line(String::new())?;
    for block in &view.blocks {
        let width = block
            .changes
            .iter()
            .map(|c| c.path.len())
            .max()
            .unwrap_or(0);
        let mut first = true;
        for change in &block.changes {
            let name = if first {
                first = false;
                block.name.to_owned()
            } else {
                " ".repeat(block.name.len())
            };
            let state = change.state.as_str();
            let mut row = format!(
                "{name:<w$}  {:<pw$}  {:<vw$}  {state}",
                change.path,
                change.value,
                w = width,
                pw = width,
                vw = 26,
            );
            if let Some(note) = &change.note {
                row.push_str("  ");
                row.push_str(note);
            }
            line(palette_optional(palette, row))?;
        }
    }
    line(String::new())?;
    if let Some(what) = &view.will_fetch {
        line(format!("Will fetch    {what}"))?;
    }
    if let Some(what) = &view.will_stop {
        line(format!("Will stop     {what}"))?;
    } else {
        line(format!(
            "Will start    {} after you apply",
            join_or_nothing(&view.will_start)
        ))?;
    }
    line(format!(
        "Will remove   {}",
        join_or_nothing(&view.will_remove)
    ))?;
    line(format!(
        "Will back up  {}",
        view.will_back_up
            .clone()
            .unwrap_or_else(|| "nothing, there is no config file yet".to_owned())
    ))?;
    if !view.tested.is_empty() {
        line(String::new())?;
        for entry in &view.tested {
            let marker = if entry.was_tested {
                confirm(palette, &entry.detail)
            } else {
                warn(palette, &format!("not tested ({})", entry.detail))
            };
            line(format!("Tested        {:<16} {marker}", entry.what))?;
        }
    }
    Ok(())
}

/// Paint a row green when it changes something and plain when it does not.
///
/// A run where nothing changed should look like nothing changed, so the `new` and
/// `removed` markers carry colour and `unchanged` does not.
fn palette_optional(palette: Palette, row: String) -> String {
    if row.ends_with(ChangeState::New.as_str()) || row.ends_with(ChangeState::Removed.as_str()) {
        confirm(palette, &row)
    } else {
        row
    }
}

/// `a, b, c`, or the plan's phrase when the list is empty.
fn join_or_nothing(items: &[impl std::fmt::Display]) -> String {
    if items.is_empty() {
        "nothing".to_owned()
    } else {
        items
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
#[path = "setup_report_tests.rs"]
mod tests;
