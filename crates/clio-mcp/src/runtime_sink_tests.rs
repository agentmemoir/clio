// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for stderr sink serialization tests: every
//! emitted event type from the admission, verify, mutator, and domain sink
//! traits serializes without error.
//!
//! ## Owns
//! - `EventSink`, `VerifySink`, `TripleSink`, `ContinuousSink`, `DiscardSink`,
//!   `HistorySink`, `PersonaSink`, and `BeliefSink` emit coverage over
//!   `StderrSinks`
//!
//! ## Does not own
//! - The sink implementations (`runtime.rs`)
//! - Open and state wiring coverage (`runtime_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the stderr sink set or an event shape changes; they
//! never change it.
//!
//! Keep this module focused on stderr sink serialization.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_admission::EventSink;
use clio_belief::observe::BeliefSink;
use clio_history::HistorySink;
use clio_persona::PersonaSink;
use clio_write::DiscardSink;
use clio_write::telemetry::VerifySink;
use clio_write::triple::TripleSink;
use clio_write::update::ContinuousSink;

fn sinks() -> StderrSinks {
    StderrSinks
}

#[test]
fn stderr_sinks_serialize_admission_and_verify_events() {
    EventSink::emit(
        &sinks(),
        clio_admission::AdmissionEvent {
            operation: "admit_reject".into(),
            candidate_key: "k".into(),
            candidate_kind: "semantic".into(),
            admission_score: Some(0.1),
            rejection_reason: Some("test".into()),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            preview: serde_json::Value::Null,
        },
    );
    VerifySink::emit(
        &sinks(),
        clio_write::telemetry::VerifyEvent {
            operation: "verify_ok".into(),
            bank: "b".into(),
            actor: "agent".into(),
            principal: None,
            source_ref: "s".into(),
            timestamp: "t".into(),
            failure_fields: vec![],
            preview: serde_json::Value::Null,
        },
    );
}

#[test]
fn stderr_sinks_serialize_mutator_events() {
    TripleSink::emit(
        &sinks(),
        clio_write::triple::TripleEvent {
            operation: "triple_end".into(),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            edge_or_item_id: "e".into(),
            prior_ids: vec![],
            replacement_id: None,
        },
    );
    ContinuousSink::emit(
        &sinks(),
        clio_write::update::ContinuousEvent {
            operation: "update_continuous".into(),
            bank: "b".into(),
            actor: "agent".into(),
            attr_key: "k".into(),
            timestamp: "t".into(),
            state: 1.0,
            trend: 0.0,
        },
    );
    DiscardSink::emit(
        &sinks(),
        clio_write::DiscardEvent {
            operation: "discard".into(),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            item_id: "i".into(),
            category: None,
            epistemic_kind: "fact".into(),
            reason_len: 3,
        },
    );
}

#[test]
fn stderr_sinks_serialize_domain_events() {
    HistorySink::emit(
        &sinks(),
        clio_history::HistoryEvent {
            operation: "task_upsert".into(),
            outcome: "stored".into(),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            task_id: "t".into(),
            record_id: "r".into(),
            record_count: 1,
            admission_score: Some(0.5),
            rejection_reason: None,
        },
    );
    PersonaSink::emit(
        &sinks(),
        clio_persona::PersonaEvent {
            operation: "persona_put_stable".into(),
            path: "discrete".into(),
            outcome: "admitted".into(),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            key: "k".into(),
            state: None,
            trend: None,
            admission_score: Some(0.5),
            rejection_reason: None,
        },
    );
    BeliefSink::emit(
        &sinks(),
        clio_belief::observe::BeliefEvent {
            operation: "belief_observe".into(),
            outcome: "created".into(),
            bank: "b".into(),
            actor: "agent".into(),
            timestamp: "t".into(),
            belief_id: "bel".into(),
            proposition_key: "k".into(),
            entry_count: 1,
            confidence: 0.5,
            source_type: "user_stated".into(),
            admission_score: Some(0.5),
            rejection_reason: None,
        },
    );
}
