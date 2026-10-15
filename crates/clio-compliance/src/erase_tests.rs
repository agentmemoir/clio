// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the compliance erasure workflow tests:
//! authorization, the confirm gate, shred, and the presenting credential label
//! an event carries plus its absence on a local call. Test-only: never
//! compiled into production builds.
//!
//! ## Owns
//! - Behaviour assertions for `erase_request`, tombstone salts, and the
//!   tombstone view they produce
//! - The durable-denial assertions a completed erasure leaves in the journal
//! - Credential-label assertions on the erase event payload
//!
//! ## Does not own
//! - The erasure workflow itself (`erase.rs`)
//! - Reviewer audit row coverage (`validate_principal_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the erasure workflow changes; they never change it.
//!
//! Keep this module focused on the compliance erasure workflow.

use crate::erase::{
    DEFAULT_REASON_CODE, EraseArgs, EraseVecSink, erase_request, tombstone_salt,
    tombstone_salt_from,
};
use crate::erasure_journal::{JournalState, load_journal};
use crate::restore_guard::JournalHold;
use clio_store::{ComplianceStore, DekProvider, LocalDevKms, SqliteStore, Store, sha256_hex};
use clio_types::{
    Actor, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, PrincipalLabel,
    SemanticCategory,
};
use clio_write::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use clio_write::{DiscardArgs, DiscardVecSink, MemForest, MemTreeMaint, discard};
use std::sync::{Arc, Mutex};

use crate::inventory_fixture::FixtureDir;
use crate::test_unique::unique;

fn item(id: &str, bank: &str, subject: &str) -> MemoryItem {
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
        snapshot: Some(serde_json::json!({"entity": "secret"})),
        gist: Some("secret gist".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

struct Fixture {
    store: SqliteStore,
    bank: String,
    subject: String,
    item_id: String,
    /// The owner-only journal this fixture's erasures record into.
    journal: std::path::PathBuf,
    /// The per-test directory holding the journal, removed on drop.
    #[expect(dead_code, reason = "read only by its own drop")]
    dir: FixtureDir,
}

fn fixture() -> Fixture {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("erase-bank");
    let subject = unique("subject");
    let item_id = unique("item");
    store
        .create_memory_item(&item(&item_id, &bank, &subject), "tester")
        .unwrap();
    let dir = FixtureDir::create(&unique("erase-journal"));
    Fixture {
        store,
        bank,
        subject,
        item_id,
        journal: dir.path().join("erasure-journal.json"),
        dir,
    }
}

fn args<'a>(
    f: &'a Fixture,
    sink: &'a EraseVecSink,
    ctx: &'a OpContext,
    confirm: bool,
    request: &'a str,
    maint: Option<&'a Arc<MemTreeMaint>>,
) -> EraseArgs<'a> {
    // A fresh salt per process: no test asserts the tombstone hash value.
    static SALT: std::sync::LazyLock<[u8; 16]> = std::sync::LazyLock::new(rand::random);
    EraseArgs {
        store: &f.store,
        journal_path: &f.journal,
        maint,
        ctx,
        subject_id: &f.subject,
        legal_basis: "GDPR_ART_17",
        request_id: request,
        reason_code: None,
        confirm,
        salt: &SALT[..],
        timestamp: "2026-09-17T11:00:00Z",
        sink,
    }
}

fn ctx(f: &Fixture, actor: Actor) -> OpContext {
    OpContext::new(Bank::new(f.bank.clone()), actor)
}

/// The same reviewer, presented by a named credential.
fn bound_ctx(f: &Fixture, actor: Actor, label: &str) -> OpContext {
    OpContext::with_principal(Bank::new(f.bank.clone()), actor, PrincipalLabel::new(label))
}

#[test]
fn agent_and_unconfirmed_calls_are_rejected_without_state_change() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");

    let agent = ctx(&f, Actor::Agent);
    let err = erase_request(&args(&f, &sink, &agent, true, &request, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(!f.store.subject_dek_destroyed(&f.subject).unwrap());

    let harness = ctx(&f, Actor::Harness);
    let err = erase_request(&args(&f, &sink, &harness, false, &request, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(!f.store.subject_dek_destroyed(&f.subject).unwrap());
    let events = sink.events();
    assert!(events.is_empty(), "{events:?}");
    let tombstones = f.store.read_tombstones(&f.bank, &request).unwrap();
    assert!(tombstones.is_empty(), "{tombstones:?}");
}

#[test]
fn harness_confirm_shreds_and_propagates() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");

    // Seed the live MemTree with the same leaf so propagation is observable.
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    leaves.publish(item(&f.item_id, &f.bank, &f.subject));
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached(&f.item_id, &f.bank));

    let harness = ctx(&f, Actor::Harness);
    let outcome = erase_request(&args(&f, &sink, &harness, true, &request, Some(&maint))).unwrap();
    assert_eq!(outcome.status, "COMPLETED");
    assert_eq!(outcome.item_count, 1);
    assert_eq!(outcome.tombstones_written, 1);
    assert_eq!(outcome.request_id, request);
    assert_eq!(outcome.subject_hash.len(), 64);
    assert!(f.store.subject_dek_destroyed(&f.subject).unwrap());
    assert!(leaves.get(&f.bank, &f.item_id).is_none());

    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reason_code, DEFAULT_REASON_CODE);
    assert_eq!(events[0].item_count, 1);
    assert!(!events[0].subject_hash.contains(&f.subject));

    let err = f.store.get_memory_item(&f.bank, &f.item_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
}

#[test]
fn unknown_subject_is_not_found() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let sys = ctx(&f, Actor::System);
    let mut a = args(&f, &sink, &sys, true, &request, None);
    let missing = unique("missing");
    a.subject_id = &missing;
    let err = erase_request(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let tombstones = f.store.read_tombstones(&f.bank, &request).unwrap();
    assert!(tombstones.is_empty(), "{tombstones:?}");
}

#[test]
fn missing_required_arguments_are_rejected() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = ctx(&f, Actor::Harness);

    let mut a = args(&f, &sink, &harness, true, &request, None);
    a.subject_id = "  ";
    assert_eq!(
        erase_request(&a).unwrap_err().code,
        ErrorCode::InvalidArgument
    );

    let mut a = args(&f, &sink, &harness, true, &request, None);
    a.legal_basis = "";
    assert_eq!(
        erase_request(&a).unwrap_err().code,
        ErrorCode::InvalidArgument
    );

    let mut a = args(&f, &sink, &harness, true, &request, None);
    a.request_id = "";
    assert_eq!(
        erase_request(&a).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn tombstone_salt_prefers_configured_value() {
    assert_eq!(
        tombstone_salt_from(None).as_slice(),
        b"clio-compliance-tombstone-v1".as_slice()
    );
    assert_eq!(
        tombstone_salt_from(Some("custom".into())).as_slice(),
        b"custom".as_slice()
    );
    // The env wrapper falls back to the default when the variable is unset or
    // empty; either way it returns a non-empty salt.
    let salt = tombstone_salt();
    assert!(!salt.is_empty(), "{salt:?}");
}

#[test]
fn discard_does_not_destroy_dek() {
    let f = fixture();
    let sink = DiscardVecSink::default();
    let ctx = OpContext::new(Bank::new(f.bank.clone()), Actor::Harness);
    let removed = discard(&DiscardArgs {
        store: &f.store,
        ctx: &ctx,
        item_id: &f.item_id,
        reason: "ops cleanup",
        confirm: true,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    })
    .unwrap();
    assert!(matches!(removed, clio_write::DiscardOutcome::Discarded(_)));
    assert!(!f.store.subject_dek_destroyed(&f.subject).unwrap());
    assert_eq!(sink.events().len(), 1);
}

#[test]
fn a_bound_erasure_event_names_the_credential_beside_the_actor() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = bound_ctx(&f, Actor::Harness, "ops-key");
    erase_request(&args(&f, &sink, &harness, true, &request, None)).unwrap();
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].principal.as_deref(), Some("ops-key"));
    assert_eq!(events[0].actor, "harness", "the claimed role is unchanged");
    assert!(!events[0].subject_hash.contains(&f.subject));
}

#[test]
fn a_local_erasure_event_carries_no_credential() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = ctx(&f, Actor::Harness);
    erase_request(&args(&f, &sink, &harness, true, &request, None)).unwrap();
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].principal, None,
        "an unbound call must not leave an empty or null label"
    );
}

#[test]
fn an_absent_credential_is_omitted_from_the_serialized_event() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    erase_request(&args(
        &f,
        &sink,
        &ctx(&f, Actor::Harness),
        true,
        &request,
        None,
    ))
    .unwrap();
    let json = serde_json::to_value(&sink.events()[0]).expect("serialize event");
    assert!(
        json.get("principal").is_none(),
        "the key is absent, never null: {json}"
    );

    let bound_sink = EraseVecSink::default();
    let harness = bound_ctx(&f, Actor::Harness, "ops-key");
    erase_request(&args(&f, &bound_sink, &harness, true, &unique("req"), None)).unwrap();
    let json = serde_json::to_value(&bound_sink.events()[0]).expect("serialize event");
    assert_eq!(json["principal"], "ops-key");
}

/// A completed erasure leaves a durable denial keyed by the same one-way
/// subject hash the store's outcome carries.
#[test]
fn a_completed_erasure_denies_the_subject_in_the_durable_journal() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = ctx(&f, Actor::Harness);
    // A known salt, so this test can recompute the store's subject hash.
    let salt = rand::random::<[u8; 16]>();
    let mut a = args(&f, &sink, &harness, true, &request, None);
    a.salt = &salt;
    let outcome = erase_request(&a).unwrap();

    // The store derives `sha256_hex(salt, subject_id)`; the journal must hold
    // that same value, so the denial is keyed to the store's one-way hash.
    let expected = sha256_hex(&salt, &f.subject);
    assert_eq!(outcome.subject_hash, expected);
    let JournalState::Current(journal) = load_journal(&f.journal) else {
        panic!("a completed erasure must leave a current journal");
    };
    let denial = journal.denied(&expected).expect("the subject is denied");
    assert_eq!(journal.denials.len(), 1);
    assert_eq!(denial.banks, vec![f.bank.clone()]);
    assert!(journal.generation >= 1, "generation {}", journal.generation);
    assert_eq!(journal.deployment_id.len(), 64);
}

/// A damaged journal refuses the erasure before any store effect: the key
/// survives and the subject's item stays readable.
#[test]
fn a_damaged_journal_refuses_the_erasure_before_key_loss() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = ctx(&f, Actor::Harness);
    // A widened-mode, unparseable file: the journal layer classifies it
    // damaged and refuses to overwrite an authority it cannot trust.
    std::fs::write(&f.journal, b"{not a journal").expect("plant a damaged journal");
    let err = erase_request(&args(&f, &sink, &harness, true, &request, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(sink.events().is_empty(), "{:?}", sink.events());
    assert!(!f.store.subject_dek_destroyed(&f.subject).unwrap());
    let item = f.store.get_memory_item(&f.bank, &f.item_id).unwrap();
    assert!(item.is_some(), "the subject's item must stay readable");
}

/// While the placement lock is held, an erasure fails closed on the lock
/// before the durable denial or any store effect.
#[test]
fn a_held_journal_lock_refuses_the_erasure_before_any_effect() {
    let f = fixture();
    let sink = EraseVecSink::default();
    let request = unique("req");
    let harness = ctx(&f, Actor::Harness);
    let hold = JournalHold::acquire(&f.journal).expect("hold the placement lock");
    let err = erase_request(&args(&f, &sink, &harness, true, &request, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    drop(hold);
    assert!(sink.events().is_empty(), "{:?}", sink.events());
    assert!(!f.store.subject_dek_destroyed(&f.subject).unwrap());
    let tombstones = f.store.read_tombstones(&f.bank, &request).unwrap();
    assert!(tombstones.is_empty(), "{tombstones:?}");
    assert!(
        matches!(load_journal(&f.journal), JournalState::Missing),
        "the denial must not run while the lock is held"
    );
}
