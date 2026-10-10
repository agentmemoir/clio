// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! Telemetry and preview-masking tests for the gated write path.
//!
//! ## Owns
//! - Reject-event secret masking, scrub edge cases, gist truncation, confidence lift
//!
//! ## Does not own
//! - Core gate/write decisions (see `gated_tests`)
//!
//! ## Boundary
//!
//! Changes to masked preview / scrub behavior should normally require changes here.
//! Changes to scoring thresholds should not require changes here.
//!
//! Keep this module focused on admission event telemetry tests.

use super::gated_test_support::{ctx, gate_req, gist_item, weak_source};
use super::{VecSink, gated_create};
use crate::decision::Candidate;
use crate::policy::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_types::{EpisodicType, EpistemicKind, SourceType};
use serde_json::json;

#[test]
fn reject_events_mask_secrets() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let mut item = gist_item("sec-1", "note with token=abc123 inside");
    item.snapshot = Some(json!({"api_key": "sk-SECRETKEY99"}));
    let source = weak_source();
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &source,
        &mut |_| panic!("reject must never write"),
    )
    .expect("reject");
    assert!(!decision.pass);
    let events = sink.events();
    assert_eq!(events.len(), 1);
    let preview = events[0].preview.to_string();
    assert!(!preview.contains("SECRETKEY99"));
    assert!(!preview.contains("abc123"));
    assert!(preview.contains("****"));
}

#[test]
fn scrub_never_panics_and_masks_secrets() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let source = weak_source();
    // (gist, must_contain, must_absent); every case must reject (gist prior).
    for (gist, must_contain, must_absent) in [
        ("the token expired yesterday", "token expired", ""),
        ("trailing marker token=", "token=", ""),
        ("see token=zzz", "[REDACTED]", "token=zzz"),
        ("spaced secret= abc", "[REDACTED]", "secret= abc"),
        ("tabbed secret=\tabc", "[REDACTED]", "secret=\tabc"),
        (
            "caf\u{e9} token=caf\u{e9} latte",
            "[REDACTED]",
            "token=caf\u{e9}",
        ),
        (
            "token=\u{4e2d}\u{6587}\u{503c}",
            "[REDACTED]",
            "token=\u{4e2d}",
        ),
        (
            "emoji password=\u{1F511}key",
            "[REDACTED]",
            "password=\u{1F511}key",
        ),
        ("UPPER TOKEN=abc", "[REDACTED]", "TOKEN=abc"),
        ("a=b token=x c=d", "[REDACTED]", "token=x"),
        ("d\u{e9}j\u{e0} vu token=zl\u{00fc}t", "[REDACTED]", ""),
    ] {
        let item = gist_item("scrub-edge", gist);
        let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
        let decision = gated_create(
            &req,
            Candidate::Episodic(EpisodicType::Gist),
            &source,
            &mut |_| panic!("reject must never write"),
        )
        .expect("reject");
        assert!(!decision.pass, "gist: {gist}");
        let preview = sink.events().last().expect("event").preview.to_string();
        assert!(preview.contains(must_contain), "gist: {gist}");
        if !must_absent.is_empty() {
            assert!(!preview.contains(must_absent), "gist: {gist}");
        }
    }
}

#[test]
fn long_gist_truncates_in_preview() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let sink = VecSink::default();
    let ctx = ctx();
    let item = gist_item("long-1", &"a".repeat(200));
    let source = weak_source();
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &source,
        &mut |_| panic!("reject must never write"),
    )
    .expect("reject");
    assert!(!decision.pass);
    let preview = sink.events().last().expect("event").preview.to_string();
    assert!(preview.contains('…'));
}

#[test]
fn user_stated_source_lifts_confidence() {
    let env = RankingEnv::system_defaults();
    // `weak_source` mirrors the item, so novelty is 0 and the item is an exact
    // duplicate of its neighbor. Force waives that one rejection so this test
    // keeps measuring what it is about: the confidence factor. The novelty
    // arithmetic below is unchanged.
    let policy = AdmissionPolicy::new(&env).with_force(true);
    let sink = VecSink::default();
    let ctx = ctx();
    let mut item = gist_item("conf-1", "plain note without markers");
    item.epistemic_kind = EpistemicKind::Belief;
    item.source_type = Some(SourceType::UserStated);
    // Utility 0.5, confidence 0.9, novelty 0, recency 1.0, gist prior 0.4:
    // 0.2 * 2.8 = 0.56 admits; the same unstated text scores 0.48 and rejects.
    let source = weak_source();
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &source,
        &mut |stored| {
            assert!(stored.admission_score.is_some());
            Ok(())
        },
    )
    .expect("admit");
    assert!(decision.pass);

    item.source_type = None;
    item.epistemic_kind = EpistemicKind::Fact;
    let req = gate_req(&item, &ctx, policy.clone(), &sink, "2026-09-17T00:00:00Z");
    let decision = gated_create(
        &req,
        Candidate::Episodic(EpisodicType::Gist),
        &source,
        &mut |_| panic!("unstated must reject"),
    )
    .expect("reject");
    assert!(!decision.pass);
}
