// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for test-only fixtures for the bundle import
//! tests: the SQLite harness, entity builders, and temp bundle-file writing.
//!
//! ## Owns
//! - `Harness` (store + bank + admission env + telemetry sinks)
//! - Entity builders for every restoreable bundle payload kind
//! - `write_bundle` (typed serialization + manifest checksum) and temp cleanup
//!
//! ## Does not own
//! - The test assertions (`import_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::import_fixture_signals::EmptySignals;
use std::path::PathBuf;
use std::sync::Arc;

use clio_admission::AdmissionPolicy;
use clio_admission::gated::VecSink;
use clio_config::ranking::RankingEnv;
use clio_store::{
    AssocEdgeRecord, ComplianceTombstoneView, DekProvider, ItemInspectFilter, LocalDevKms,
    SqliteStore, Store,
};
use clio_types::{
    Actor, Bank, BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, FailureRecord,
    ItemKind, MemoryItem, OpContext, PersonaPreference, PersonaStableEntry, SemanticCategory,
    SourceType, TaskRecord, TripleRecord,
};

use crate::bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, entities_checksum,
};
use crate::import::ImportArgs;

const TS: &str = "2026-06-01T00:00:00Z";
const EXPORTED_BANK: &str = "exported-bank";

/// Temporary bundle file removed on drop.
pub(super) struct TempBundle(pub(super) PathBuf);

impl Drop for TempBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl TempBundle {
    pub(super) fn text(&self) -> String {
        std::fs::read_to_string(&self.0).unwrap_or_default()
    }

    pub(super) fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

pub(super) use crate::test_unique::unique;

fn semantic_item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({ "k": "v" })),
        gist: Some("The deploy must run the smoke task checklist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

fn carrier(id: &str, bank: &str, subject: &str, tag: EpisodicType) -> MemoryItem {
    let mut it = semantic_item(id, bank, subject);
    it.kind = ItemKind::Episodic;
    it.category = None;
    it.episodic_type = Some(tag);
    it.confidence = None;
    it.admission_score = Some(0.5);
    it
}

fn triple(bank: &str, item_id: &str, subject: &str) -> TripleRecord {
    TripleRecord {
        context: None,
        id: format!("edge-{item_id}"),
        bank_id: bank.into(),
        item_id: item_id.into(),
        subject: subject.into(),
        predicate: "prefers".into(),
        object: "concise".into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: None,
    }
}

pub(super) fn entry(as_of: &str, confidence: f64, evidence: &str) -> ConfidenceEntry {
    ConfidenceEntry::new(as_of, confidence, evidence, SourceType::UserStated).unwrap()
}

fn belief(subject: &str, entries: Vec<ConfidenceEntry>) -> BeliefObject {
    BeliefObject {
        context: None,
        context_as_of: None,
        id: format!("bel-{subject}-x"),
        bank_id: EXPORTED_BANK.into(),
        subject_id: subject.into(),
        proposition: "User must use the deploy checklist task".into(),
        confidence_history: entries,
        created_at: TS.into(),
        updated_at: TS.into(),
    }
}

pub(super) struct Harness {
    pub(super) store: SqliteStore,
    pub(super) bank: String,
    pub(super) subject: String,
    pub(super) env: RankingEnv,
    pub(super) admit: VecSink,
}

impl Harness {
    pub(super) fn new() -> Self {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        Harness {
            store: SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap(),
            bank: unique("import-bank"),
            subject: unique("subject"),
            env: RankingEnv::system_defaults(),
            admit: VecSink::default(),
        }
    }

    pub(super) fn ctx(&self) -> OpContext {
        OpContext::new(Bank::new(self.bank.clone()), Actor::Agent)
    }

    pub(super) fn policy(&self) -> AdmissionPolicy<'_> {
        AdmissionPolicy::new(&self.env)
    }

    /// One bundle covering every restoreable entity kind, targeting a foreign
    /// bank id so the import must rewrite every entity into `ctx.bank`.
    #[allow(clippy::too_many_lines)]
    pub(super) fn full_entities(&self) -> ExportEntities {
        let bank = EXPORTED_BANK;
        let subject = &self.subject;
        let triple_id = unique("trp");
        let task_id = unique("tsk");
        let failure_id = unique("flr");
        let mut failure_carrier = carrier(&failure_id, bank, subject, EpisodicType::Failure);
        failure_carrier.epistemic_kind = EpistemicKind::Belief;
        failure_carrier.source_type = Some(SourceType::AgentInferred);
        ExportEntities {
            items: vec![
                semantic_item(&unique("itm"), bank, subject),
                carrier(&triple_id, bank, subject, EpisodicType::Triple),
                carrier(&task_id, bank, subject, EpisodicType::Task),
                failure_carrier,
            ],
            triples: vec![triple(bank, &triple_id, subject)],
            assoc_edges: vec![AssocEdgeRecord {
                id: unique("ae"),
                bank_id: bank.into(),
                source_id: "unused-a".into(),
                target_id: "unused-b".into(),
                assoc_kind: clio_store::AssocKind::Explicit,
                relationship: Some("relates".into()),
                weight: 0.4,
                last_updated_at: TS.into(),
            }],
            persona_stable: vec![PersonaStableEntry {
                context: None,
                bank_id: bank.into(),
                key: "communication_style".into(),
                value: "concise".into(),
                confidence: Some(0.8),
                source: "explicit".into(),
                source_ref: Some("turn_1".into()),
                admission_score: 0.8,
                as_of: TS.into(),
                valid_until: None,
            }],
            persona_preferences: vec![PersonaPreference {
                context: None,
                bank_id: bank.into(),
                key: "brevity_affinity".into(),
                value: 0.7,
                trend: 0.6,
                confidence: None,
                source: "explicit".into(),
                source_ref: None,
                admission_score: 0.7,
                as_of: TS.into(),
            }],
            tasks: vec![TaskRecord {
                context: None,
                id: format!("tr-{task_id}"),
                bank_id: bank.into(),
                item_id: task_id.clone(),
                task_id: format!("task-{task_id}"),
                subject_id: subject.into(),
                version: 1,
                definition: "fix the build".into(),
                steps: vec!["edit".into()],
                status: "in_progress".into(),
                evidence_ref: None,
                admission_score: 0.5,
                created_at: TS.into(),
            }],
            failures: vec![FailureRecord {
                context: None,
                id: format!("fr-{failure_id}"),
                bank_id: bank.into(),
                item_id: failure_id.clone(),
                task_id: format!("task-{failure_id}"),
                subject_id: subject.into(),
                attempt_n: 1,
                what_failed: "compile error".into(),
                lesson: "run cargo check first".into(),
                lesson_tokens: 5,
                evidence_ref: None,
                admission_score: 0.5,
                epistemic_kind: EpistemicKind::Belief,
                created_at: TS.into(),
            }],
            beliefs: vec![belief(subject, vec![entry("2026-01-01", 0.7, "turn_1")])],
            ..ExportEntities::default()
        }
    }

    pub(super) fn opaque_entities(&self) -> ExportEntities {
        ExportEntities {
            opaque_items: vec![OpaqueItem {
                id: unique("opq"),
                subject_id: self.subject.clone(),
                kind: "semantic".into(),
                category: Some("schema".into()),
                episodic_type: None,
                epistemic_kind: "fact".into(),
                source_type: None,
                confidence: Some(0.9),
                admission_score: Some(0.8),
                source_ref: None,
                valid_from: "2026-01-01T00:00:00Z".into(),
                valid_until: None,
                tx_from: "2026-01-01T00:00:00Z".into(),
                tx_until: None,
                created_at: TS.into(),
                updated_at: TS.into(),
                content_cipher: "opaque-at-rest-ciphertext-from-exporting-node".into(),
            }],
            ..ExportEntities::default()
        }
    }

    /// One admission-rejectable semantic item bundle (input).
    pub(super) fn reject_entities(&self) -> ExportEntities {
        ExportEntities {
            items: vec![semantic_item(&unique("itm"), EXPORTED_BANK, &self.subject)],
            ..ExportEntities::default()
        }
    }

    /// One admit-passing episodic `temporal` item (gated episodic path).
    pub(super) fn temporal_entities(&self) -> ExportEntities {
        ExportEntities {
            items: vec![carrier(
                &unique("itm"),
                EXPORTED_BANK,
                &self.subject,
                clio_types::EpisodicType::Temporal,
            )],
            ..ExportEntities::default()
        }
    }

    /// `count` admit-passing semantic items (for >50-op chunk coverage).
    pub(super) fn bulk_entities(&self, count: usize) -> ExportEntities {
        ExportEntities {
            items: (0..count)
                .map(|i| {
                    semantic_item(
                        &format!("bulk-{i}-{}", unique("x")),
                        EXPORTED_BANK,
                        &self.subject,
                    )
                })
                .collect(),
            ..ExportEntities::default()
        }
    }

    /// `count` triples whose carrier items are absent from the bundle.
    pub(super) fn missing_carrier_entities(&self, count: usize) -> ExportEntities {
        let subject = &self.subject;
        let triples = (0..count)
            .map(|i| {
                triple(
                    EXPORTED_BANK,
                    &format!("absent-{i}-{}", unique("x")),
                    subject,
                )
            })
            .collect::<Vec<_>>();
        ExportEntities {
            triples,
            ..ExportEntities::default()
        }
    }

    /// Tombstone provenance rows only.
    pub(super) fn tombstone_entities() -> ExportEntities {
        ExportEntities {
            tombstones: vec![ComplianceTombstoneView {
                request_id: unique("req"),
                legal_basis: "GDPR_ART_17".into(),
                subject_hash: "h1".into(),
                item_id_hash: Some("h2".into()),
                category: None,
                reason_code: "COMPLIANCE_CRYPTO_SHRED".into(),
                deleted_at: TS.into(),
            }],
            ..ExportEntities::default()
        }
    }

    /// Belief-only bundle with a caller-supplied trajectory.
    pub(super) fn belief_entities(&self, entries: Vec<ConfidenceEntry>) -> ExportEntities {
        ExportEntities {
            beliefs: vec![belief(&self.subject, entries)],
            ..ExportEntities::default()
        }
    }

    pub(super) fn args<'a>(
        &'a self,
        ctx: &'a OpContext,
        policy: AdmissionPolicy<'a>,
        source: &'a str,
        dry_run: bool,
        force: bool,
        reason: Option<&'a str>,
    ) -> ImportArgs<'a> {
        ImportArgs {
            store: &self.store,
            batch: &self.store,
            persona: &self.store,
            history: &self.store,
            belief: &self.store,
            ctx,
            source,
            dry_run,
            force,
            reason,
            policy,
            signals: &EmptySignals,
            sink: &self.admit,
            belief_signals: &EmptySignals,
            timestamp: TS,
        }
    }
}

pub(super) fn write_bundle(entities: &ExportEntities, mode: ExportContentMode) -> TempBundle {
    let envelope = BundleEnvelope {
        format: BUNDLE_FORMAT.to_owned(),
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: mode,
        manifest: CompletenessManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            content_mode: mode,
            bank_ids: vec![EXPORTED_BANK.to_owned()],
            counts_by_type: entities.counts_by_type(),
            counts_by_category: vec![],
            content_sha256: entities_checksum(entities),
            filter: serde_json::json!({}),
            generated_at: TS.into(),
            complete: true,
            omissions: vec![],
        },
        entities: entities.clone(),
    };
    let path = std::env::temp_dir().join(unique("am-import-bundle"));
    std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    TempBundle(path)
}

pub(super) fn count_items(h: &Harness) -> usize {
    let filter = ItemInspectFilter::new(&h.bank);
    h.store.count_items(&filter).unwrap()
}
