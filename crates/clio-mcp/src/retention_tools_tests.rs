// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Retention profile tools and batch admission preview tests.
use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use serde_json::{Value, json};
fn state() -> RetentionStoreFixture {
    RetentionStoreFixture::open(&McpOpenOptions::default())
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

fn store_stats(state: &McpState, bank: &str) -> String {
    let args = json!({ "bank": bank, "timestamp": "2026-10-05T00:00:00Z" });
    let report = crate::additive_tools::dispatch_read(state, "stats", &args)
        .expect("stats is a bound read tool")
        .expect("stats");
    serde_json::to_string(&report).expect("serialize stats")
}

/// A routine status candidate: low utility, so strict profiles reject it.
fn routine_item(id: &str, text: &str) -> Value {
    json!({
        "item": {"id": id, "gist": text},
        "episodic_type": "gist",
        "epistemic_kind": "fact"
    })
}

fn mixed_batch() -> Value {
    let mut items = Vec::new();
    for i in 0..10 {
        let text = if i % 2 == 0 {
            format!("must require decision item {i}")
        } else {
            "ok".to_owned()
        };
        items.push(routine_item(&format!("b{i}"), &text));
    }
    json!({"bank": "bank-a", "actor": "agent", "items": items})
}

#[test]
fn profile_get_defaults_to_builtin_and_hides_inert_fields() {
    let s = state();
    let out = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["source"], json!("builtin"));
    assert_eq!(out["verbosity"], json!("balanced"));
    assert_eq!(out["duplicate_tolerance_write"], json!("balanced"));
    assert_eq!(out["version"], json!(3));
    assert_eq!(
        out["accepted_but_inert"]["honored_by"],
        json!("recall_scope_and_dedup")
    );
    // Stored-but-inactive fields are never presented as active top-level values.
    assert!(out.get("duplicate_tolerance_read").is_none());
    assert!(out.get("recall_scope_default").is_none());
}

#[test]
fn profile_get_and_set_carry_the_retention_knob_fields() {
    let s = state();
    let get = || call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    let defaults = get();
    let knobs = [
        ("retention_curve", json!(true)),
        ("retention_floor", json!(0.25)),
        ("pack_fidelity_default", json!("flow")),
        ("provenance_floor_default", json!(null)),
        ("maintenance_trigger", json!(true)),
    ];
    for (key, want) in knobs {
        assert_eq!(defaults[key], want, "default `{key}`: {defaults}");
    }

    let stored = call(
        &s,
        "retention_profile_set",
        json!({
            "bank": "bank-a",
            "profile": {"retention_curve": false, "retention_floor": 0.5, "pack_fidelity_default": "exact", "provenance_floor_default": "stated", "maintenance_trigger": false},
            "confirm": true
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    let out = get();
    let changed = [
        ("retention_curve", json!(false)),
        ("retention_floor", json!(0.5)),
        ("pack_fidelity_default", json!("exact")),
        ("provenance_floor_default", json!("stated")),
        ("maintenance_trigger", json!(false)),
    ];
    for (key, want) in changed {
        assert_eq!(stored["profile"][key], want, "stored `{key}`: {stored}");
        assert_eq!(out[key], want, "read `{key}`: {out}");
    }
}

#[test]
fn reason_bucket_maps_mission_drop_reasons() {
    assert_eq!(reason_bucket(Some("mission_drop:1a2b")), "mission_drop");
    assert_eq!(reason_bucket(Some("mission_drop")), "mission_drop");
    assert_eq!(reason_bucket(Some("category gate x")), "category_gate");
    assert_eq!(
        reason_bucket(Some("episodic_type gate x")),
        "episodic_type_gate"
    );
    assert_eq!(
        reason_bucket(Some("duplicate tolerance x")),
        "duplicate_tolerance"
    );
    assert_eq!(reason_bucket(Some("score 0.1 below")), "below_threshold");
    assert_eq!(reason_bucket(Some("mystery reason")), "other");
    assert_eq!(reason_bucket(None), "unspecified");
}

#[test]
fn profile_get_reports_mission_presence_and_counts() {
    let s = state();
    let bare = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(bare["mission"], serde_json::Value::Null);

    let stored = call(
        &s,
        "retention_profile_set",
        json!({
            "bank": "bank-a",
            "profile": {
                "verbosity": "selective",
                "mission": {
                    "mission_statement": "Keep build failures and decisions; drop test-run chatter.",
                    "drop_examples": [{"text": "tests passed", "match": "literal", "reject": true}]
                }
            },
            "confirm": true
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");

    let out = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(
        out["mission"]["statement"],
        json!("Keep build failures and decisions; drop test-run chatter.")
    );
    assert_eq!(out["mission"]["keep_examples"], json!(0));
    assert_eq!(out["mission"]["drop_examples"], json!(1));
    assert_eq!(out["mission"]["version"], json!(3));

    // Bank isolation: a bank without a mission reports null.
    let other = call(&s, "retention_profile_get", json!({"bank": "bank-b"}));
    assert_eq!(other["mission"], serde_json::Value::Null);
}

#[test]
fn batch_preview_item_carries_mission_rule_ids_and_labels() {
    let s = state();
    let args = json!({
        "bank": "bank-a",
        "actor": "agent",
        "items": [
            {"item": {"id": "m0", "gist": "Tests passed"}, "episodic_type": "gist", "epistemic_kind": "fact"},
            {"item": {"id": "m1", "gist": "must require decision"}, "episodic_type": "gist", "epistemic_kind": "fact"}
        ],
        "profile_override": {
            "mission": {
                "mission_statement": "Keep decisions; drop test-run chatter.",
                "drop_examples": [{"text": "tests passed", "match": "literal", "reject": true}]
            }
        }
    });
    let out = call(&s, "admit_preview_batch", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);

    // The drop hit rejects outright with a mission_drop reason and the matched ids.
    assert_eq!(items[0]["pass"], json!(false), "{items:?}");
    let reason = items[0]["rejection_reason"].as_str().expect("reason");
    assert!(reason.starts_with("mission_drop:"), "{reason}");
    let ids = items[0]["mission_rule_ids"].as_array().expect("ids");
    assert_eq!(ids.len(), 1, "{items:?}");
    let labels = items[0]["mission_rule_labels"].as_array().expect("labels");
    assert_eq!(labels.len(), ids.len());
    assert!(labels[0].as_str().is_some_and(|l| !l.is_empty()));
    assert_eq!(out["summary"]["by_reason"]["mission_drop"], json!(1));

    // No mission hit on the second item: empty parallel lists, other reasons.
    let second_ids = items[1]["mission_rule_ids"].as_array().expect("ids");
    assert!(second_ids.is_empty(), "{second_ids:?}");
    let second_labels = items[1]["mission_rule_labels"].as_array().expect("labels");
    assert!(second_labels.is_empty(), "{second_labels:?}");
}

#[test]
fn profile_set_requires_confirmation_and_is_bank_scoped() {
    let s = state();
    let unconfirmed = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "selective"}}),
    );
    assert_eq!(unconfirmed["ok"], json!(true));
    assert_eq!(unconfirmed["confirm"], json!(false));
    assert_eq!(unconfirmed["stored"], json!(false));
    let after_preview = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(after_preview["source"], json!("builtin"));

    let stored = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "selective"}, "confirm": true}),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["stored"], json!(true));
    assert_eq!(stored["persisted"], json!(true), "fixture store");

    let a = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(a["source"], json!("bank"));
    assert_eq!(a["verbosity"], json!("selective"));
    // Bank isolation: bank B never sees bank A's override.
    let b = call(&s, "retention_profile_get", json!({"bank": "bank-b"}));
    assert_eq!(b["source"], json!("builtin"));
    assert_eq!(b["verbosity"], json!("balanced"));
}

#[test]
fn profile_set_deployment_scope_applies_to_banks_without_overrides() {
    let s = state();
    let stored = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "selective"}, "scope": "deployment", "confirm": true}),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["scope"], json!("deployment"));
    let b = call(&s, "retention_profile_get", json!({"bank": "bank-b"}));
    assert_eq!(b["source"], json!("deployment"));
    assert_eq!(b["verbosity"], json!("selective"));
}

#[test]
fn profile_set_rejects_permissive_as_a_deployment_default() {
    let s = state();
    let rejected = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "permissive"}, "scope": "deployment", "confirm": true}),
    );
    assert_eq!(rejected["ok"], json!(false), "{rejected}");
    assert_eq!(rejected["code"], json!("invalid_argument"));
    // Banks keep using the previous default; the rejected level never applies.
    let b = call(&s, "retention_profile_get", json!({"bank": "bank-b"}));
    assert_eq!(b["source"], json!("builtin"));
    assert_eq!(b["verbosity"], json!("balanced"));
    // The same level stays legal for a single bank.
    let bank_scoped = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "permissive"}, "confirm": true}),
    );
    assert_eq!(bank_scoped["ok"], json!(true), "{bank_scoped}");
    let a = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(a["source"], json!("bank"));
    assert_eq!(a["verbosity"], json!("permissive"));
}

#[test]
fn profile_set_rejects_invalid_values_and_leaves_the_store_unchanged() {
    let s = state();
    let bad_verbosity = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "verbose"}, "confirm": true}),
    );
    assert_eq!(bad_verbosity["ok"], json!(false));
    assert_eq!(bad_verbosity["code"], json!("invalid_type"));

    let oversize = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"theta_offsets": {"persona": 0.9}}, "confirm": true}),
    );
    assert_eq!(oversize["ok"], json!(false));
    assert_eq!(oversize["code"], json!("out_of_range"));

    let unknown_key = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"theta_offsets": {"not_a_key": 0.1}}, "confirm": true}),
    );
    assert_eq!(unknown_key["ok"], json!(false));
    assert_eq!(unknown_key["code"], json!("unknown_path"));

    let bad_scope = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {}, "scope": "global", "confirm": true}),
    );
    assert_eq!(bad_scope["ok"], json!(false));
    assert_eq!(bad_scope["code"], json!("invalid_argument"));

    let missing = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "confirm": true}),
    );
    assert_eq!(missing["ok"], json!(false));

    let after = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(after["source"], json!("builtin"));
}

#[test]
fn batch_preview_reports_per_item_decisions_and_writes_nothing() {
    let s = state();
    let before = store_stats(&s, "bank-a");
    let out = call(&s, "admit_preview_batch", mixed_batch());
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["read_only"], json!(true));
    assert_eq!(out["writes"], json!(0));
    assert_eq!(out["count"], json!(10));
    assert_eq!(out["profile_version"], json!(3));
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 10);
    // Input order preserved; every item carries a decision and a reason when rejected.
    for (i, item) in items.iter().enumerate() {
        assert!(item["pass"].is_boolean(), "item {i} decision: {item}");
        assert!(item["admission_score"].as_f64().is_some());
        assert!(item["factors"]["novelty"].as_f64().is_some());
        if item["pass"] == json!(false) {
            assert!(item["rejection_reason"].is_string());
        }
    }
    let would_admit = out["summary"]["would_admit"].as_u64().expect("admit");
    let would_reject = out["summary"]["would_reject"].as_u64().expect("reject");
    assert_eq!(would_admit + would_reject, 10);
    assert_eq!(store_stats(&s, "bank-a"), before, "preview must not write");
    assert!(
        s.store
            .get_memory_item("bank-a", "b0")
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn batch_preview_override_affects_only_the_response() {
    let s = state();
    let mut args = mixed_batch();
    args["profile_override"] = json!({"verbosity": "selective"});
    let out = call(&s, "admit_preview_batch", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["profile_source"], json!("override"));
    // The persisted profile is untouched: a later preview uses the builtin default.
    let after = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(after["source"], json!("builtin"));
    assert_eq!(after["verbosity"], json!("balanced"));
}

#[test]
fn batch_preview_is_monotone_across_verbosity_levels() {
    let s = state();
    let count = |profile: &str| {
        let mut args = mixed_batch();
        args["profile_override"] = json!({"verbosity": profile});
        let out = call(&s, "admit_preview_batch", args);
        assert_eq!(out["ok"], json!(true), "{out}");
        out["summary"]["would_admit"].as_u64().expect("admit")
    };
    let selective = count("selective");
    let balanced = count("balanced");
    let permissive = count("permissive");
    assert!(
        selective < balanced,
        "selective {selective} < balanced {balanced}"
    );
    assert!(balanced <= permissive);
}

#[test]
fn batch_preview_oversize_and_empty_batches_are_clean_errors() {
    let s = state();
    let before = store_stats(&s, "bank-a");
    let items: Vec<Value> = (0..=MAX_PREVIEW_BATCH)
        .map(|i| routine_item(&format!("o{i}"), "ok"))
        .collect();
    let out = call(
        &s,
        "admit_preview_batch",
        json!({"bank": "bank-a", "items": items}),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
    assert!(
        out["message"]
            .as_str()
            .expect("msg")
            .contains("preview limit"),
        "{out}"
    );
    assert!(out.get("items").is_none(), "nothing may be scored");

    let empty = call(
        &s,
        "admit_preview_batch",
        json!({"bank": "bank-a", "items": []}),
    );
    assert_eq!(empty["code"], json!("invalid_argument"));

    let missing = call(&s, "admit_preview_batch", json!({"bank": "bank-a"}));
    assert_eq!(missing["code"], json!("invalid_argument"));

    let not_objects = call(
        &s,
        "admit_preview_batch",
        json!({"bank": "bank-a", "items": [1]}),
    );
    assert_eq!(not_objects["code"], json!("invalid_argument"));

    assert_eq!(store_stats(&s, "bank-a"), before);
}

#[test]
fn batch_preview_reports_per_item_errors_without_aborting() {
    let s = state();
    let out = call(
        &s,
        "admit_preview_batch",
        json!({
            "bank": "bank-a",
            "items": [
                routine_item("good", "must require decision"),
                {"item": {"id": "bad"}, "epistemic_kind": "fact"}
            ]
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["pass"], json!(false));
    assert_eq!(items[1]["error"]["code"], json!("invalid_argument"));
    assert_eq!(out["summary"]["by_reason"]["error"], json!(1));
}
