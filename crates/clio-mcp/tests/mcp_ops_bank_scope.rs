// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration test drives the operator tools over the real HTTP MCP
//! transport: a credential bound to one bank must not reach another bank's
//! `reindex`, `repair`, or `diagnose`, whether the caller names that bank or
//! omits the argument.
//!
//! ## Owns
//! - The one bound server this target drives: a restricted operator principal
//!   and a seeding principal that plants the foreign bank's rows through the
//!   same transport
//! - The refused explicit bank, the granted omitted bank, the granted plan
//!   round-trip, the scoped report, and the 401 without any credential
//!
//! ## Does not own
//! - The harness itself (`mcp_principal_binding_support`), shared with the
//!   sibling credential-binding target
//! - Operator semantics (`clio-ops`) or the in-process grant checks
//!   (`clio-mcp`'s `ops_tools_grant_tests`)
//!
//! ## Boundary
//!
//! Changes to what an operator tool answers a bound credential over HTTP should
//! normally require changes here. Changes to operator semantics should not.
//!
//! Keep this module focused on the bank grant as the transport enforces it.
//!
//! Run locally with `cargo nextest run -p clio-mcp --test mcp_ops_bank_scope`.
//!
//! The harness lives in `mcp_principal_binding_support`.

// The harness is shared with the sibling credential-binding target and is
// compiled into this one whole, so this target uses only part of it.
#[allow(dead_code)]
mod mcp_principal_binding_support;

use clio_types::Actor;
use mcp_principal_binding_support::{
    Store, assert_no_secret_material, bearer, bound_server, call, forbidden, post, tool_ok,
};
use serde_json::{Value, json};

/// One bound server serving two principals over one in-memory store.
struct Fixture {
    port: u16,
    /// Authorization header of the credential bound to `bank-a` alone.
    ops: String,
    /// Authorization header of the credential that may reach both banks; it
    /// seeds the foreign bank a grant test needs to exist.
    seed: String,
    /// Every secret the credential store holds, for the leak assertion.
    secrets: Vec<String>,
    store: Store,
}

impl Fixture {
    /// One server whose default bank is `bank-a`.
    fn new(tag: &str) -> Self {
        let store = Store::new(tag);
        let mut secrets = Vec::new();
        let credentials = store.write(
            &[
                ("operator", Actor::Harness, &["bank-a"]),
                ("seed", Actor::Harness, &["bank-a", "bank-b"]),
            ],
            &mut secrets,
        );
        Self {
            port: bound_server(&credentials, "bank-a"),
            ops: bearer(&secrets[0]),
            seed: bearer(&secrets[1]),
            secrets,
            store,
        }
    }

    /// Write one item at `bank` as the seeding principal, so the foreign bank
    /// holds durable rows the grant must keep out of reach.
    fn seed(&self, bank: &str, id: &str) {
        let body = call(
            "store",
            json!({
                "bank": bank,
                "item": {"id": id, "gist": "a bank-scope grant fixture item"},
                "category": "task_spec", "epistemic_kind": "fact"
            }),
        );
        let (status, raw) = post(self.port, &body, &self.seed);
        assert_eq!(status, 200, "seeding {bank} must be served: {raw}");
        assert!(tool_ok(&raw), "seeding {bank} must land: {raw}");
    }

    /// One operator call as the restricted principal: the raw body and the
    /// structured result it carries.
    fn ops_call(&self, tool: &str, args: Value) -> (String, Value) {
        let (status, raw) = post(self.port, &call(tool, args), &self.ops);
        assert_eq!(status, 200, "{raw}");
        assert_no_secret_material(&raw, &self.secrets, &self.store.digests());
        let parsed: Value = serde_json::from_str(&raw).expect("a JSON-RPC body");
        (raw, parsed["result"]["structuredContent"].clone())
    }
}

#[test]
fn reindex_with_an_explicit_foreign_bank_is_forbidden() {
    let f = Fixture::new("scope-reindex-explicit");
    f.seed("bank-b", "itm-b");

    let (raw, out) = f.ops_call(
        "reindex",
        json!({"bank": "bank-b", "target": "lexical", "dry_run": false, "confirm": true}),
    );
    assert!(forbidden(&raw), "a foreign bank must be refused: {raw}");
    assert_eq!(out["ok"], json!(false), "{out}");
}

#[test]
fn reindex_without_a_bank_stays_inside_the_grant() {
    let f = Fixture::new("scope-reindex-omitted");
    f.seed("bank-a", "itm-a");
    f.seed("bank-b", "itm-b");

    // A live lexical pass, not a dry run: the granted bank is reindexed and the
    // foreign bank is never swept into the scope.
    let (raw, out) = f.ops_call(
        "reindex",
        json!({"target": "lexical", "dry_run": false, "confirm": true}),
    );
    assert!(tool_ok(&raw), "the granted bank's reindex must run: {raw}");
    assert_eq!(out["bank"], json!("bank-a"), "{out}");
    assert_eq!(out["indexable_items"], json!(1), "{out}");
    assert!(
        !raw.contains("bank-b"),
        "the response names a foreign bank: {raw}"
    );
}

#[test]
fn repair_without_a_bank_stays_inside_the_grant() {
    let f = Fixture::new("scope-repair");
    f.seed("bank-b", "itm-b");

    // Naming the foreign bank is refused before any plan can be registered.
    let (raw, out) = f.ops_call("doctor", json!({"bank": "bank-b", "dry_run": false}));
    assert!(forbidden(&raw), "a foreign bank must be refused: {raw}");
    assert_eq!(out["code"], json!("forbidden"), "{out}");

    // The omitted bank plans for the granted bank only: the foreign bank's
    // coverage gap is not an action this plan carries.
    let (raw, plan) = f.ops_call("doctor", json!({"dry_run": false}));
    assert!(tool_ok(&raw), "the granted bank's plan must compose: {raw}");
    assert_eq!(plan["bank"], json!("bank-a"), "{plan}");
    assert_eq!(plan["actions"], json!([]), "{plan}");
    assert!(
        !raw.contains("bank-b"),
        "the plan names a foreign bank: {raw}"
    );
    let plan_id = plan["plan_id"].as_str().expect("a plan id").to_owned();

    // The registered plan resolves over the same transport and runs inside the
    // grant.
    let (raw, out) = f.ops_call("repair", json!({"plan_id": plan_id, "confirm": true}));
    assert!(tool_ok(&raw), "the granted plan must run: {raw}");
    assert_eq!(out["bank"], json!("bank-a"), "{out}");
    assert_eq!(out["plan_id"], json!(plan_id), "{out}");
    assert!(
        !raw.contains("bank-b"),
        "the repair names a foreign bank: {raw}"
    );

    // An explicit action list is confined the same way: the omitted bank is the
    // granted one, never the whole store.
    let (raw, out) = f.ops_call(
        "repair",
        json!({"actions": ["drop_orphan_assoc_edge"], "confirm": true}),
    );
    assert!(tool_ok(&raw), "the granted scope must run: {raw}");
    assert_eq!(out["bank"], json!("bank-a"), "{out}");
    assert_eq!(out["results"][0]["status"], json!("skipped"), "{out}");
    assert!(
        !raw.contains("bank-b"),
        "the repair names a foreign bank: {raw}"
    );
}

#[test]
fn diagnose_without_a_bank_names_no_foreign_bank() {
    let f = Fixture::new("scope-diagnose");
    f.seed("bank-b", "itm-b");

    let (raw, out) = f.ops_call("diagnose", json!({}));
    assert!(tool_ok(&raw), "{raw}");
    let banks: Vec<&str> = out["banks"]
        .as_array()
        .map(|banks| banks.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        !banks.contains(&"bank-b"),
        "the report names a foreign bank: {out}"
    );
    assert!(
        !raw.contains("bank-b"),
        "the response names a foreign bank: {raw}"
    );
    assert_eq!(
        out["indexable_items"],
        json!(0),
        "the foreign bank's volume is in the report: {out}"
    );

    // The foreign bank really is in this store: the seeding credential, which
    // may reach it, reads it back over the same transport. The silence above is
    // the grant, not an empty store.
    let body = call("diagnose", json!({"bank": "bank-b"}));
    let (status, raw) = post(f.port, &body, &f.seed);
    assert_eq!(status, 200, "{raw}");
    let privileged: Value = serde_json::from_str(&raw).expect("a JSON-RPC body");
    assert_eq!(
        privileged["result"]["structuredContent"]["banks"],
        json!(["bank-b"]),
        "{raw}"
    );
}

#[test]
fn an_operator_call_without_a_bearer_token_is_refused() {
    let f = Fixture::new("scope-no-token");
    let body = call(
        "reindex",
        json!({"target": "lexical", "dry_run": false, "confirm": true}),
    );

    let (status, raw) = post(f.port, &body, "");
    assert_eq!(status, 401, "no credential must not reach a tool: {raw}");
    assert!(!raw.contains("structuredContent"), "{raw}");

    // The same frame with the granted credential is served, so the 401 is the
    // transport's credential gate rather than a broken request.
    let (status, raw) = post(f.port, &body, &f.ops);
    assert_eq!(status, 200, "{raw}");
    assert!(tool_ok(&raw), "{raw}");
}
