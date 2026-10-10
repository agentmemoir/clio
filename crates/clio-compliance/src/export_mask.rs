// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the defensive secret masking of every
//! exported payload record before the export writer serializes it into a
//! bundle: memory items, triples, tasks, failures, beliefs,
//! and persona rows — their prose fields and every optional free-text
//! `context`.
//!
//! ## Owns
//! - `mask_item`: masked clone of the snapshot and inline scrubbing of the
//!   gist and `context` strings
//! - `mask_triples` / `mask_tasks` / `mask_failures` / `mask_beliefs` /
//!   `mask_assoc_edges` / `mask_stable` / `mask_preference`: the same
//!   scrubber applied to the bank-wide domain-record prose and `context`
//!   fields, per content mode
//! - `scrub_context`: the shared rule that a context which scrubs to an empty
//!   string, or which the scrub pushed over the published
//!   [`clio_types::CONTEXT_MAX_BYTES`] maximum, is exported as absent — never
//!   as an empty or over-long string
//! - `OPAQUE_CONTENT` and `sealed`: the content-free placeholders that
//!   `ciphertext_backup` mode writes instead of readable domain-record prose
//!
//! ## Does not own
//! - Item enumeration, filtering, and bundle assembly (`crate::export`)
//! - Secret detection heuristics (`clio_config::secret`)
//! - Import-side masking before a force overwrite (`crate::import_apply`)
//!
//! ## Boundary
//!
//! Changes to which exported fields are masked should normally require
//! changes here. Changes to the bundle layout or the masking heuristics
//! should not require changes here.
//!
//! Keep this module focused on masking exported payload records.

use crate::bundle::ExportContentMode;
use clio_config::secret::{masked_clone, scrub_inline_secrets};
use clio_types::{
    BeliefObject, CONTEXT_MAX_BYTES, FailureRecord, MemoryItem, PersonaPreference,
    PersonaStableEntry, TaskRecord, TripleRecord,
};

/// Content-free placeholder for the required prose fields of a domain record.
///
/// A `ciphertext_backup` bundle carries no DEKs, so it cannot carry readable
/// record content: the readable prose of a triple, task, failure, belief, or
/// persona row is replaced by a placeholder and the optional `context` is
/// dropped, while the record's metadata inventory (ids, keys, timestamps,
/// scores, enums, evidence refs) stays complete.
///
/// Two forms exist:
///
/// - [`OPAQUE_CONTENT`], the shared constant, for prose that is NOT part of a
///   record's identity (task definition, failure prose, persona stable value).
/// - [`sealed`], `[ciphertext_backup: content sealed {id}][:field]`, a
///   deterministic per-record (and per-field) placeholder for identity-bearing
///   prose (belief proposition, triple subject/predicate/object, assoc-edge
///   relationship). A constant would collapse distinct records onto one
///   identity key — for beliefs the identity is `(bank_id, proposition_key)`
///   and `proposition_key` lowercases the text — and make the bundle
///   unrestorable.
pub(crate) const OPAQUE_CONTENT: &str = "[ciphertext_backup: content sealed]";

/// Deterministic content-free placeholder for ONE prose field of one record:
/// distinct record ids (and field suffixes) yield distinct placeholders, and
/// the sealed prefix marks the text as masked content, not a real proposition.
fn sealed(id: &str, field: &str) -> String {
    if field.is_empty() {
        format!("[ciphertext_backup: content sealed {id}]")
    } else {
        format!("[ciphertext_backup: content sealed {id}:{field}]")
    }
}

/// Scrub the optional free-text `context` of any exported record.
///
/// A scrub result that is empty is treated as absent: record validators
/// reject an empty `context` on import, so an emptied scrub must never be
/// written as `Some("")`. A scrub can also EXPAND a value — a secret of four
/// or fewer characters becomes `[REDACTED]` — so the scrubbed result is
/// re-checked against the published [`CONTEXT_MAX_BYTES`] maximum and dropped
/// to absent when it exceeds it: a scrub must never push a value over the
/// bound that import validators enforce.
pub(crate) fn scrub_context(context: Option<&str>) -> Option<String> {
    context
        .map(scrub_inline_secrets)
        .filter(|c| !c.is_empty() && c.len() <= CONTEXT_MAX_BYTES)
}

/// The `context` an exported record carries in `mode`.
///
/// `dsar_plaintext` keeps the masked (scrubbed, bounded) value;
/// `ciphertext_backup` carries no readable record content, so the value is
/// absent entirely in that mode.
fn exported_context(context: Option<&str>, mode: ExportContentMode) -> Option<String> {
    match mode {
        ExportContentMode::DsarPlaintext => scrub_context(context),
        ExportContentMode::CiphertextBackup => None,
    }
}

/// Decrypt (already done by the store) then defensively mask content halves and
/// the descriptive source context.
pub(crate) fn mask_item(mut item: MemoryItem) -> MemoryItem {
    if let Some(snapshot) = item.snapshot.as_mut() {
        *snapshot = masked_clone(snapshot);
    }
    if let Some(gist) = item.gist.as_mut() {
        *gist = scrub_inline_secrets(gist);
    }
    item.context = scrub_context(item.context.as_deref());
    item
}

/// Scrub every triple's prose and optional `context`.
///
/// `dsar_plaintext` runs subject/predicate/object through the inline secret
/// scrubber (non-secret text passes through unchanged). In
/// `ciphertext_backup` mode the readable SPO content is replaced by per-record
/// [`sealed`] placeholders, so distinct triples keep distinct identities.
pub(crate) fn mask_triples(triples: &mut [TripleRecord], mode: ExportContentMode) {
    for triple in triples {
        triple.context = exported_context(triple.context.as_deref(), mode);
        match mode {
            ExportContentMode::CiphertextBackup => {
                triple.subject = sealed(&triple.id, "subject");
                triple.predicate = sealed(&triple.id, "predicate");
                triple.object = sealed(&triple.id, "object");
            }
            ExportContentMode::DsarPlaintext => {
                triple.subject = scrub_inline_secrets(&triple.subject);
                triple.predicate = scrub_inline_secrets(&triple.predicate);
                triple.object = scrub_inline_secrets(&triple.object);
            }
        }
    }
}

/// Scrub a task's definition, steps, and optional `context`.
///
/// In `ciphertext_backup` mode the definition becomes [`OPAQUE_CONTENT`] and
/// the step list is emptied: neither is carried as readable content.
pub(crate) fn mask_tasks(tasks: &mut [TaskRecord], mode: ExportContentMode) {
    for task in tasks {
        task.context = exported_context(task.context.as_deref(), mode);
        if mode == ExportContentMode::CiphertextBackup {
            OPAQUE_CONTENT.clone_into(&mut task.definition);
            task.steps.clear();
        } else {
            task.definition = scrub_inline_secrets(&task.definition);
            for step in &mut task.steps {
                *step = scrub_inline_secrets(step);
            }
        }
    }
}

/// Scrub a failure's prose fields and optional `context`.
///
/// In `ciphertext_backup` mode `what_failed` and `lesson` become
/// [`OPAQUE_CONTENT`].
pub(crate) fn mask_failures(failures: &mut [FailureRecord], mode: ExportContentMode) {
    for failure in failures {
        failure.context = exported_context(failure.context.as_deref(), mode);
        if mode == ExportContentMode::CiphertextBackup {
            OPAQUE_CONTENT.clone_into(&mut failure.what_failed);
            OPAQUE_CONTENT.clone_into(&mut failure.lesson);
        } else {
            failure.what_failed = scrub_inline_secrets(&failure.what_failed);
            failure.lesson = scrub_inline_secrets(&failure.lesson);
        }
    }
}

/// Scrub a belief's proposition and optional `context`.
///
/// In `ciphertext_backup` mode the proposition becomes a per-record [`sealed`]
/// placeholder (a shared constant would collapse every belief onto one
/// lowercased proposition key); the confidence trajectory (times, scores,
/// evidence refs) stays.
pub(crate) fn mask_beliefs(beliefs: &mut [BeliefObject], mode: ExportContentMode) {
    for belief in beliefs {
        belief.context = exported_context(belief.context.as_deref(), mode);
        if mode == ExportContentMode::CiphertextBackup {
            belief.proposition = sealed(&belief.id, "");
        } else {
            belief.proposition = scrub_inline_secrets(&belief.proposition);
        }
    }
}

/// Scrub an assoc edge's free-text prose per content mode.
///
/// `relationship` is the record's only free-text field (and a required
/// identity part for explicit edges), so `dsar_plaintext` scrubs it inline and
/// `ciphertext_backup` replaces it with a per-record [`sealed`] placeholder
/// The id/enum/weight/timestamp fields are metadata, not prose: they
/// stay untouched in both modes.
pub(crate) fn mask_assoc_edges(edges: &mut [clio_store::AssocEdgeRecord], mode: ExportContentMode) {
    for edge in edges {
        edge.relationship = match mode {
            ExportContentMode::DsarPlaintext => {
                edge.relationship.take().map(|r| scrub_inline_secrets(&r))
            }
            ExportContentMode::CiphertextBackup => edge
                .relationship
                .take()
                .map(|_| sealed(&edge.id, "relationship")),
        };
    }
}

/// Scrub a persona stable version's value and optional `context`.
///
/// In `ciphertext_backup` mode the value becomes [`OPAQUE_CONTENT`].
pub(crate) fn mask_stable(entry: &mut PersonaStableEntry, mode: ExportContentMode) {
    entry.context = exported_context(entry.context.as_deref(), mode);
    if mode == ExportContentMode::CiphertextBackup {
        OPAQUE_CONTENT.clone_into(&mut entry.value);
    } else {
        entry.value = scrub_inline_secrets(&entry.value);
    }
}

/// Scrub a persona preference row's optional `context`.
///
/// The numeric `value` and `trend` are scores, not prose: they stay in both
/// content modes as part of the record's metadata inventory.
pub(crate) fn mask_preference(pref: &mut PersonaPreference, mode: ExportContentMode) {
    pref.context = exported_context(pref.context.as_deref(), mode);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scrub can EXPAND a value (a secret of four or fewer characters
    /// becomes `[REDACTED]`), so a value that fits the published maximum before
    /// the scrub must never be written as an over-long `context` afterwards.
    #[test]
    fn scrub_context_drops_a_value_the_scrub_pushed_over_the_maximum() {
        let expanding = format!("{}token: a", "x".repeat(CONTEXT_MAX_BYTES - 8));
        assert_eq!(expanding.len(), CONTEXT_MAX_BYTES);
        assert_eq!(scrub_context(Some(&expanding)), None);

        let fitting = "y".repeat(CONTEXT_MAX_BYTES);
        assert_eq!(
            scrub_context(Some(&fitting)).map(|context| context.len()),
            Some(CONTEXT_MAX_BYTES)
        );
        assert_eq!(scrub_context(None), None);
        assert_eq!(scrub_context(Some("")), None);
    }
}
