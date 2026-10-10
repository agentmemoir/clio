// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the retention mission schema:
//! validation caps, the specificity lint, rule ids, round-trips, and the
//! builtin coding default.
//!
//! ## Owns
//! - Validation errors for every cap and the specificity lint
//! - Regex-entry validation at set time and id stability
//! - Byte-identical profile/store round-trips and the coding default gate
//!
//! ## Does not own
//! - Pattern grammar internals (see `mission_match_tests`)
//! - Admission arithmetic (see `clio-admission` decision tests)
//!
//! ## Boundary
//!
//! Changes to validation rules or ids should normally require changes here.
//! Changes to storage layout should not.
//!
//! Keep this module focused on mission schema behavior.

use super::*;
use crate::coding_default::{CHATTER_FIXTURE, coding_default_profile};
use crate::error::ErrorCode;
use crate::retention::RetentionProfile;
use crate::retention_mission::mission_rule::mission_rule_id;
use crate::retention_store::RetentionStore;

fn entry(text: &str) -> MissionEntry {
    MissionEntry {
        text: text.to_owned(),
        match_kind: MissionMatchKind::Literal,
        reject: true,
    }
}

fn mission(statement: &str, keep: Vec<MissionEntry>, drop: Vec<MissionEntry>) -> RetentionMission {
    RetentionMission {
        mission_statement: statement.to_owned(),
        keep_examples: keep,
        drop_examples: drop,
    }
}

fn valid_mission() -> RetentionMission {
    mission(
        "Keep durable outcomes; drop routine status chatter.",
        vec![entry("prefers")],
        vec![entry("tests passed")],
    )
}

/// a vague mission (no examples) is rejected with specificity guidance.
#[test]
fn vague_missions_are_rejected_with_guidance() {
    let err = mission("keep things", vec![], vec![])
        .validate()
        .expect_err("no drop");
    assert!(err.message.contains("drop example"), "{}", err.message);

    let all_short = mission(
        "keep things",
        vec![entry("ab")],
        vec![entry("cd"), entry("ef")],
    );
    let err = all_short.validate().expect_err("all short");
    assert!(
        err.message.contains("e.g. drop `tests passed`"),
        "{}",
        err.message
    );

    let all_stopwords = mission(
        "keep things",
        vec![entry("data"), entry("info")],
        vec![entry("status"), entry("misc")],
    );
    assert!(
        all_stopwords.validate().is_err(),
        "all-generic missions rejected"
    );

    // A single specific entry rescues the mission.
    let ok = mission(
        "keep things",
        vec![entry("ab")],
        vec![entry("tests passed")],
    );
    ok.validate().expect("specific entry passes the lint");
}

#[test]
fn caps_and_required_fields_are_enforced() {
    let mut m = valid_mission();
    m.mission_statement.clear();
    assert_eq!(
        m.validate().expect_err("statement").code,
        ErrorCode::InvalidArgument
    );

    m = valid_mission();
    m.mission_statement = "x".repeat(501);
    assert_eq!(
        m.validate().expect_err("statement len").code,
        ErrorCode::OutOfRange
    );

    m = valid_mission();
    m.drop_examples.clear();
    assert!(m.validate().is_err(), "at least one drop example required");

    m = valid_mission();
    m.keep_examples = (0..21).map(|i| entry(&format!("marker-{i}"))).collect();
    assert!(m.validate().is_err(), "21 keep entries rejected");
    m.keep_examples = (0..20).map(|i| entry(&format!("marker-{i}"))).collect();
    m.validate().expect("20 entries allowed");

    m = valid_mission();
    m.drop_examples[0].text = "x".repeat(201);
    assert!(m.validate().is_err(), "entry over 200 chars rejected");

    m = valid_mission();
    m.drop_examples[0].text = String::new();
    assert!(m.validate().is_err(), "empty entries rejected");

    m = valid_mission();
    m.keep_examples.push(entry("tests passed")); // same text+kind as the drop entry
    assert!(m.validate().is_err(), "duplicates across lists rejected");

    m = valid_mission();
    m.keep_examples = (0..20)
        .map(|i| entry(&format!("keep marker {i} ").repeat(10)))
        .collect();
    m.drop_examples = (0..20)
        .map(|i| entry(&format!("drop marker {i} ").repeat(10)))
        .collect();
    assert!(
        m.validate().is_err(),
        "combined text over the total cap rejected"
    );
}

/// regex entries are validated against the pinned subset at set time.
#[test]
fn regex_entries_validated_at_set_time() {
    let mut m = valid_mission();
    m.drop_examples[0] = MissionEntry {
        text: "(tests)+ passed".to_owned(),
        match_kind: MissionMatchKind::Regex,
        reject: true,
    };
    assert!(
        m.validate().is_err(),
        "groups outside the pinned subset rejected"
    );

    m.drop_examples[0] = MissionEntry {
        text: "pr created|issue [0-9]+".to_owned(),
        match_kind: MissionMatchKind::Regex,
        reject: true,
    };
    m.validate().expect("pinned-subset regex accepted");
}

/// Rule ids are content hashes: stable across edits that do not touch the entry,
/// distinct when the entry or its discriminator changes.
#[test]
fn rule_ids_are_stable_content_hashes() {
    let a = entry("tests passed");
    let id = mission_rule_id(&a);
    let edited = RetentionMission {
        mission_statement: "a new statement".to_owned(),
        keep_examples: vec![entry("unrelated marker")],
        drop_examples: vec![a],
    };
    edited.validate().expect("edited mission valid");
    assert_eq!(
        mission_rule_id(&edited.drop_examples[0]),
        id,
        "stable across edits"
    );

    let as_regex = MissionEntry {
        text: "tests passed".to_owned(),
        match_kind: MissionMatchKind::Regex,
        reject: true,
    };
    assert_ne!(
        mission_rule_id(&as_regex),
        id,
        "discriminator is part of the id"
    );
    assert_eq!(id.len(), 16, "hex-encoded 64-bit hash");
}

/// the gap-thread mission stores and round-trips byte-identically
/// through a profile and a retention store save/load.
#[test]
fn mission_round_trips_byte_identical() {
    let profile = RetentionProfile {
        mission: Some(valid_mission()),
        ..RetentionProfile::default()
    };
    let first = serde_json::to_string(&profile).expect("serialize");
    let parsed: RetentionProfile = serde_json::from_str(&first).expect("parse");
    assert_eq!(serde_json::to_string(&parsed).expect("re-serialize"), first);

    let dir = std::env::temp_dir().join(format!("am-mission-rt-{}", std::process::id()));
    let path = dir.join("profiles.json");
    let mut store = RetentionStore::default();
    store.set_bank("bank-a", profile.clone()).expect("set");
    store.save(&path).expect("save");
    let reloaded = RetentionStore::load(&path).expect("load");
    assert_eq!(reloaded.bank_override("bank-a"), Some(&profile));
    std::fs::remove_dir_all(dir).ok();

    // A profile without a mission serializes without a mission key (v1 byte compat).
    let bare = RetentionProfile::default();
    assert!(!serde_json::to_string(&bare).unwrap().contains("mission"));
}

/// The builtin coding default passes its own lint and gates, and every chatter
/// fixture line hits at least one drop entry.
#[test]
fn coding_default_passes_the_lint_and_covers_the_fixture() {
    let profile = coding_default_profile();
    profile.validate().expect("coding default valid");
    assert_eq!(profile.verbosity, crate::retention::Verbosity::Selective);
    assert_eq!(
        profile.duplicate_tolerance_write,
        crate::retention::DuplicateTolerance::Strict
    );
    let mission = profile.mission.as_ref().expect("builtin mission");
    assert!(
        mission
            .drop_examples
            .iter()
            .all(|e| e.reject && e.match_kind == MissionMatchKind::Literal)
    );
    // Every chatter line matches at least one drop entry; every durable line
    // matches none.
    let resolved = ResolvedMission::from_mission(Some(mission));
    for line in &CHATTER_FIXTURE {
        let evaluation = resolved.evaluate(line);
        let hits_drop = evaluation.utility_delta < 0.0;
        if CHATTER_FIXTURE
            .iter()
            .position(|l| l == line)
            .expect("member")
            < 4
        {
            assert!(
                !hits_drop,
                "durable line must not match a drop entry: {line}"
            );
        } else {
            assert!(hits_drop, "chatter line must match a drop entry: {line}");
        }
    }
}

/// An entry whose text carries `key: secret` / `key=secret` material is
/// rejected at set time (Section 7 security constraint), without echoing the
/// raw secret back, and with guidance toward the hygiene tooling.
#[test]
fn secret_bearing_entries_are_rejected_with_hygiene_guidance() {
    let secret = "sk-live-9f8e7d6c5b4a";
    let mut m = valid_mission();
    m.drop_examples[0] = entry(&format!("api_key: {secret}"));
    let err = m.validate().expect_err("secret-bearing entry rejected");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("hygiene"), "{}", err.message);
    assert!(
        !err.message.contains(secret),
        "the error must not echo the secret back: {}",
        err.message
    );

    // The marker word alone (no `:`/`=` value) is pattern text, not a secret.
    let plain = mission(
        "Keep durable outcomes; drop credential dumps.",
        vec![entry("prefers")],
        vec![entry("credential")],
    );
    plain
        .validate()
        .expect("marker word alone is a fine pattern");
}

/// Serde defaults on an entry: `match` falls back to literal and `reject`
/// falls back to true when the JSON omits them.
#[test]
fn entry_serde_defaults_fill_match_and_reject() {
    let e: MissionEntry = serde_json::from_value(serde_json::json!({"text": "tests passed"}))
        .expect("entry with defaults");
    assert_eq!(e.match_kind, MissionMatchKind::Literal);
    assert!(e.reject);
    let r = serde_json::from_value::<MissionEntry>(
        serde_json::json!({"text": "x", "match": "regex", "reject": false}),
    )
    .expect("explicit entry");
    assert_eq!(r.match_kind, MissionMatchKind::Regex);
    assert!(!r.reject);
    let err = serde_json::from_value::<MissionEntry>(serde_json::json!({"text": "x", "nope": 1}))
        .expect_err("unknown field");
    assert!(err.to_string().contains("unknown field"));
}
