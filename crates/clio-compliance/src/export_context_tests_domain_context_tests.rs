// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for source-`context` coverage of the JSON export
//! writer's domain records: secret masking, ciphertext-backup opacity, and the
//! bounded emitted-context invariants across every context-bearing kind.
//!
//! ## Owns
//! - Domain-record context export assertions over real in-memory SQLite stores
//!
//! ## Does not own
//! - Shared export fixtures (see `export_tests.rs`)
//! - Item-level context export (see `export_context_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode};
use crate::export::{ExportFilter, export};
use crate::export_mask::OPAQUE_CONTENT;
use crate::export_tests::{
    Fixture, T, args, assert_emitted_contexts_are_bounded, belief, ctx, domain_context, failure,
    fixture, item, pref, read, stable, task, triple,
};
use clio_store::{BeliefStore, HistoryStore, PersonaStore, Store};
use clio_types::{Actor, EpisodicType};

use super::{SECRET, SECRET_MASKED};

/// Seeds one domain record of every context-bearing kind with the shared
/// secret-shaped `context`, plus one persona stable row with a valid
/// non-secret context for the round trip.
fn seed_domain_record_contexts(f: &Fixture) {
    let mut stable_secret = stable(&f.bank, "warm", "2026-03-01T00:00:00Z");
    stable_secret.context = Some(SECRET.to_owned());
    f.store.put_stable_entry(&stable_secret, "tester").unwrap();

    let mut plain = stable(&f.bank, "direct", "2026-04-01T00:00:00Z");
    plain.context = Some("settled in a team chat".to_owned());
    f.store.put_stable_entry(&plain, "tester").unwrap();

    let mut pref_secret = pref(&f.bank);
    pref_secret.context = Some(SECRET.to_owned());
    f.store
        .put_preference_state(&pref_secret, "tester")
        .unwrap();

    let mut belief_secret = belief(&f.bank);
    belief_secret.id = "belief-secret-1".into();
    belief_secret.proposition = "user rotates keys monthly".into();
    belief_secret.context = Some(SECRET.to_owned());
    f.store.create_belief(&belief_secret, "tester").unwrap();

    // Task/failure/triple records surface the carrier item's sealed `context`
    // on reads (the store join), so the secret must be seeded on the carrier.
    let (mut task_carrier, mut task_record) = task(&f.bank);
    task_carrier.id = "task-secret-1".into();
    task_carrier.context = Some(SECRET.to_owned());
    task_record.item_id = task_carrier.id.clone();
    task_record.id = "tr-task-secret-1".into();
    task_record.task_id = "build-secret".into();
    f.store
        .commit_task_upsert(&task_carrier, &task_record, "tester")
        .unwrap();

    let (mut fail_carrier, mut fail_record) = failure(&f.bank);
    fail_carrier.id = "fail-secret-1".into();
    fail_carrier.context = Some(SECRET.to_owned());
    fail_record.item_id = fail_carrier.id.clone();
    fail_record.id = "fr-fail-secret-1".into();
    fail_record.task_id = "build-secret".into();
    f.store
        .commit_failure_record(&fail_carrier, &fail_record, "tester")
        .unwrap();

    let mut tri_carrier = item(
        "tri-secret-1",
        &f.bank,
        "sub-c",
        None,
        Some(EpisodicType::Triple),
        None,
    );
    tri_carrier.context = Some(SECRET.to_owned());
    let mut triple_secret = triple(&f.bank);
    triple_secret.id = "triple-secret-1".into();
    triple_secret.item_id = tri_carrier.id.clone();
    triple_secret.predicate = "stores".into();
    triple_secret.object = "vault".into();
    f.store
        .commit_triple_add(&tri_carrier, &triple_secret, false, "tester")
        .unwrap();
}

/// No raw secret reaches the bundle in either mode: `dsar_plaintext` scrubs the
/// domain-record context, `ciphertext_backup` omits it entirely.
#[test]
fn domain_record_contexts_never_carry_raw_secrets_in_either_mode() {
    let f = fixture("ctx-domain-secret");
    seed_domain_record_contexts(&f);

    for mode in [
        ExportContentMode::DsarPlaintext,
        ExportContentMode::CiphertextBackup,
    ] {
        let ctx = ctx(&f, Actor::Harness);
        export(&args(&f, &ctx, ExportFilter::default(), mode)).expect("export succeeds");
        let text = read(&f);
        assert!(
            !text.contains("sk-test-1234567890"),
            "{mode:?}: raw secret in a domain-record context must not reach the bundle"
        );
    }
}

#[test]
fn dsar_export_masks_domain_record_contexts_but_keeps_them_present() {
    let f = fixture("ctx-domain-mask");
    seed_domain_record_contexts(&f);
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

    let secret_stable = e
        .persona_stable
        .iter()
        .find(|s| s.value == "warm")
        .expect("secret stable version exported");
    assert_eq!(secret_stable.context.as_deref(), Some(SECRET_MASKED));
    // A valid non-secret context stays present verbatim.
    let plain_stable = e
        .persona_stable
        .iter()
        .find(|s| s.value == "direct")
        .expect("plain stable version exported");
    assert_eq!(
        plain_stable.context.as_deref(),
        Some("settled in a team chat")
    );

    assert_eq!(
        e.persona_preferences[0].context.as_deref(),
        Some(SECRET_MASKED)
    );
    let belief = e
        .beliefs
        .iter()
        .find(|b| b.id == "belief-secret-1")
        .expect("secret belief exported");
    assert_eq!(belief.context.as_deref(), Some(SECRET_MASKED));
    let task = e
        .tasks
        .iter()
        .find(|t| t.task_id == "build-secret")
        .expect("secret task exported");
    assert_eq!(task.context.as_deref(), Some(SECRET_MASKED));
    let failure = e
        .failures
        .iter()
        .find(|fr| fr.id == "fr-fail-secret-1")
        .expect("secret failure exported");
    assert_eq!(failure.context.as_deref(), Some(SECRET_MASKED));
    let triple = e
        .triples
        .iter()
        .find(|t| t.id == "triple-secret-1")
        .expect("secret triple exported");
    assert_eq!(triple.context.as_deref(), Some(SECRET_MASKED));
}

/// A `ciphertext_backup` bundle keeps each domain record's metadata
/// inventory and carries no readable content for any of them.
///
/// Shared prose assertions for the backup-mode domain tests: every masked
/// prose field is content-free (the shared constant or a per-record sealed
/// placeholder) and identity-bearing fields stay distinct per record.
pub(crate) fn assert_masked_prose_is_content_free(e: &crate::bundle::ExportEntities) {
    let opaque_fields = e
        .triples
        .iter()
        .flat_map(|r| [&r.subject, &r.predicate, &r.object])
        .chain(e.tasks.iter().map(|r| &r.definition))
        .chain(e.failures.iter().map(|r| &r.what_failed))
        .chain(e.failures.iter().map(|r| &r.lesson))
        .chain(e.beliefs.iter().map(|r| &r.proposition))
        .chain(e.persona_stable.iter().map(|r| &r.value));
    assert!(
        opaque_fields
            .into_iter()
            .all(|field| field == OPAQUE_CONTENT
                || field.starts_with("[ciphertext_backup: content sealed ")),
        "every masked prose field must be content-free"
    );
    // Identity-bearing placeholders stay distinct per record, so a backup
    // bundle of several beliefs/triples still restores.
    let propositions: std::collections::HashSet<&str> =
        e.beliefs.iter().map(|r| r.proposition.as_str()).collect();
    assert_eq!(
        propositions.len(),
        e.beliefs.len(),
        "belief masking must not collapse identities"
    );
    let triple_identities: std::collections::HashSet<(&str, &str, &str)> = e
        .triples
        .iter()
        .map(|r| (r.subject.as_str(), r.predicate.as_str(), r.object.as_str()))
        .collect();
    assert_eq!(
        triple_identities.len(),
        e.triples.len(),
        "triple masking must not collapse identities"
    );
}

/// A `ciphertext_backup` bundle keeps each domain record's metadata
/// inventory and carries no readable content for any of them.
#[test]
fn backup_export_keeps_domain_record_metadata_without_readable_content() {
    let f = fixture("ctx-backup-domain");
    seed_domain_record_contexts(&f);
    let ctx = ctx(&f, Actor::Harness);
    let outcome = export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::CiphertextBackup,
    ))
    .expect("export succeeds");
    let text = read(&f);
    for leaked in [
        "sk-test-1234567890",
        "settled in a team chat",
        "user rotates keys monthly",
        "vault",
        "boom",
        "slow down",
        "step one",
        "\"concise\"",
        "\"warm\"",
        "\"direct\"",
    ] {
        assert!(
            !text.contains(leaked),
            "readable content `{leaked}` in a ciphertext backup"
        );
    }
    let envelope: BundleEnvelope = serde_json::from_str(&text).expect("bundle parses");
    let e = &envelope.entities;
    assert_emitted_contexts_are_bounded(e);
    for (kind, id) in [
        ("stable", "2026-03-01T00:00:00Z"),
        ("preference", "brevity_affinity"),
        ("belief", "belief-secret-1"),
        ("task", "build-secret"),
        ("failure", "fr-fail-secret-1"),
        ("triple", "triple-secret-1"),
    ] {
        assert_eq!(
            domain_context(e, kind, id),
            None,
            "{kind} context in a backup"
        );
    }
    // No readable prose survives: every record's prose field is a content-free
    // placeholder, and step lists are emptied. Identity-bearing fields (triple
    // subject/predicate/object, belief proposition) carry a per-record sealed
    // placeholder so masking cannot collapse distinct records onto one key;
    // non-identity prose carries the shared constant.
    assert_masked_prose_is_content_free(e);
    assert!(e.tasks.iter().all(|r| r.steps.is_empty()));
    // ...while the metadata inventory (ids, keys, timestamps, scores, enums) and
    // the evidence references stay complete.
    assert!(
        e.triples
            .iter()
            .any(|r| r.id == "triple-secret-1" && r.created_at == T)
    );
    assert!(
        e.tasks
            .iter()
            .any(|r| r.task_id == "build-secret" && r.status == "done")
    );
    assert!(
        e.failures
            .iter()
            .any(|r| r.id == "fr-fail-secret-1" && r.attempt_n == 1)
    );
    assert!(
        e.beliefs
            .iter()
            .any(|r| r.id == "belief-secret-1" && r.subject_id == "sub-a")
    );
    assert!(
        e.persona_stable
            .iter()
            .any(|r| r.key == "communication_style")
    );
    assert!(
        e.persona_preferences
            .iter()
            .any(|r| r.key == "brevity_affinity")
    );
    let counts = &outcome.manifest.counts_by_type;
    let stable_count = counts.iter().find(|(label, _)| label == "persona_stable");
    assert_eq!(
        stable_count.map(|(_, count)| *count),
        Some(e.persona_stable.len())
    );
}
