// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins what a verify event records about the credential that
//! presented the operation: a bound call names it, a local call names nobody,
//! and a local caller cannot fake one through its arguments.
//!
//! ## Owns
//! - Test coverage for the credential label on a gated-store verify event
//! - The rule that an unbound event carries no such key at all
//!
//! ## Does not own
//! - Admission, refusal and preview behaviour (`store_path_tests`)
//! - The event shape (`telemetry.rs`) and the emitter (`store_path.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the verified store's attribution changes; they never
//! change it.
//!
//! Keep this module focused on the credential label on a verify event.

use super::store_path_test_support::{EmptyNeighbors, bound_ctx, ctx, item_with_snapshot};
use crate::schema::default_event_fields;
use crate::store_path::{VerifiedStoreRequest, gated_store_verified};
use crate::telemetry::{VerifyEvent, VerifyVecSink};
use crate::verify::VerifyConfig;
use clio_admission::decision::Candidate;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{Actor, Bank, OpContext, PrincipalLabel};
use serde_json::json;

/// The one `verify_ok` event, or a panic naming what was recorded instead.
fn verify_ok_event(sink: &VerifyVecSink) -> VerifyEvent {
    sink.events()
        .into_iter()
        .find(|e| e.operation == "verify_ok")
        .expect("a verify_ok event")
}

/// Run one admitted store through the gated path with the given context.
fn store_with(ctx: &OpContext, verify_sink: &VerifyVecSink) {
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let fields = default_event_fields();
    let mut req = VerifiedStoreRequest {
        item: item_with_snapshot("i-p", json!({"entity": "Ada"})),
        ctx,
        source_text: Some("Ada likes concise style"),
        evidence_ref: Some("turn_2"),
        fields: &fields,
        verify: VerifyConfig::default(),
        policy: AdmissionPolicy::new(&env),
        admit_sink: &admit,
        verify_sink,
        timestamp: "t0",
    };
    gated_store_verified(
        &mut req,
        Candidate::Semantic(clio_types::SemanticCategory::Persona),
        &EmptyNeighbors,
        &mut |_| Ok(()),
    )
    .expect("admit");
}

#[test]
fn a_bound_store_names_the_credential_on_its_verify_event() {
    let verify_sink = VerifyVecSink::default();
    store_with(&bound_ctx("ops-key"), &verify_sink);
    let event = verify_ok_event(&verify_sink);
    assert_eq!(event.principal.as_deref(), Some("ops-key"));
    assert_eq!(event.actor, "agent", "the claimed role is unchanged");
    let json = serde_json::to_value(&event).expect("serialize event");
    assert!(
        !json.to_string().contains("digest") && !json.to_string().contains("sha256"),
        "the label is recorded verbatim and derives nothing: {json}"
    );
}

#[test]
fn a_local_store_emits_a_verify_event_with_no_credential() {
    let verify_sink = VerifyVecSink::default();
    store_with(&ctx(), &verify_sink);
    let event = verify_ok_event(&verify_sink);
    assert_eq!(
        event.principal, None,
        "an unbound event carries no empty or null label"
    );
    let json = serde_json::to_value(&event).expect("serialize event");
    assert!(json.get("principal").is_none(), "{json}");
}

#[test]
fn the_context_field_and_the_accessor_report_the_same_label() {
    let ctx = OpContext::with_principal(
        Bank::new("bank-1"),
        Actor::Harness,
        PrincipalLabel::new("ops-key"),
    );
    assert_eq!(ctx.principal().map(PrincipalLabel::as_str), Some("ops-key"));
    assert_eq!(ctx.principal, Some(PrincipalLabel::new("ops-key")));
    let verify_sink = VerifyVecSink::default();
    store_with(&ctx, &verify_sink);
    assert_eq!(
        verify_ok_event(&verify_sink).principal.as_deref(),
        Some("ops-key")
    );
}
