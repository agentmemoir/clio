// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the identity-collapse restore tests: a
//! `ciphertext_backup` bundle of a bank with several beliefs (and triples)
//! restores into a fresh store with every masked record keeping its own
//! distinct, content-free identity, and masked propositions still pass belief
//! validation on import.
//!
//! ## Owns
//! - End-to-end export→import round trips over real SQLite stores in
//!   `ciphertext_backup` mode
//!
//! ## Does not own
//! - Export-side masking assertions (`export_mask_identity_tests.rs`)
//! - Persona `context` force-scrub assertions (`import_persona_scrub_tests.rs`)
//! - Fixture builders (`export_tests.rs`, `import_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode, ExportEntities};
use crate::export::{ExportArgs, ExportFilter, export};
use crate::export_mask::{mask_beliefs, mask_triples};
use crate::export_tests::{DestPath, T, belief, dest_path, item, store, triple};
use crate::import::import;
use crate::import_fixture::{Harness, entry, write_bundle};
use crate::test_unique::unique;
use clio_store::{BeliefStore, SqliteStore, Store};
use clio_types::{
    Actor, Bank, BeliefObject, EpisodicType, OpContext, TripleRecord, proposition_key,
};

/// Seeds two distinct beliefs plus two distinct triples (with their carrier
/// items) — the bank shape the reproduction exported.
fn seed_two_beliefs_and_triples(store: &SqliteStore, bank: &str, subject: &str) {
    for (id, proposition) in [
        ("bel-f06-1", "user likes espresso"),
        ("bel-f06-2", "user prefers rye bread"),
    ] {
        store
            .create_belief(
                &BeliefObject {
                    id: id.into(),
                    bank_id: bank.into(),
                    subject_id: subject.into(),
                    proposition: proposition.into(),
                    context: None,
                    context_as_of: None,
                    confidence_history: vec![entry(T, 0.8, "turn-1")],
                    created_at: T.into(),
                    updated_at: T.into(),
                },
                "tester",
            )
            .unwrap();
    }
    for (triple_id, predicate, object) in [
        ("trp-f06-1", "likes", "oat milk"),
        ("trp-f06-2", "drinks", "black tea"),
    ] {
        let carrier = item(
            triple_id,
            bank,
            subject,
            None,
            Some(EpisodicType::Triple),
            None,
        );
        let mut record = triple(bank);
        record.id = format!("edge-{triple_id}");
        record.item_id = triple_id.into();
        record.predicate = predicate.into();
        record.object = object.into();
        store
            .commit_triple_add(&carrier, &record, false, "tester")
            .unwrap();
    }
}

/// Exports `bank` from `source` into a fresh temp file in `mode` and returns the
/// destination guard, which removes the file when it drops.
fn export_bundle(source: &SqliteStore, bank: &str, mode: ExportContentMode) -> DestPath {
    let dest = dest_path("f06-backup");
    let dest_str = dest.as_str().to_owned();
    let ctx = OpContext::new(Bank::new(bank.to_owned()), Actor::Harness);
    export(&ExportArgs {
        store: source,
        compliance: source,
        persona: source,
        history: source,
        belief: source,
        ctx: &ctx,
        destination: &dest_str,
        filter: ExportFilter::default(),
        content_mode: mode,
        timestamp: T,
        subject_id: None,
        facts: None,
        backend: None,
        delivery: None,
    })
    .expect("export succeeds");
    dest
}

/// Every triple's SPO fields carry the per-record, per-field sealed
/// placeholder, so distinct triples keep distinct identities after masking.
fn assert_triple_placeholders_distinct(triples: &[TripleRecord]) {
    assert!(triples.len() >= 2, "two or more triples are required");
    for t in triples {
        for (field, value) in [
            ("subject", t.subject.as_str()),
            ("predicate", t.predicate.as_str()),
            ("object", t.object.as_str()),
        ] {
            assert_eq!(
                value,
                format!("[ciphertext_backup: content sealed {}:{}]", t.id, field),
                "triple {} {field} must be a per-record placeholder",
                t.id
            );
        }
    }
    assert_ne!(triples[0].subject, triples[1].subject);
}

/// (approver reproduction): a bank with two beliefs exported in
/// `ciphertext_backup` mode used to collapse both propositions onto the one
/// constant `[ciphertext_backup: content sealed]`; importing that bundle into
/// a fresh database failed with `invalid_argument: belief already exists for
/// proposition key` (the UNIQUE index over `(bank_id, proposition_key)`).
/// The bundle must now restore one distinct, content-free identity per record.
#[test]
fn backup_bundle_of_two_beliefs_restores_into_a_fresh_store() {
    let source = store();
    let bank = unique("f06-src-bank");
    let subject = unique("f06-src-subject");
    seed_two_beliefs_and_triples(&source, &bank, &subject);
    let dest = export_bundle(&source, &bank, ExportContentMode::CiphertextBackup);

    // The exported bundle carries per-record placeholders, not one constant.
    let text = std::fs::read_to_string(dest.as_path()).unwrap();
    let envelope: BundleEnvelope = serde_json::from_str(&text).unwrap();
    let propositions: Vec<&str> = envelope
        .entities
        .beliefs
        .iter()
        .map(|b| b.proposition.as_str())
        .collect();
    assert_eq!(propositions.len(), 2);
    assert!(
        propositions
            .iter()
            .all(|p| p.starts_with("[ciphertext_backup: content sealed bel-f06-")),
        "every masked belief proposition must be a per-record placeholder"
    );
    assert_ne!(propositions[0], propositions[1]);
    assert_triple_placeholders_distinct(&envelope.entities.triples);

    // Restore into a fresh bank.
    let h = Harness::new();
    let report = import(&h.args(&h.ctx(), h.policy(), dest.as_str(), false, false, None))
        .expect("a ciphertext_backup bundle of two beliefs must restore cleanly");
    assert_eq!(report.would_create, 4, "two opaque carriers + two beliefs");
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert_eq!(beliefs.len(), 2, "both beliefs restore");
    let mut keys: Vec<String> = beliefs
        .iter()
        .map(|b| proposition_key(&b.proposition))
        .collect();
    keys.sort();
    assert_ne!(
        keys[0], keys[1],
        "restored belief identity keys stay distinct"
    );

    // Honest restore scope: a real backup bundle carries opaque item bodies
    // only, so the triple rows are skipped with a note (their carrier items
    // are absent as plaintext carriers).
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("carrier item missing from bundle")),
        "triple restore must be skipped with an honest note"
    );
    let triples = h.store.list_triples(&h.bank).unwrap();
    assert!(triples.is_empty(), "{triples:?}");
}

/// Masked backup records restore with distinct identities end to end.
/// The bundle carries plaintext carrier items so the triple row restore path
/// can run (a real backup bundle carries opaque item bodies instead; the
/// honest skip for that shape is asserted in the repro test above).
#[test]
fn masked_backup_rows_restore_with_distinct_identities() {
    let h = Harness::new();
    let bank = "exported-bank";
    let subject = "sub-f06";
    let carrier_a = unique("car");
    let carrier_b = unique("car");
    let mut rec_a = triple(bank);
    rec_a.id = "edge-f06-a".into();
    rec_a.item_id = format!("car-a-{carrier_a}");
    rec_a.object = "oat milk".into();
    let mut rec_b = triple(bank);
    rec_b.id = "edge-f06-b".into();
    rec_b.item_id = format!("car-b-{carrier_b}");
    rec_b.object = "black tea".into();
    let mut belief_a = belief(bank);
    belief_a.id = "bel-f06-a".into();
    belief_a.subject_id = subject.into();
    belief_a.proposition = "user likes espresso".into();
    let mut belief_b = belief(bank);
    belief_b.id = "bel-f06-b".into();
    belief_b.subject_id = subject.into();
    belief_b.proposition = "user prefers rye bread".into();
    let mut entities = ExportEntities {
        items: vec![
            item(
                &rec_a.item_id,
                bank,
                subject,
                None,
                Some(EpisodicType::Triple),
                None,
            ),
            item(
                &rec_b.item_id,
                bank,
                subject,
                None,
                Some(EpisodicType::Triple),
                None,
            ),
        ],
        triples: vec![rec_a, rec_b],
        beliefs: vec![belief_a, belief_b],
        ..ExportEntities::default()
    };
    mask_triples(&mut entities.triples, ExportContentMode::CiphertextBackup);
    mask_beliefs(&mut entities.beliefs, ExportContentMode::CiphertextBackup);

    let bundle = write_bundle(&entities, ExportContentMode::CiphertextBackup);
    let text = bundle.text();
    for readable in ["espresso", "rye bread", "oat milk", "black tea", "rust"] {
        assert!(
            !text.contains(readable),
            "readable content `{readable}` must not reach a backup bundle"
        );
    }

    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("the masked backup bundle must restore cleanly");
    assert_eq!(report.would_create, 6, "2 carriers + 2 triples + 2 beliefs");

    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert_eq!(beliefs.len(), 2);
    assert!(
        beliefs.iter().all(|b| b
            .proposition
            .starts_with("[ciphertext_backup: content sealed bel-f06-")),
        "each restored proposition is its own content-free placeholder"
    );
    let mut keys: Vec<String> = beliefs
        .iter()
        .map(|b| proposition_key(&b.proposition))
        .collect();
    keys.sort();
    assert_ne!(
        keys[0], keys[1],
        "restored belief identity keys stay distinct"
    );

    let triples = h.store.list_triples(&h.bank).unwrap();
    assert_eq!(triples.len(), 2, "both triples restore");
    let mut restored: Vec<&str> = triples.iter().map(|t| t.subject.as_str()).collect();
    restored.sort_unstable();
    let mut exported: Vec<&str> = entities
        .triples
        .iter()
        .map(|t| t.subject.as_str())
        .collect();
    exported.sort_unstable();
    assert_eq!(
        restored, exported,
        "the restore keeps each record's own content-free placeholder"
    );
    assert_ne!(restored[0], restored[1]);

    // The restore is idempotent: a re-import finds every identity and skips.
    let again = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("re-import succeeds");
    assert_eq!(
        again.would_create, 0,
        "every masked identity already exists"
    );
}
