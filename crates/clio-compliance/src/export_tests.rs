// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared export test fixtures over an
//! in-memory SQLite store: seed data, export argument builders, and the
//! end-to-end export driver used by the export scenario suites.
//!
//! ## Owns
//! - Export test fixtures (`Fixture`, item/triple/persona/history seeds) and
//!   the domain-record context seeder
//! - The `export_at` driver, bundle read helper, and emitted-context check
//!
//! ## Does not own
//! - Export scenarios (see `export_completeness_tests`, `export_context_tests`,
//!   `export_security_tests`, `export_truncation_tests`)
//! - The export writer itself (see `export.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::ExportContentMode;
use crate::export::{ExportArgs, ExportFilter, export};
use clio_store::{
    AssocEdgeRecord, AssocKind, BeliefStore, ComplianceStore, DekProvider, DestinationCustodyRow,
    DestinationCustodyStore, HistoryStore, LocalDevKms, LocalEventIntent, LocalEventStore,
    PersonaStore, SqliteStore, Store, SubjectEraseSpec,
};
use clio_types::{
    Actor, Bank, BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, FailureRecord,
    ItemKind, MemoryItem, OpContext, PersonaPreference, PersonaStableEntry, SemanticCategory,
    SourceType, TaskRecord, TripleRecord,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) const T: &str = "2026-06-01T00:00:00Z";

pub(crate) use crate::export_dest_fixtures::{DestPath, dest_path};
use crate::test_unique::unique;

pub(crate) fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

pub(crate) fn item(
    id: &str,
    bank: &str,
    subject: &str,
    category: Option<SemanticCategory>,
    episodic: Option<EpisodicType>,
    snapshot: Option<Value>,
) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: if category.is_some() {
            ItemKind::Semantic
        } else {
            ItemKind::Episodic
        },
        category,
        episodic_type: episodic,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        context: None,
        snapshot,
        gist: Some("plain gist".into()),
        valid_from: T.into(),
        valid_until: None,
        tx_from: T.into(),
        tx_until: None,
        created_at: T.into(),
        updated_at: T.into(),
    }
}

pub(crate) fn triple(bank: &str) -> TripleRecord {
    TripleRecord {
        context: None,
        id: "triple-1".into(),
        bank_id: bank.into(),
        item_id: "tri-1".into(),
        subject: "user".into(),
        predicate: "likes".into(),
        object: "rust".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        valid_from: T.into(),
        valid_until: None,
        tx_from: T.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: T.into(),
        updated_at: None,
    }
}

pub(crate) fn stable(bank: &str, value: &str, as_of: &str) -> PersonaStableEntry {
    PersonaStableEntry {
        context: None,
        bank_id: bank.into(),
        key: "communication_style".into(),
        value: value.into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: 0.9,
        as_of: as_of.into(),
        valid_until: None,
    }
}

pub(crate) fn pref(bank: &str) -> PersonaPreference {
    PersonaPreference {
        context: None,
        bank_id: bank.into(),
        key: "brevity_affinity".into(),
        value: 0.5,
        trend: 0.4,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: 0.6,
        as_of: T.into(),
    }
}

pub(crate) fn task(bank: &str) -> (MemoryItem, TaskRecord) {
    let carrier = item(
        "task-1",
        bank,
        "sub-a",
        None,
        Some(EpisodicType::Task),
        None,
    );
    let record = TaskRecord {
        context: None,
        id: "task-rec-1".into(),
        bank_id: bank.into(),
        item_id: "task-1".into(),
        task_id: "build".into(),
        subject_id: "sub-a".into(),
        version: 1,
        definition: "definition".into(),
        steps: vec!["step one".into()],
        status: "done".into(),
        evidence_ref: None,
        admission_score: 0.7,
        created_at: T.into(),
    };
    (carrier, record)
}

pub(crate) fn failure(bank: &str) -> (MemoryItem, FailureRecord) {
    let mut carrier = item(
        "fail-1",
        bank,
        "sub-a",
        None,
        Some(EpisodicType::Failure),
        None,
    );
    carrier.epistemic_kind = EpistemicKind::Belief;
    carrier.source_type = Some(SourceType::AgentInferred);
    let record = FailureRecord {
        context: None,
        id: "fail-rec-1".into(),
        bank_id: bank.into(),
        item_id: "fail-1".into(),
        task_id: "build".into(),
        subject_id: "sub-a".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "slow down".into(),
        lesson_tokens: 2,
        evidence_ref: None,
        admission_score: 0.7,
        epistemic_kind: EpistemicKind::Belief,
        created_at: T.into(),
    };
    (carrier, record)
}

pub(crate) fn belief(bank: &str) -> BeliefObject {
    BeliefObject {
        context: None,
        context_as_of: None,
        id: "belief-1".into(),
        bank_id: bank.into(),
        subject_id: "sub-a".into(),
        proposition: "user prefers tests".into(),
        confidence_history: vec![
            ConfidenceEntry::new(T, 0.8, "turn-1", SourceType::UserStated).unwrap(),
        ],
        created_at: T.into(),
        updated_at: T.into(),
    }
}

pub(crate) fn seed(store: &SqliteStore, bank: &str) {
    store
        .create_memory_item(
            &item(
                "sem-1",
                bank,
                "sub-a",
                Some(SemanticCategory::parse("schema").unwrap()),
                None,
                Some(json!({"api_key": "sk-test-1234567890"})),
            ),
            "tester",
        )
        .unwrap();
    store
        .create_memory_item(
            &item("epi-1", bank, "sub-b", None, Some(EpisodicType::Gist), None),
            "tester",
        )
        .unwrap();
    let carrier = item(
        "tri-1",
        bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    store
        .commit_triple_add(&carrier, &triple(bank), false, "tester")
        .unwrap();
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "edge-1".into(),
            bank_id: bank.into(),
            source_id: "sem-1".into(),
            target_id: "epi-1".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: None,
            weight: 0.5,
            last_updated_at: T.into(),
        })
        .unwrap();
    store
        .put_stable_entry(&stable(bank, "terse", "2026-01-01T00:00:00Z"), "tester")
        .unwrap();
    store
        .put_stable_entry(&stable(bank, "brief", "2026-02-01T00:00:00Z"), "tester")
        .unwrap();
    store.put_preference_state(&pref(bank), "tester").unwrap();
    let (task_carrier, task_record) = task(bank);
    store
        .commit_task_upsert(&task_carrier, &task_record, "tester")
        .unwrap();
    let (fail_carrier, fail_record) = failure(bank);
    store
        .commit_failure_record(&fail_carrier, &fail_record, "tester")
        .unwrap();
    store.create_belief(&belief(bank), "tester").unwrap();
    store
        .create_memory_item(
            &item(
                "gone-1",
                bank,
                "sub-gone",
                Some(SemanticCategory::parse("persona").unwrap()),
                None,
                None,
            ),
            "tester",
        )
        .unwrap();
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    store
        .erase_subject(&SubjectEraseSpec {
            bank_id: bank,
            subject_id: "sub-gone",
            request_id: "req-1",
            legal_basis: "GDPR_ART_17",
            reason_code: "COMPLIANCE_CRYPTO_SHRED",
            deleted_at: T,
            salt: &salt,
        })
        .unwrap();
}

pub(crate) struct Fixture {
    pub(crate) store: SqliteStore,
    pub(crate) bank: String,
    pub(crate) dest: String,
    pub(crate) path: DestPath,
}

pub(crate) fn fixture(name: &str) -> Fixture {
    let store = store();
    let bank = unique("bank");
    seed(&store, &bank);
    let path = dest_path(name);
    let dest = path.as_str().to_owned();
    Fixture {
        store,
        bank,
        dest,
        path,
    }
}

pub(crate) fn ctx(f: &Fixture, actor: Actor) -> OpContext {
    OpContext::new(Bank::new(f.bank.clone()), actor)
}

pub(crate) fn args<'a>(
    f: &'a Fixture,
    ctx: &'a OpContext,
    filter: ExportFilter,
    mode: ExportContentMode,
) -> ExportArgs<'a> {
    ExportArgs {
        store: &f.store,
        compliance: &f.store,
        persona: &f.store,
        history: &f.store,
        belief: &f.store,
        ctx,
        destination: &f.dest,
        filter,
        content_mode: mode,
        timestamp: "2026-09-20T00:00:00Z",
        subject_id: None,
        facts: None,
        backend: None,
        delivery: None,
    }
}

pub(crate) fn export_at(
    f: &Fixture,
    dest: &str,
) -> Result<crate::export::ExportOutcome, clio_types::AmError> {
    let ctx = ctx(f, Actor::Harness);
    let args = ExportArgs {
        destination: dest,
        ..args(
            f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    };
    export(&args)
}

pub(crate) fn read(f: &Fixture) -> String {
    std::fs::read_to_string(f.path.as_path()).unwrap()
}

/// A scratch root holding a staging directory and a destination path.
pub(crate) struct Scratch {
    pub(crate) root: PathBuf,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(unique("clio-compliance-export-subject"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    pub(crate) fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    pub(crate) fn destination(&self) -> PathBuf {
        self.root.join("out.bin")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The entries of a staging directory, sorted; empty when it is absent.
pub(crate) fn staging_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(read) => read.map(|entry| entry.expect("dir entry").path()).collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries
}

/// A pending custody row and a matching event intent for `action_id`.
pub(crate) fn seed_pending(
    events: &dyn LocalEventStore,
    custody: &dyn DestinationCustodyStore,
    action_id: &str,
    destination: &Path,
) {
    events
        .record_intent(&LocalEventIntent {
            action_id,
            actor: "tester",
            scope: "export_custody",
            kind: "export_delivery",
            event_time: T,
        })
        .expect("record intent");
    custody
        .save_pending(&DestinationCustodyRow {
            action_id: action_id.to_owned(),
            destination_path: destination.display().to_string(),
            custody_owner: "clio".to_owned(),
            required_action: "delivery".to_owned(),
            state: "pending".to_owned(),
            exception_reason: None,
            updated_at: T.to_owned(),
        })
        .expect("save pending");
}

#[cfg(test)]
#[path = "export_domain_fixtures_tests.rs"]
mod export_domain_fixtures_tests;

pub(crate) use export_domain_fixtures_tests::{
    assert_emitted_contexts_are_bounded, domain_context, seed_plain_domain_record_contexts,
};

#[cfg(test)]
#[path = "export_tests_mask_identity_tests.rs"]
mod export_mask_identity_tests;
