// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pure per-item helpers `import_apply_keys.rs` owns: the
//! write id (borrowed in-bank, re-keyed across banks), the admission kind/key
//! pair for both item kinds, and the masking pass on a staged item.
//!
//! ## Owns
//! - `import_id` in both banks of its branch
//! - `kind_key` for semantic and episodic items, with and without a tag
//! - `masked_item` over the snapshot, gist, and context halves
//!
//! ## Does not own
//! - Planning order, dedupe, and gating (see `import_apply_tests.rs`)
//! - The secret-shape rules themselves (see `clio-config` and `export_mask`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::borrow::Cow;

use clio_types::{EpisodicType, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;

use super::{import_id, kind_key, masked_item};

const SECRET: &str = "api_key: sk-test-1234567890";
const SECRET_MASKED: &str = "api_key: ****7890";

fn item() -> MemoryItem {
    MemoryItem {
        id: "itm-1".into(),
        bank_id: "bank-a".into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::TaskSpec),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.88),
        source_ref: Some("turn-9".into()),
        snapshot: Some(json!({"deploy": "v1"})),
        gist: Some("deploys nightly".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: Some("settled in a team chat".into()),
    }
}

/// An in-bank re-import writes the bundle's own id, borrowed not reformatted.
#[test]
fn an_in_bank_import_borrows_the_bundle_id() {
    let id = import_id("bank-a", "bank-a", "itm-1");
    assert!(matches!(id, Cow::Borrowed("itm-1")), "{id:?}");
}

/// A cross-bank restore writes `{id}#{bank}` so the id stays deployment-unique
/// and a repeat restore into the same bank derives the same id.
#[test]
fn a_cross_bank_import_derives_the_id_from_the_target_bank() {
    let id = import_id("bank-b", "bank-a", "itm-1");
    assert!(matches!(id, Cow::Owned(_)), "{id:?}");
    assert_eq!(id, "itm-1#bank-b");
    assert_eq!(
        import_id("bank-b", "bank-a", "itm-1"),
        import_id("bank-b", "bank-c", "itm-1")
    );
}

/// A semantic item is admitted on its category.
#[test]
fn a_semantic_item_keys_on_its_category() {
    assert_eq!(kind_key(&item()), ("semantic", "task_spec"));
}

/// A semantic item with no category carries an empty key rather than a guess.
#[test]
fn a_semantic_item_without_a_category_keys_on_an_empty_string() {
    let mut no_category = item();
    no_category.category = None;
    assert_eq!(kind_key(&no_category), ("semantic", ""));
}

/// An episodic item is admitted on its episodic type, not its category.
#[test]
fn an_episodic_item_keys_on_its_episodic_type() {
    let mut episodic = item();
    episodic.kind = ItemKind::Episodic;
    episodic.category = None;
    episodic.episodic_type = Some(EpisodicType::Task);
    assert_eq!(kind_key(&episodic), ("episodic", "task"));
}

/// An episodic item with no type carries an empty key rather than a guess.
#[test]
fn an_episodic_item_without_a_type_keys_on_an_empty_string() {
    let mut episodic = item();
    episodic.kind = ItemKind::Episodic;
    episodic.category = None;
    episodic.episodic_type = None;
    assert_eq!(kind_key(&episodic), ("episodic", ""));
}

/// Every staged half is masked: the snapshot, the gist, and the context, while
/// an unsecret half passes through unchanged.
#[test]
fn staging_masks_the_snapshot_gist_and_context() {
    let mut staged = item();
    staged.snapshot = Some(json!({"api_key": "sk-test-1234567890"}));
    staged.gist = Some(SECRET.to_owned());
    staged.context = Some(SECRET.to_owned());

    let masked = masked_item(&staged);

    let snapshot = masked.snapshot.expect("snapshot kept").to_string();
    assert!(!snapshot.contains("sk-test-1234567890"), "{snapshot}");
    assert_eq!(masked.gist.as_deref(), Some(SECRET_MASKED));
    assert_eq!(masked.context.as_deref(), Some(SECRET_MASKED));
    assert_eq!(masked.id, "itm-1", "the id is never masked");
}

/// Absent halves stay absent, so staging cannot invent content.
#[test]
fn staging_keeps_absent_halves_absent() {
    let mut bare = item();
    bare.snapshot = None;
    bare.gist = None;
    bare.context = None;
    let masked = masked_item(&bare);
    assert_eq!(masked.snapshot, None);
    assert_eq!(masked.gist, None);
    assert_eq!(masked.context, None);
}

/// A context past the published bound is written as absent, never truncated.
#[test]
fn staging_writes_an_over_long_context_as_absent() {
    let mut long = item();
    long.context = Some("x".repeat(clio_types::CONTEXT_MAX_BYTES + 1));
    assert_eq!(masked_item(&long).context, None);
}

/// Masking clones: the caller's item is left untouched.
#[test]
fn staging_does_not_mutate_the_planner_item() {
    let mut staged = item();
    staged.gist = Some(SECRET.to_owned());
    let masked = masked_item(&staged);
    assert_eq!(
        staged.gist.as_deref(),
        Some(SECRET),
        "source item stays raw"
    );
    assert_eq!(masked.gist.as_deref(), Some(SECRET_MASKED));
}
