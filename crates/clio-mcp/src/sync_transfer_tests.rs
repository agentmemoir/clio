// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP sync transfer tools' denial guard
//! (`sync_transfer.rs`): a pulled row for a denied subject is dead-lettered
//! before any write — for the pull and the bidirectional run — while a row for
//! an unrelated bank still applies, a journal path that cannot be resolved
//! refuses the transfer, and a resolved path with no file yet stays clear.
//!
//! ## Owns
//! - Behavior coverage for the transfer tools' guarded pull and bidirectional
//!   runs against a served peer
//! - The scratch journal fixture these transfer tests share
//!
//! ## Does not own
//! - The apply guard itself (`clio-sync` `apply_denial_tests`)
//! - The probe and its construction (`clio-compliance` `journal_denial_tests`)
//! - The transfer tools' argument shapes and key custody (see
//!   `sync_tools_tests` and `sync_tools_key_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the transfer tools' denial wiring changes; they never
//! change it.
//!
//! Keep this module focused on the transfer tools' denial guard.

use super::transfer_with;
use crate::runtime::McpState;
use crate::sync_tools::sync_tools_test_support_tests::state;
use clio_compliance::JournalDenialProbe;
use clio_store::{DeadLetterStatus, ErasureDenial};
use clio_sync::prelude::{ApplyEngine, ApplyOutcome, apply_mutation};
use clio_sync::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One scratch journal path, removed on drop: the crate's fixture pattern —
/// the pid and a nanosecond stamp keep parallel tests apart.
struct ScratchJournal(PathBuf);

impl ScratchJournal {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let file = format!("clio-mcp-{label}-{}-{nanos}.json", std::process::id());
        Self(std::env::temp_dir().join(file))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchJournal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The deployment environment naming `journal` as this host's erasure journal.
fn journal_env(journal: &ScratchJournal) -> impl Fn(&str) -> Option<String> + '_ {
    move |key: &str| {
        (key == clio_config::ERASURE_JOURNAL_FILE_ENV).then(|| journal.path().display().to_string())
    }
}

fn item(bank: &str, id: &str, subject: &str, stamp: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("sync-bridge-seed".into()),
        snapshot: Some(json!({ "kind": "seed", "id": id })),
        gist: Some(format!("seeded {id}")),
        valid_from: stamp.into(),
        valid_until: None,
        tx_from: stamp.into(),
        tx_until: None,
        created_at: stamp.into(),
        updated_at: stamp.into(),
        context: None,
    }
}

fn mutation(mid: &str, bank: &str, id: &str, subject: &str, stamp: &str) -> SyncMutation {
    let content =
        serde_json::to_value(item(bank, id, subject, stamp)).expect("serialize the seed item");
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: id.into(),
        op: OP_UPSERT.into(),
        transaction_time: stamp.into(),
        device_id: "dev-seed".into(),
        content: SyncContent::Plain(content),
        content_hash: format!("h-{mid}"),
        admission_receipt: None,
    }
}

/// Seed one bank's row into the peer's journal the way the serve push route
/// stores an accepted mutation: through the apply engine, sealed under the
/// peer's own keys, so a later pull serves plain content.
fn seed(peer: &McpState, m: &SyncMutation) {
    let envelope = serde_json::to_string(m).expect("serialize the seed mutation");
    let engine = ApplyEngine {
        store: peer.store.as_ref(),
        sync: peer.sync.as_ref(),
        device_id: "dev-seed".into(),
        kms: Some(peer.kms.as_ref()),
    };
    let outcome = apply_mutation(&engine, m, &envelope).expect("the seed mutation applies");
    assert_eq!(
        outcome,
        ApplyOutcome::Applied,
        "the premise: the seed applies"
    );
}

/// A serving peer holding one row for `b-held` and one for `b-open`, plus the
/// scratch journal a pulling client resolves. Nothing is recorded in the
/// journal here: a caller records the denial it wants to exercise.
fn seeded_peer() -> (Arc<McpState>, String, ScratchJournal) {
    let peer = state();
    seed(
        &peer,
        &mutation(
            "m-held",
            "b-held",
            "i-held",
            "s-held",
            "2026-01-01T00:00:00Z",
        ),
    );
    seed(
        &peer,
        &mutation(
            "m-open",
            "b-open",
            "i-open",
            "s-clear",
            "2026-01-01T00:00:01Z",
        ),
    );
    let journal = ScratchJournal::new("pull-denial");
    let addr = {
        let get = journal_env(&journal);
        let payload = crate::sync_tools::sync_serve_with(
            &peer,
            &json!({ "dev": true, "ttl_secs": 30, "banks": ["b-held", "b-open"] }),
            &get,
        )
        .expect("the peer starts serving");
        payload["addr"]
            .as_str()
            .expect("the bound address")
            .to_owned()
    };
    (peer, addr, journal)
}

/// The client-side facts after one guarded transfer: the denied row never
/// became readable and is dead-lettered, while the unrelated bank's row
/// applied and its cursor advanced.
fn assert_denied_subject_refused(client: &McpState) {
    assert!(
        client
            .store
            .get_memory_item("b-held", "i-held")
            .unwrap()
            .is_none(),
        "the denied row never became readable"
    );
    assert_eq!(
        client.sync.dead_letter_status("m-held").unwrap(),
        Some(DeadLetterStatus::Open),
        "the denied row is dead-lettered"
    );
    assert!(
        client
            .store
            .get_memory_item("b-open", "i-open")
            .unwrap()
            .is_some(),
        "the unrelated bank's row in the same run still applies"
    );
    assert!(
        client.sync.cursor_get("b-held", "pull").unwrap().is_none(),
        "the pull cursor never advances past a refusal"
    );
}

/// A pulled row for a denied subject is dead-lettered before any write while
/// an unrelated bank's row in the same pull still applies: the bridge's pull
/// runs under the journal authority, so a denial this host recorded is never
/// bypassed by a pulled replay.
#[test]
fn a_pulled_denied_subject_is_dead_lettered_while_an_unrelated_bank_applies() {
    let (_peer, addr, journal) = seeded_peer();
    let get = journal_env(&journal);
    JournalDenialProbe::new(journal.path().to_path_buf())
        .record_denial("s-held", &["b-held".to_owned()])
        .expect("record the subject denial");

    let client = state();
    let out = transfer_with(
        &client,
        &json!({ "remote": addr, "mode": "pull", "banks": ["b-held", "b-open"] }),
        "pull",
        &get,
    )
    .expect("the pull runs");
    assert_eq!(
        out["banks"].as_array().map(Vec::len),
        Some(2),
        "one pull carries two banks"
    );

    assert_denied_subject_refused(&client);
    assert!(
        client
            .sync
            .journal_get("b-held", "m-held")
            .unwrap()
            .is_none(),
        "a refused row is never journaled"
    );
}

/// The bidirectional run's pull half carries the same authority: a pulled
/// denied subject is dead-lettered rather than applied on the way into the
/// push half.
#[test]
fn a_bidirectional_transfer_dead_letters_a_pulled_denied_subject_too() {
    let (_peer, addr, journal) = seeded_peer();
    let get = journal_env(&journal);
    JournalDenialProbe::new(journal.path().to_path_buf())
        .record_denial("s-held", &["b-held".to_owned()])
        .expect("record the subject denial");

    let client = state();
    transfer_with(
        &client,
        &json!({ "remote": addr, "mode": "bidirectional", "banks": ["b-held", "b-open"] }),
        "pull",
        &get,
    )
    .expect("the bidirectional transfer runs");

    assert_denied_subject_refused(&client);
}

/// A journal path that cannot be resolved refuses both pull-apply modes before
/// any transport work: running them without the authority would ignore every
/// recorded denial, so the refusal names the override that fixes it.
#[test]
fn an_unresolvable_journal_path_refuses_the_pull_transfers() {
    let client = state();
    let env = |_key: &str| -> Option<String> { None };
    for mode in ["pull", "bidirectional"] {
        let result = transfer_with(
            &client,
            &json!({ "remote": "127.0.0.1:1", "mode": mode }),
            "pull",
            &env,
        );
        let error = result
            .err()
            .unwrap_or_else(|| panic!("{mode} must not run without its authority"));
        assert_eq!(error.code, ErrorCode::ConfigCorrupt, "{mode}: {error:?}");
        assert!(
            error
                .message
                .contains(clio_config::ERASURE_JOURNAL_FILE_ENV),
            "{mode}: the refusal names the override that fixes it: {error}"
        );
    }
}

/// A resolved journal path with no file yet is a fresh deployment: the bridge
/// pulls and applies rather than refusing on the absence of the file.
#[test]
fn a_resolved_journal_path_with_no_file_keeps_the_pull_clear() {
    let (_peer, addr, journal) = seeded_peer();
    let get = journal_env(&journal);
    assert!(
        !journal.path().exists(),
        "the premise: no journal file exists yet"
    );

    let client = state();
    transfer_with(
        &client,
        &json!({ "remote": addr, "mode": "pull", "banks": ["b-held"] }),
        "pull",
        &get,
    )
    .expect("the pull runs");
    assert!(
        client
            .store
            .get_memory_item("b-held", "i-held")
            .unwrap()
            .is_some(),
        "no recorded denial covers the subject"
    );
}
