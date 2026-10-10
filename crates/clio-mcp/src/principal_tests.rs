// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Principal authorization tests: the actor-conflict rule, the bank-list rule,
//! the omitted-bank case, and the record identity a session binds to.
//! Every case here is a refusal that must happen before any tool body runs,
//! which is why they are unit tests over the rule rather than over a dispatched
//! tool.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use serde_json::json;

fn harness_credential() -> Principal {
    Principal::new("ci".into(), Actor::Harness, vec!["demo".into()])
}

fn agent_credential() -> Principal {
    Principal::new("bot".into(), Actor::Agent, vec!["demo".into()])
}

#[test]
fn a_harness_credential_passes_the_compliance_gate_and_an_agent_one_does_not() {
    // The actor the compliance verbs authorize on is what the credential names,
    // not what the caller declares. That is the whole point of binding.
    let harness = harness_credential();
    let agent = agent_credential();
    assert_eq!(harness.actor, Actor::Harness);
    assert_eq!(agent.actor, Actor::Agent);
    // The unchanged gate in the compliance crate keys on exactly these values.
    assert!(matches!(harness.actor, Actor::Harness | Actor::System));
    assert!(!matches!(agent.actor, Actor::Harness | Actor::System));
}

#[test]
fn an_absent_actor_uses_the_bound_one() {
    let principal = harness_credential();
    let args = json!({"bank": "demo"});
    assert_eq!(
        principal.authorize(&args, "demo").expect("ok"),
        Actor::Harness
    );
}

#[test]
fn an_equal_actor_is_accepted() {
    let principal = harness_credential();
    let args = json!({"actor": "harness", "bank": "demo"});
    assert_eq!(
        principal.authorize(&args, "demo").expect("ok"),
        Actor::Harness
    );
}

#[test]
fn a_differing_actor_is_forbidden() {
    // The caller believes it acted as something else; accepting would put that
    // belief into the audit rows.
    let principal = harness_credential();
    let args = json!({"actor": "agent", "bank": "demo"});
    let err = principal.authorize(&args, "demo").expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("harness"), "{}", err.message);
}

#[test]
fn an_actor_outside_the_four_valid_names_is_also_forbidden() {
    // The refusal must not become a way to enumerate the accepted names: a
    // value that is not an actor at all is refused exactly as a valid but
    // wrong one is.
    let principal = harness_credential();
    let err = principal
        .authorize(&json!({"actor": "superuser"}), "demo")
        .expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn an_explicit_bank_inside_the_list_is_allowed() {
    let principal = Principal::new(
        "ci".into(),
        Actor::Harness,
        vec!["demo".into(), "other".into()],
    );
    assert!(principal.authorize_bank("other").is_ok());
}

#[test]
fn an_explicit_bank_outside_the_list_is_forbidden() {
    let principal = harness_credential();
    let args = json!({"bank": "someone-elses-bank"});
    let err = principal
        .authorize(&args, "someone-elses-bank")
        .expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        err.message.contains("someone-elses-bank"),
        "{}",
        err.message
    );
}

#[test]
fn an_out_of_list_bank_never_falls_back_to_a_listed_one() {
    // The refusal names the bank asked for and does not offer an alternative.
    // A fallback would write where the caller did not ask, which is worse than
    // a refusal because the caller would see success.
    let principal = harness_credential();
    let err = principal.authorize_bank("nope").expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(!err.message.contains("demo"), "{}", err.message);
}

#[test]
fn an_omitted_bank_resolving_inside_the_list_is_allowed() {
    let principal = Principal::new("ci".into(), Actor::Harness, vec!["demo".into()]);
    // The caller omitted the bank, so the state default resolved to `demo`.
    assert!(principal.authorize(&json!({}), "demo").is_ok());
}

#[test]
fn an_omitted_bank_resolving_outside_the_list_is_forbidden() {
    // Same omission, different default: the default does not exempt the call.
    let principal = Principal::new("ci".into(), Actor::Harness, vec!["demo".into()]);
    let err = principal
        .authorize(&json!({}), "default")
        .expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn the_shared_gate_is_the_credential_list_not_a_replacement_for_it() {
    // Two independent conditions. A credential may list `shared` and the
    // deployment may still refuse it; that gate lives in the resolver and runs
    // on top, so this layer only ever sees the list.
    let principal = Principal::new("ci".into(), Actor::Harness, vec!["shared".into()]);
    assert!(
        principal.authorize_bank("shared").is_ok(),
        "a listed bank passes this layer; the deployment gate is separate"
    );
    let without = harness_credential();
    assert!(
        without.authorize_bank("shared").is_err(),
        "an unlisted bank is refused here regardless of the deployment gate"
    );
}

#[test]
fn two_records_with_different_labels_have_different_identities() {
    let first = Principal::new("ci".into(), Actor::Harness, vec!["demo".into()]);
    let second = Principal::new("bot".into(), Actor::Agent, vec!["demo".into()]);
    assert_ne!(
        first.session_identity(),
        second.session_identity(),
        "two records are two principals"
    );
}

/// The identity is the label itself, and the store refuses a repeated label, so
/// equal labels mean the same credential and must compare equal. Uniqueness is
/// the store's own guarantee; this pins what the identity does with it.
#[test]
fn two_records_with_one_label_share_an_identity() {
    let first = Principal::new("ci".into(), Actor::Harness, vec!["demo".into()]);
    let second = Principal::new("ci".into(), Actor::Harness, vec!["other".into()]);
    assert_eq!(first.session_identity(), second.session_identity());
}
