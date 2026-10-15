// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for source-`context` coverage of the JSON export
//! writer: the plaintext round trip, defensive secret masking, ciphertext
//! backup opacity, and crypto-shredded context never reappearing.
//!
//! ## Owns
//! - Context-focused export assertions over real in-memory SQLite stores
//!
//! ## Does not own
//! - Shared export fixtures (see `export_tests.rs`)
//! - Import-side restoration (see `import_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode};
use crate::erase::{EraseArgs, EraseVecSink, erase_request};
use crate::export::{ExportFilter, export};
use crate::export_mask::scrub_context;
use crate::export_tests::{
    Fixture, T, args, assert_emitted_contexts_are_bounded, ctx, dest_path, domain_context, fixture,
    item, read, seed_plain_domain_record_contexts, store,
};
use crate::inventory_fixture::FixtureDir;
use crate::test_unique::unique;
use clio_store::Store;
use clio_types::{Actor, Bank, MemoryItem, OpContext, SemanticCategory};

const SECRET: &str = "api_key: sk-test-1234567890";
const SECRET_MASKED: &str = "api_key: ****7890";

/// The shared `item` fixture with one descriptive `context` value set.
fn context_item(id: &str, bank: &str, subject: &str, context: &str) -> MemoryItem {
    let mut it = item(
        id,
        bank,
        subject,
        Some(SemanticCategory::parse("schema").unwrap()),
        None,
        None,
    );
    it.context = Some(context.to_owned());
    it
}

#[test]
fn plaintext_export_carries_the_exact_context() {
    let f = fixture("ctx-roundtrip");
    f.store
        .create_memory_item(
            &context_item("ctx-roundtrip-1", &f.bank, "sub-a", "support ticket 4815"),
            "tester",
        )
        .expect("seed context item");
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).expect("bundle parses");
    let stored = envelope
        .entities
        .items
        .iter()
        .find(|i| i.id == "ctx-roundtrip-1")
        .expect("context item present in the bundle");
    assert_eq!(stored.context.as_deref(), Some("support ticket 4815"));
}

#[test]
fn secret_shaped_context_is_scrubbed_in_the_bundle() {
    let f = fixture("ctx-secret");
    f.store
        .create_memory_item(
            &context_item(
                "ctx-secret-1",
                &f.bank,
                "sub-a",
                "api_key: sk-test-1234567890",
            ),
            "tester",
        )
        .expect("seed context item");
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let text = read(&f);
    assert!(
        !text.contains("sk-test-1234567890"),
        "raw secret must not reach the bundle"
    );
    let envelope: BundleEnvelope = serde_json::from_str(&text).expect("bundle parses");
    let stored = envelope
        .entities
        .items
        .iter()
        .find(|i| i.id == "ctx-secret-1")
        .expect("context item present in the bundle");
    assert_eq!(stored.context.as_deref(), Some("api_key: ****7890"));
}

#[test]
fn ciphertext_backup_keeps_context_inside_the_opaque_body() {
    let f = fixture("ctx-cipher");
    let context = "private ticket context";
    f.store
        .create_memory_item(
            &context_item("ctx-cipher-1", &f.bank, "sub-a", context),
            "tester",
        )
        .expect("seed context item");
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::CiphertextBackup,
    ))
    .expect("export succeeds");
    let text = read(&f);
    assert!(
        !text.contains(context),
        "context must stay sealed in the opaque body"
    );
    let envelope: BundleEnvelope = serde_json::from_str(&text).expect("bundle parses");
    assert!(
        envelope.entities.items.is_empty(),
        "{:?}",
        envelope.entities.items
    );
    assert!(
        envelope
            .entities
            .opaque_items
            .iter()
            .any(|o| o.id == "ctx-cipher-1")
    );
}

#[test]
fn erased_subject_context_never_reappears_in_plaintext_export() {
    let store = store();
    let bank = unique("ctx-erase-bank");
    let subject = unique("ctx-erase-subject");
    let item_id = unique("ctx-erased-item");
    let context = "context tied to the erased subject";
    store
        .create_memory_item(&context_item(&item_id, &bank, &subject, context), "tester")
        .expect("seed context item");
    let path = dest_path("ctx-erased");
    let f = Fixture {
        store,
        bank,
        dest: path.as_str().to_owned(),
        path,
    };
    let request = unique("req");
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(f.bank.clone()), Actor::Harness);
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    // The journal lives in a Drop-guarded directory, so it is removed with the
    // fixture instead of being left behind under the shared temp root.
    let journal_dir = FixtureDir::create(&unique("ctx-erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");
    let outcome = erase_request(&EraseArgs {
        store: &f.store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: &subject,
        legal_basis: "GDPR_ART_17",
        request_id: &request,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: T,
        sink: &sink,
    })
    .expect("erasure succeeds");
    assert_eq!(outcome.item_count, 1);

    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let text = read(&f);
    assert!(
        !text.contains(context),
        "shredded context must not reappear in the bundle"
    );
    let envelope: BundleEnvelope = serde_json::from_str(&text).expect("bundle parses");
    assert!(envelope.entities.items.iter().all(|i| i.id != item_id));
}

/// Chosen behavior for the scrub-emptied edge case: a `context` whose scrub
/// result is empty is exported/imported as absent, never as `Some("")`
/// (record validators reject an empty `context` on import).
#[test]
fn scrub_emptied_context_is_treated_as_absent() {
    assert_eq!(scrub_context(None), None);
    assert_eq!(scrub_context(Some("")), None);
    assert_eq!(
        scrub_context(Some("settled in a team chat")).as_deref(),
        Some("settled in a team chat")
    );
    assert_eq!(scrub_context(Some(SECRET)).as_deref(), Some(SECRET_MASKED));
}

/// Every domain-record `context` round-trips verbatim in
/// `dsar_plaintext` mode; masking removes secrets, never the value itself.
#[test]
fn plaintext_export_round_trips_each_domain_record_context() {
    let f = fixture("ctx-plain-domain");
    seed_plain_domain_record_contexts(&f);
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("export succeeds");
    let envelope: BundleEnvelope = serde_json::from_str(&read(&f)).expect("bundle parses");
    let e = &envelope.entities;
    assert_emitted_contexts_are_bounded(e);
    for (kind, id, expected) in [
        ("stable", "2026-05-01T00:00:00Z", "stable why"),
        ("preference", "brevity_affinity", "pref why"),
        ("belief", "belief-plain-1", "belief why"),
        ("task", "build-plain", "task why"),
        ("failure", "fr-fail-plain-1", "failure why"),
        ("triple", "triple-plain-1", "triple why"),
    ] {
        assert_eq!(
            domain_context(e, kind, id).as_deref(),
            Some(expected),
            "{kind} context"
        );
    }
}

#[cfg(test)]
#[path = "export_context_tests_domain_context_tests.rs"]
mod export_domain_context_tests;
