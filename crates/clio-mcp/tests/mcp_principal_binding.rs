// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Credential-bound HTTP conformance: what a credential decides, and what a
//! missing one may not reach.
//!
//! The actor a request runs as, and the banks it may reach, come from the record
//! its secret names. A conflict is refused rather than resolved in the caller's
//! favour, and a refusal writes nothing anywhere.
//!
//! Run locally with `cargo test -p clio-mcp --test mcp_principal_binding`.
//!
//! The harness lives in `mcp_principal_binding_support`.

mod mcp_principal_binding_support;

use clio_mcp::http::HttpConfig;
use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_types::{Actor, ErrorCode};
use mcp_principal_binding_support::{
    Store, assert_no_secret_material, bearer, bound_server, bound_server_with_shared, call,
    erase_at, export_at, forbidden, post, spawn_server, stored, tool_error_code, tool_ok,
};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn a_harness_credential_may_erase_and_export() {
    // The compliance verbs authorize on the actor the credential names, so a
    // scoped harness record reaches both.
    let store = Store::new("harness");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "not-listed");

    for body in [export_at("demo"), erase_at("demo")] {
        let (status, response) = post(port, &body, &bearer(&secrets[0]));
        assert_eq!(status, 200, "{response}");
        assert!(!forbidden(&response), "must be allowed: {response}");
    }
}

#[test]
fn an_agent_credential_may_not_erase_or_export() {
    // Same verbs, same bank, different record: the actor is what differs.
    let store = Store::new("agent");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("bot", Actor::Agent, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "not-listed");

    for body in [export_at("demo"), erase_at("demo")] {
        let (status, response) = post(port, &body, &bearer(&secrets[0]));
        assert_eq!(status, 200, "{response}");
        assert!(forbidden(&response), "must be refused: {response}");
    }
}

#[test]
fn an_explicit_bank_outside_the_list_is_refused_and_writes_nothing() {
    let store = Store::new("bank-escape");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (status, response) = post(port, &store_item_at("someone-elses", "escape-1"), &auth);
    assert_eq!(status, 200, "{response}");
    assert!(forbidden(&response), "{response}");

    // Absent from the bank that was asked for and from the one that was listed:
    // a refusal must not have written anywhere.
    assert!(!stored(port, "someone-elses", "escape-1", &auth));
    assert!(!stored(port, "demo", "escape-1", &auth));
}

#[test]
fn an_omitted_bank_resolving_outside_the_list_is_refused() {
    // The state default is `not-listed`, and omitting the bank does not exempt
    // the call from the credential's list.
    let store = Store::new("default-outside");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "not-listed");

    let (_, response) = post(
        port,
        &store_item_at("not-listed", "default-escape"),
        &bearer(&secrets[0]),
    );
    assert!(forbidden(&response), "{response}");
}

#[test]
fn an_omitted_bank_resolving_inside_the_list_is_allowed() {
    // Same code path, a default the record names.
    let store = Store::new("default-inside");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (_, response) = post(port, &store_item_at("demo", "allowed-1"), &auth);
    assert!(tool_ok(&response), "{response}");
    assert!(stored(port, "demo", "allowed-1", &auth));
}

#[test]
fn a_declared_actor_differing_from_the_bound_one_is_refused() {
    // The caller believes it acted as something else; accepting would put that
    // belief into the audit rows.
    let store = Store::new("actor-conflict");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    for declared in ["agent", "user", "system", "not-an-actor"] {
        let body = store_item_declaring("demo", "spoof-1", declared);
        let (status, response) = post(port, &body, &auth);
        assert_eq!(status, 200, "{declared}: {response}");
        assert!(
            forbidden(&response),
            "{declared} must be refused: {response}"
        );
    }
    assert!(
        !stored(port, "demo", "spoof-1", &auth),
        "a refused write landed"
    );
}

#[test]
fn a_declared_actor_equal_to_the_bound_one_is_accepted() {
    let store = Store::new("actor-equal");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (_, response) = post(
        port,
        &store_item_declaring("demo", "declared-1", "harness"),
        &auth,
    );
    assert!(tool_ok(&response), "an equal actor is accepted: {response}");
    assert!(stored(port, "demo", "declared-1", &auth));
}

#[test]
fn an_absent_actor_uses_the_bound_one() {
    // Observable through behavior rather than through a stored field: a harness
    // credential with no declared actor reaches the compliance verbs, which
    // authorize only on `harness`/`system`. Had the state default won instead,
    // the actor would be `agent` and the call would be refused — which the
    // agent-credential case above asserts.
    let store = Store::new("actor-absent");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);

    let port = bound_server(&credentials, "demo");
    let (_, response) = post(port, &export_at("demo"), &bearer(&secrets[0]));
    assert!(
        !forbidden(&response),
        "an absent actor must be the bound one: {response}"
    );
}

#[test]
fn the_shared_gate_still_applies_on_top_of_the_credential_list() {
    // A credential listing `shared` does not switch the deployment gate on:
    // `shared_bank_enabled` is left off, as a default deployment leaves it.
    let store = Store::new("shared-off");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo", "shared"])], &mut secrets);
    let port = bound_server(&credentials, "demo");

    let body = call(
        "shared_store",
        json!({"key": "k", "value": "v", "bank": "shared"}),
    );
    let (_, response) = post(port, &body, &bearer(&secrets[0]));
    assert!(
        !tool_ok(&response),
        "the deployment gate must still refuse `shared`: {response}"
    );
}

#[test]
fn a_listed_shared_bank_is_allowed_when_the_deployment_enabled_it() {
    // The other half of the case above. The shared gate is two conditions, not
    // one: a deployment permission and a credential whose list names the bank.
    // Satisfying both must let the write land, so the refusal test above is not
    // taken as proof that the gate merely refuses everything.
    let store = Store::new("shared-on");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo", "shared"])], &mut secrets);
    let port = bound_server_with_shared(&credentials, "demo", true);
    let auth = bearer(&secrets[0]);

    let body = call(
        "shared_store",
        json!({
            "item": {"id": "shared-1", "gist": "an enabled shared convention"},
            "category": "tool_config", "epistemic_kind": "fact",
            "actor": "harness"
        }),
    );
    let (status, response) = post(port, &body, &auth);
    assert_eq!(status, 200, "{response}");
    assert!(
        tool_ok(&response),
        "both conditions hold, so the write must land: {response}"
    );
    assert!(stored(port, "shared", "shared-1", &auth), "{response}");
}

#[test]
fn the_unbound_loopback_default_still_honors_a_declared_actor_and_bank() {
    // No credential source at all: the local contract is unchanged, which is
    // what keeps a developer's loopback server working.
    let port = spawn_server(HttpConfig::default(), "demo");
    let (_, response) = post(port, &store_item_declaring("demo", "unbound-1", "user"), "");
    assert!(tool_ok(&response), "{response}");

    // Even a bank no credential could name is reachable here, because there is
    // no credential. That is the documented local contract, not a leak.
    let (_, response) = post(port, &store_item_at("anywhere-at-all", "unbound-2"), "");
    assert!(tool_ok(&response), "{response}");
    assert!(stored(port, "anywhere-at-all", "unbound-2", ""));
}

#[test]
fn stdio_and_the_in_process_dispatcher_keep_the_unbound_contract() {
    // The bound rule is reached only from the HTTP edge; the other two callers of
    // the handler are unchanged.
    let state = McpState::open(&McpOpenOptions {
        default_bank: "demo".into(),
        ..McpOpenOptions::default()
    })
    .expect("state");
    let handler = McpHandler::new(Arc::clone(&state));
    let frame = store_item_declaring("anywhere-at-all", "stdio-1", "harness");
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(
        &handler,
        std::io::Cursor::new(format!("{frame}\n").into_bytes()),
        &mut out,
    )
    .expect("stdio serve");
    let first = out.split(|b| *b == b'\n').next().expect("a line");
    let response: Value = serde_json::from_slice(first).expect("json");
    assert_eq!(
        response["result"]["structuredContent"]["ok"],
        json!(true),
        "{response}"
    );
}

#[test]
fn a_missing_or_unknown_secret_is_401_and_never_returns_a_tool_result() {
    let store = Store::new("unauthorized");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let body = store_item_at("demo", "x");

    let unknown = clio_config::auth_tokens::generate_secret();
    for (extra, why) in [
        (String::new(), "no credential"),
        (bearer(&unknown), "unknown secret"),
        ("Authorization: Basic abc\r\n".to_owned(), "wrong scheme"),
        ("Authorization: Bearer\r\n".to_owned(), "empty bearer"),
        (
            format!("?token={unknown}\r\n"),
            "secret in the query string",
        ),
    ] {
        let (status, response) = post(port, &body, &extra);
        assert_eq!(status, 401, "{why} must be 401, got {response}");
        assert!(
            !response.contains("structuredContent"),
            "{why} returned a tool result: {response}"
        );
    }
    // The legitimate credential still works, so the refusals are not a server
    // that simply stopped dispatching.
    assert_eq!(post(port, &body, &bearer(&secrets[0])).0, 200);
}

#[test]
fn a_refusal_code_is_forbidden_rather_than_a_generic_failure() {
    // The MCP specification's own split: 401 for a credential problem, and a
    // refusal inside the tool result for a permission one.
    let store = Store::new("code");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");

    let body = store_item_at("not-listed", "a");
    let (status, response) = post(port, &body, &bearer(&secrets[0]));
    assert_eq!(status, 200, "the frame was served: {response}");
    assert_eq!(
        tool_error_code(&response).as_deref(),
        Some(ErrorCode::Forbidden.as_str()),
        "{response}"
    );
}

#[test]
fn two_store_backed_servers_answer_an_unknown_secret_identically() {
    // The 401 body is a property of the transport, not of one deployment's
    // store: two independently configured servers answer the same refused
    // request with the same status and body, so a caller learns nothing about
    // which deployment it reached.
    let first = Store::new("shape-a");
    let mut secrets = Vec::new();
    let credentials = first.write(&[("ci", Actor::Harness, &["demo"])], &mut secrets);
    let first_port = bound_server(&credentials, "demo");
    let second = Store::new("shape-b");
    let mut second_secrets = Vec::new();
    let second_credentials =
        second.write(&[("ci", Actor::Harness, &["demo"])], &mut second_secrets);
    let second_port = bound_server(&second_credentials, "demo");

    let body = store_item_at("demo", "a");
    let unknown = clio_config::auth_tokens::generate_secret();
    let (first_status, first_body) = post(first_port, &body, &bearer(&unknown));
    let (second_status, second_body) = post(second_port, &body, &bearer(&unknown));
    assert_eq!(first_status, second_status);
    assert_eq!(
        first_body, second_body,
        "the two 401 bodies must be identical"
    );
}

#[test]
fn an_agent_record_keeps_reads_and_writes_and_refuses_the_compliance_verbs() {
    // The compliance verbs authorize on the actor the record names, so an
    // agent-named record keeps reads and writes at a listed bank and fails
    // closed on the verbs that need `harness`/`system`.
    let store = Store::new("agent-verbs");
    let mut secrets = Vec::new();
    let credentials = store.write(&[("bot", Actor::Agent, &["demo"])], &mut secrets);
    let port = bound_server(&credentials, "demo");
    let auth = bearer(&secrets[0]);

    let (status, response) = post(port, &store_item_at("demo", "agent-1"), &auth);
    assert_eq!(status, 200, "{response}");
    assert!(tool_ok(&response), "agent write must work: {response}");
    assert!(
        stored(port, "demo", "agent-1", &auth),
        "agent read must work"
    );

    for body in [export_at("demo"), erase_at("demo")] {
        let (status, response) = post(port, &body, &auth);
        assert_eq!(status, 200, "{response}");
        assert!(
            forbidden(&response),
            "a compliance verb through an agent record must fail closed: {response}"
        );
    }
}

#[test]
fn no_token_material_appears_in_any_refusal_or_response() {
    // Asserted over every byte a caller can see, across credential classes.
    let store = Store::new("no-leak");
    let mut secrets = Vec::new();
    let credentials = store.write(
        &[
            ("ci", Actor::Harness, &["demo"]),
            ("bot", Actor::Agent, &["scratch"]),
        ],
        &mut secrets,
    );
    let digests = store.digests();
    let unknown = clio_config::auth_tokens::generate_secret();
    let port = bound_server(&credentials, "demo");
    let everything = [secrets.clone(), vec![unknown.clone()]].concat();

    let probes = [
        store_item_at("not-listed", "a"),
        store_item_declaring("demo", "a", "system"),
        export_at("demo"),
    ];
    let headers = [
        bearer(&secrets[0]),
        bearer(&secrets[1]),
        bearer(&unknown),
        String::new(),
    ];
    for body in probes {
        for extra in &headers {
            let (_, response) = post(port, &body, extra);
            assert_no_secret_material(&response, &everything, &digests);
        }
    }
}

/// A `store` of `id` at `bank` with no declared actor.
fn store_item_at(bank: &str, id: &str) -> String {
    call(
        "store",
        json!({
            "bank": bank,
            "item": {"id": id, "gist": "a write that may or may not land"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    )
}

/// A `store` of `id` at `bank` declaring `actor`.
fn store_item_declaring(bank: &str, id: &str, actor: &str) -> String {
    call(
        "store",
        json!({
            "bank": bank, "actor": actor,
            "item": {"id": id, "gist": "a write that may or may not land"},
            "category": "task_spec", "epistemic_kind": "fact"
        }),
    )
}
