// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the persona import `context` scrub
//! tests: a forced (or plain) persona stable/preference import stages the same
//! scrubbed, bounded `context` the export masking produces, and a context the
//! scrub pushes over the published maximum drops to absent.
//!
//! ## Owns
//! - Persona force-branch and plain-create import assertions over the shared
//!   `import_fixture` harness and the real export writer
//!
//! ## Does not own
//! - The persona planners (`import_apply_persona.rs`)
//! - The export-side masking contract (`export_mask_identity_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::bundle::{BundleEnvelope, ExportContentMode, ExportEntities};
use crate::export::{ExportArgs, ExportFilter, export};
use crate::export_mask::scrub_context;
use crate::export_tests::{T, dest_path, store};
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};
use crate::test_unique::unique;
use clio_store::PersonaStore;
use clio_types::{
    Actor, Bank, CONTEXT_MAX_BYTES, OpContext, PersonaPreference, PersonaStableEntry,
};

const SECRET: &str = "api_key: sk-test-1234567890";
const SECRET_MASKED: &str = "api_key: ****7890";

fn stable_row(bank: &str, context: Option<&str>) -> PersonaStableEntry {
    PersonaStableEntry {
        context: context.map(str::to_owned),
        bank_id: bank.into(),
        key: "communication_style".into(),
        value: "concise".into(),
        confidence: Some(0.8),
        source: "explicit".into(),
        source_ref: Some("turn_1".into()),
        admission_score: 0.8,
        as_of: T.into(),
        valid_until: None,
    }
}

fn pref_row(bank: &str, context: Option<&str>) -> PersonaPreference {
    PersonaPreference {
        context: context.map(str::to_owned),
        bank_id: bank.into(),
        key: "brevity_affinity".into(),
        value: 0.7,
        trend: 0.6,
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: 0.7,
        as_of: T.into(),
    }
}

fn persona_entities(
    stable: Vec<PersonaStableEntry>,
    preferences: Vec<PersonaPreference>,
) -> ExportEntities {
    ExportEntities {
        persona_stable: stable,
        persona_preferences: preferences,
        ..ExportEntities::default()
    }
}

/// Seeds one stable row and one preference row in `bank`, both with a plain
/// non-secret `context`, so the bundle rows overwrite under force.
fn seed_plain_persona_rows(h: &Harness) {
    h.store
        .put_stable_entry(&stable_row(&h.bank, Some("old why")), "tester")
        .unwrap();
    h.store
        .put_preference_state(&pref_row(&h.bank, Some("old pref why")), "tester")
        .unwrap();
}

/// Additional: a forced persona import must never persist an unscrubbed
/// `context` — the stable and preference force branches stage the same
/// scrubbed value the memory-item force branch produces.
#[test]
fn forced_persona_import_scrubs_secret_shaped_context() {
    let h = Harness::new();
    seed_plain_persona_rows(&h);

    let bundle = write_bundle(
        &persona_entities(
            vec![stable_row("exported-bank", Some(SECRET))],
            vec![pref_row("exported-bank", Some(SECRET))],
        ),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(
        &h.ctx(),
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("ops restore"),
    ))
    .expect("force import succeeds");
    assert_eq!(report.would_overwrite, 2, "both persona rows overwrite");

    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert!(
        history
            .iter()
            .any(|e| e.context.as_deref() == Some(SECRET_MASKED)),
        "the forced stable context must persist scrubbed, got {:?}",
        history
            .iter()
            .map(|e| e.context.as_deref())
            .collect::<Vec<_>>()
    );
    assert!(
        !history.iter().any(|e| e.context.as_deref() == Some(SECRET)),
        "the raw secret must never persist"
    );
    let stored = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(
        stored.context.as_deref(),
        Some(SECRET_MASKED),
        "the forced preference context must persist scrubbed"
    );
}

/// additional: a `context` the scrub pushes over
/// [`CONTEXT_MAX_BYTES`] drops to absent on a persona import — it is never
/// staged over the bound import validators enforce, and nothing unscrubbed or
/// over-bound persists. On a fresh bank (no prior version to carry the
/// context forward) the restored row's `context` is absent; on a force
/// overwrite the store's ordinary-update semantics may carry the PRIOR stored
/// context forward, which is a bounded value the store already held.
#[test]
fn persona_force_context_pushed_over_the_maximum_drops_to_absent() {
    // Fits the bound before the scrub; the scrub expands `token: a` past it.
    let expanding = format!("{}token: a", "x".repeat(CONTEXT_MAX_BYTES - 8));
    assert_eq!(expanding.len(), CONTEXT_MAX_BYTES);

    // Fresh bank, plain create: the dropped context must persist as absent.
    let h = Harness::new();
    let bundle = write_bundle(
        &persona_entities(
            vec![stable_row("exported-bank", Some(&expanding))],
            vec![pref_row("exported-bank", Some(&expanding))],
        ),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h.args(&h.ctx(), h.policy(), bundle.path(), false, false, None))
        .expect("plain persona import succeeds");
    assert_eq!(report.would_create, 2);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(
        history[0].context.is_none(),
        "an over-bound scrubbed context must drop to absent on create"
    );
    let stored = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert!(
        stored.context.is_none(),
        "an over-bound scrubbed context must drop to absent on create"
    );

    // Force overwrite over an existing row: nothing unscrubbed or over-bound
    // may persist (the store may carry the prior stored context forward).
    let h2 = Harness::new();
    seed_plain_persona_rows(&h2);
    let bundle = write_bundle(
        &persona_entities(
            vec![stable_row("exported-bank", Some(&expanding))],
            vec![pref_row("exported-bank", Some(&expanding))],
        ),
        ExportContentMode::DsarPlaintext,
    );
    let report = import(&h2.args(
        &h2.ctx(),
        h2.policy(),
        bundle.path(),
        false,
        true,
        Some("ops restore"),
    ))
    .expect("force import succeeds");
    assert_eq!(report.would_overwrite, 2);
    let history = h2
        .store
        .stable_history(&h2.bank, "communication_style")
        .unwrap();
    for entry in &history {
        let ctx = entry.context.as_deref();
        assert_ne!(
            ctx,
            Some(expanding.as_str()),
            "the raw over-bound text persisted"
        );
        assert!(
            ctx.is_none_or(|c| c.len() <= CONTEXT_MAX_BYTES),
            "no stable version may persist an over-bound context"
        );
        assert!(
            !ctx.is_some_and(|c| c.contains("token:")),
            "the unscrubbed over-bound text persisted"
        );
    }
    let stored = h2
        .store
        .get_preference(&h2.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_ne!(stored.context.as_deref(), Some(expanding.as_str()));
    assert!(
        stored
            .context
            .as_deref()
            .is_none_or(|c| c.len() <= CONTEXT_MAX_BYTES)
    );
    assert!(!stored.context.is_some_and(|c| c.contains("token:")));
}

/// The export-side contract (`exported_context`) and the import-side persona
/// staging agree: a bundle row exported with a secret `context` restores with
/// exactly the scrubbed value `scrub_context` produces.
#[test]
fn persona_import_staging_matches_the_export_side_context_contract() {
    let source = store();
    let source_bank = unique("f06-src-bank");
    source
        .put_stable_entry(&stable_row(&source_bank, Some(SECRET)), "tester")
        .unwrap();
    source
        .put_preference_state(&pref_row(&source_bank, Some(SECRET)), "tester")
        .unwrap();
    let dest = dest_path("f06-persona-contract");
    let dest_str = dest.to_string_lossy().into_owned();
    let ctx = OpContext::new(Bank::new(source_bank), Actor::Harness);
    export(&ExportArgs {
        store: &source,
        compliance: &source,
        persona: &source,
        history: &source,
        belief: &source,
        ctx: &ctx,
        destination: &dest_str,
        filter: ExportFilter::default(),
        content_mode: ExportContentMode::DsarPlaintext,
        timestamp: T,
    })
    .expect("export succeeds");

    // The exported bundle carries the scrubbed value, not the raw secret.
    let text = std::fs::read_to_string(&dest).unwrap();
    assert!(!text.contains("sk-test-1234567890"));
    let envelope: BundleEnvelope = serde_json::from_str(&text).unwrap();
    assert_eq!(
        envelope.entities.persona_stable[0].context.as_deref(),
        Some(SECRET_MASKED)
    );
    assert_eq!(
        envelope.entities.persona_preferences[0].context.as_deref(),
        Some(SECRET_MASKED)
    );

    // Import-side staging lands on the same value (plain create path).
    let h = Harness::new();
    let report = import(&h.args(&h.ctx(), h.policy(), &dest_str, false, false, None))
        .expect("import succeeds");
    assert_eq!(report.would_create, 2);
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert_eq!(history[0].context.as_deref(), Some(SECRET_MASKED));
    let stored = h
        .store
        .get_preference(&h.bank, "brevity_affinity")
        .unwrap()
        .expect("preference exists");
    assert_eq!(stored.context.as_deref(), Some(SECRET_MASKED));
    // And the two sides agree by construction, on the shared helper.
    assert_eq!(scrub_context(Some(SECRET)).as_deref(), Some(SECRET_MASKED));
}
