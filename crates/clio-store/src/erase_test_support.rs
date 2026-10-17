// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared fixtures for the compliance-erasure
//! store contract tests (SQLite and Postgres).
//!
//! ## Owns
//! - Item/MemTree/edge seed records for erasure tests
//! - The legacy-format row restore fixture (opaque `ciphertext_backup` route)
//! - The shared `assert_erased` and validation-arms assertions
//!
//! ## Does not own
//! - The erasure transaction under test (`sqlite_erase` / `postgres_erase`)
//! - Test case selection and scenario composition (`erase_tests`)
//!
//! ## Boundary
//!
//! Changes to erasure-test fixture shape should normally require changes here.
//! Changes to the production erasure path should not require changes here.
//!
//! Keep this module focused on erasure-test fixtures.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::{ComplianceStore, SubjectEraseSpec};
use crate::dek::{DekProvider, LocalDevKms};
use crate::distilled_unit::HUB_DISTILL_EDGE_RELATIONSHIP;
use crate::memtree_model::MemTreeNodeRecord;
use crate::model::{AssocEdgeRecord, AssocKind, OpaqueItemRecord, VectorSlot};
use crate::store::Store;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};

/// Fresh local development KMS for erasure tests.
pub(crate) fn local_kms() -> std::sync::Arc<dyn DekProvider> {
    std::sync::Arc::new(LocalDevKms::new())
}

/// Restore one legacy-format row (body decrypts but is not DualContent v1)
/// through the opaque `ciphertext_backup` route, which is how such a row
/// arrives from an exporting node. No current write path mints this shape.
pub(crate) fn restore_legacy_format_row(
    store: &dyn BatchStore,
    kms: &dyn DekProvider,
    id: &str,
    bank: &str,
    subject: &str,
) {
    let cipher = kms
        .encrypt(subject, b"legacy-opaque")
        .expect("legacy envelope")
        .to_storage_string();
    store
        .apply_batch(&[BatchWrite::PutOpaqueItem {
            item: OpaqueItemRecord {
                id: id.into(),
                bank_id: bank.into(),
                subject_id: subject.into(),
                kind: "episodic".into(),
                category: None,
                episodic_type: Some("temporal".into()),
                epistemic_kind: "fact".into(),
                source_type: None,
                confidence: None,
                admission_score: None,
                source_ref: None,
                valid_from: "t0".into(),
                valid_until: None,
                tx_from: "t0".into(),
                tx_until: None,
                created_at: "t0".into(),
                updated_at: "t0".into(),
                content_cipher: cipher,
            },
            actor: "actor-1".into(),
        }])
        .expect("legacy-format row restore");
}

/// One `BatchWrite::PutOpaqueItem` op that stores `cipher` verbatim for
/// `subject`, so an erasure test can exercise the opaque import/write route.
pub(crate) fn opaque_write(bank: &str, id: &str, subject: &str, cipher: &str) -> BatchWrite {
    BatchWrite::PutOpaqueItem {
        item: OpaqueItemRecord {
            id: id.into(),
            bank_id: bank.into(),
            subject_id: subject.into(),
            kind: "semantic".into(),
            category: Some("tool_config".into()),
            episodic_type: None,
            epistemic_kind: "fact".into(),
            source_type: None,
            confidence: Some(0.8),
            admission_score: Some(0.0),
            source_ref: Some("turn_9".into()),
            valid_from: "t0".into(),
            valid_until: None,
            tx_from: "t0".into(),
            tx_until: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            content_cipher: cipher.into(),
        },
        actor: "import".into(),
    }
}

pub(crate) static SALT: std::sync::LazyLock<[u8; 16]> = std::sync::LazyLock::new(rand::random);

pub(crate) use crate::test_unique::unique;

pub(crate) fn memory_item(bank: &str, id: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("persona").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "secret-value"})),
        gist: Some("secret gist text".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn memtree_leaf(bank: &str, node: &str, item: &str, parent: &str) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: node.into(),
        bank_id: bank.into(),
        parent_id: Some(parent.into()),
        item_id: Some(item.into()),
        depth: 0,
        interval_start: "2026-06-01".into(),
        interval_end: "2026-06-01".into(),
        child_ids: vec![],
        summary_gist: Some("secret gist text".into()),
        dirty: false,
        generation: 0,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
    }
}

fn memtree_parent(bank: &str, node: &str, child: &str) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: node.into(),
        bank_id: bank.into(),
        parent_id: None,
        item_id: None,
        depth: 1,
        interval_start: "2026-06-01".into(),
        interval_end: "2026-06-01".into(),
        child_ids: vec![child.into()],
        summary_gist: Some("aggregate secret gist".into()),
        dirty: false,
        generation: 0,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
    }
}

pub(crate) fn seed(store: &dyn Store, bank: &str, subject: &str) -> (String, String) {
    let a = unique("item-a");
    let b = unique("item-b");
    store
        .create_memory_item(&memory_item(bank, &a, subject), "tester")
        .unwrap();
    store
        .create_memory_item(&memory_item(bank, &b, subject), "tester")
        .unwrap();
    store
        .upsert_vector(&VectorSlot {
            item_id: a.clone(),
            bank_id: bank.into(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: vec![0.0; crate::model::EMBEDDING_DIMS],
        })
        .unwrap();
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: unique("edge"),
            bank_id: bank.into(),
            source_id: a.clone(),
            target_id: b.clone(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("rel".into()),
            weight: 1.0,
            last_updated_at: "2026-06-01T00:00:00Z".into(),
        })
        .unwrap();
    let leaf = unique("node-leaf");
    let parent = unique("node-parent");
    store
        .upsert_memtree_node(&memtree_leaf(bank, &leaf, &a, &parent))
        .unwrap();
    store
        .upsert_memtree_node(&memtree_parent(bank, &parent, &leaf))
        .unwrap();
    (a, parent)
}

pub(crate) fn spec<'a>(bank: &'a str, subject: &'a str, request: &'a str) -> SubjectEraseSpec<'a> {
    SubjectEraseSpec {
        bank_id: bank,
        subject_id: subject,
        request_id: request,
        legal_basis: "GDPR_ART_17",
        reason_code: "COMPLIANCE_CRYPTO_SHRED",
        deleted_at: "2026-09-17T11:00:00Z",
        salt: &SALT[..],
    }
}

pub(crate) fn assert_erased(
    store: &dyn Store,
    cs: &dyn ComplianceStore,
    bank: &str,
    subject: &str,
    request: &str,
    parent: &str,
    item_a: &str,
) {
    assert!(cs.subject_dek_destroyed(subject).unwrap());
    assert_eq!(
        store.get_memory_item(bank, item_a).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    let tombstones = cs.read_tombstones(bank, request).unwrap();
    assert_eq!(tombstones.len(), 2);
    for t in &tombstones {
        assert_eq!(t.subject_hash.len(), 64);
        assert_eq!(t.legal_basis, "GDPR_ART_17");
        assert_eq!(t.reason_code, "COMPLIANCE_CRYPTO_SHRED");
        assert_eq!(t.category.as_deref(), Some("persona"));
        let hash = t.item_id_hash.clone().expect("item hash");
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(subject));
    }
    let node = store
        .get_memtree_node(bank, parent)
        .unwrap()
        .expect("parent");
    assert!(node.dirty);
    assert!(node.summary_gist.is_none());
}

pub(crate) fn validation_arms(
    store: &dyn ComplianceStore,
    bank: &str,
    subject: &str,
    request: &str,
) {
    let mut sp = spec(bank, subject, request);
    sp.bank_id = "";
    assert_eq!(
        store.erase_subject(&sp).unwrap_err().code,
        ErrorCode::MissingBank
    );
    let mut sp = spec(bank, subject, request);
    sp.subject_id = "  ";
    assert_eq!(
        store.erase_subject(&sp).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let mut sp = spec(bank, subject, request);
    sp.request_id = "";
    assert_eq!(
        store.erase_subject(&sp).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let mut sp = spec(bank, subject, request);
    sp.legal_basis = "";
    assert_eq!(
        store.erase_subject(&sp).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    // An empty subject id also reaches the KMS guard through the probe.
    assert!(store.subject_dek_destroyed("").is_err());
}

pub(crate) fn seed_hub(store: &dyn Store, bank: &str, subject: &str) -> String {
    let hub = unique("hub-item");
    store
        .create_memory_item(&memory_item(bank, &hub, subject), "tester")
        .unwrap();
    hub
}

pub(crate) fn link_hub(store: &dyn Store, bank: &str, hub: &str, item: &str) {
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: unique("hub-edge"),
            bank_id: bank.into(),
            source_id: hub.into(),
            target_id: item.into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some(HUB_DISTILL_EDGE_RELATIONSHIP.into()),
            weight: 0.5,
            last_updated_at: "2026-06-01T00:00:00Z".into(),
        })
        .unwrap();
}

/// Seed one item + MemTree leaf for a subject in the given bank (ghost rows).
pub(crate) fn seed_ghost(store: &dyn Store, bank: &str, subject: &str) -> String {
    let item = unique("ghost-item");
    store
        .create_memory_item(&memory_item(bank, &item, subject), "tester")
        .unwrap();
    let leaf = unique("ghost-leaf");
    store
        .upsert_memtree_node(&MemTreeNodeRecord {
            id: leaf,
            bank_id: bank.into(),
            parent_id: Some("ghost-parent".into()),
            item_id: Some(item.clone()),
            depth: 0,
            interval_start: "2026-06-01".into(),
            interval_end: "2026-06-01".into(),
            child_ids: vec![],
            summary_gist: Some("secret gist text".into()),
            dirty: false,
            generation: 0,
            refreshing: false,
            task_id: None,
            last_error: None,
            created_at: "2026-06-01T00:00:00Z".into(),
            updated_at: "2026-06-01T00:00:00Z".into(),
        })
        .unwrap();
    item
}
