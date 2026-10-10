// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one boundary assertion on the verify
//! telemetry surface: the read-side per-hit `entities[]` projection must not
//! appear in a verify event.
//!
//! ## Owns
//! - The absence of an `entities` key anywhere in a recorded `VerifyEvent`
//! - The absence of the probe entity name everywhere in a recorded
//!   `VerifyEvent` except the pre-existing masked snapshot preview
//!
//! ## Does not own
//! - The verify/refuse event shape and secret masking (`telemetry`)
//! - The read-side entity projection (`clio-retrieve`)
//! - The pre-existing masked-snapshot preview, which carries the snapshot's
//!   own `entity` value; this module excludes it from the name sweep and
//!   changes nothing about it
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the telemetry side of the entity boundary.

use crate::schema::default_event_fields;
use crate::store_path::{VerifiedStoreRequest, gated_store_verified};
use crate::telemetry::VerifyVecSink;
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde_json::{Value, json};

/// A span-verified entity name for the write under test.
const ENTITY: &str = "TelemetryEntityProbe";

#[derive(Debug, Default)]
struct NoSignals;

impl SignalSource for NoSignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }
}

fn candidate_item() -> MemoryItem {
    MemoryItem {
        id: "itm-tel".into(),
        bank_id: "bank-tel".into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": ENTITY, "amount": 1})),
        gist: Some("a schema note".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// The read-side `entities[]` projection never reaches the verify telemetry
/// surface, on either the pass or the fail path.
///
/// Two assertions, each against a real recorded `VerifyEvent`:
///
/// 1. No `entities` key appears anywhere in the rendered event, so the
///    read-side per-hit array cannot hide in any event field.
/// 2. The probe name appears nowhere in the event except inside the
///    pre-existing masked snapshot preview (`preview.snapshot`), which the
///    write path has always carried and which this boundary does not own.
///    Outside that preview the name is swept against the whole rendered
///    event, so a name leaking through `failure_fields`, the gist, the
///    diagnostic, or any future field fails this test.
#[test]
fn verify_telemetry_carries_no_read_side_entity_projection() {
    for (label, entity_in_source) in [("verify_ok", true), ("verify_fail", false)] {
        let ctx = OpContext::new(Bank::new("bank-tel"), Actor::Agent);
        let env = RankingEnv::system_defaults();
        let policy = AdmissionPolicy::new(&env);
        let admit = VecSink::default();
        let verify_sink = VerifyVecSink::default();
        let fields = default_event_fields();
        let source_text = if entity_in_source {
            format!("the {ENTITY} note records amount 1")
        } else {
            "a source with no matching span at all".to_owned()
        };
        let mut req = VerifiedStoreRequest {
            item: candidate_item(),
            ctx: &ctx,
            source_text: Some(source_text.as_str()),
            evidence_ref: None,
            fields: &fields,
            verify: VerifyConfig::default(),
            policy,
            admit_sink: &admit,
            verify_sink: &verify_sink,
            timestamp: "2026-01-01T00:00:00Z",
        };
        // The write closure is irrelevant here: the verify event is emitted
        // before the admission gate, and only that event is under test.
        let _ = gated_store_verified(
            &mut req,
            Candidate::Semantic(SemanticCategory::Schema),
            &NoSignals,
            &mut |_| Ok(()),
        );

        let events = verify_sink.events();
        assert_eq!(events.len(), 1, "{label} emitted one verify event");
        assert_eq!(events[0].operation, label, "{label} event kind");
        let rendered = serde_json::to_string(&events[0]).expect("serialize event");
        assert!(
            !rendered.contains("\"entities\""),
            "{label} telemetry has no read-side entities array: {rendered}"
        );
        let mut value = serde_json::to_value(&events[0]).expect("event value");
        if let Some(preview) = value.get_mut("preview") {
            preview["snapshot"] = Value::Null;
        }
        let outside_preview = serde_json::to_string(&value).expect("render event");
        assert!(
            !outside_preview.contains(ENTITY),
            "{label} telemetry carries the entity name outside the pre-existing \
             snapshot preview: {outside_preview}"
        );
    }
}
