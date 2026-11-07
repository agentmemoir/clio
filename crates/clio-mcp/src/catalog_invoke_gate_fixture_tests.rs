// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the arguments each confirmation-gated tool
//! needs before it can read its `confirm` argument at all, and for the proof
//! that supplying `confirm: true` changes that tool's answer, on both routes,
//! in the direction the gate is documented to move it.
//!
//! ## Owns
//! - The per-tool prerequisites: the stored items, the shared-surface item, the
//!   explicit `reindex` target, the `repair` action list, and the `correct`
//!   value that carries a gated tool past its own earlier refusals
//! - The per-tool expectation of what an unconfirmed call answers (a zero-write
//!   preview, or a gate refusal naming `confirm=true`) and of what the
//!   confirmed call answers instead
//! - The valid, authorized confirmed fixtures that must actually succeed, and
//!   the two tools whose confirmed success the test state cannot reach
//!
//! ## Does not own
//! - What the gate decides, or the shape of a preview (see `mutator_tools`,
//!   `hygiene_tools`, `ops_tools`, `retention_tools`, `sync_ack`)
//! - The equality of the two routes' answers for one call (see
//!   `catalog_invoke_confirmation_tests`)
//! - The wrapper's own refusals (see `catalog_invoke_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! A prerequisite here is what the shipping handler already requires before its
//! `confirm` read, never an accommodation of the test to the handler's answer:
//! if a tool stops needing one, the extra argument or stored item becomes dead
//! weight and this module changes. A gated tool that gains a prerequisite not
//! listed in [`prepare`], or whose gate moves, fails the per-tool assertions
//! here rather than passing silently.

use super::TOOL_INVOKE;
use super::confirmation_tests::{
    PINNED, contract_args, gated_tools, handler_with, outcome, without_own_identity,
};
use super::tests::{call, payload, refusal_of};
use crate::protocol::McpHandler;
use serde_json::{Value, json};

/// The bank every fixture uses unless it belongs to the shared surface.
const DEFAULT_BANK: &str = "default";

/// The dedicated bank the shared surface routes to.
const SHARED_BANK: &str = "shared";

/// The item `discard` removes.
const DISCARDED: &str = "itm-gate-discarded";

/// The item `shared_discard` removes, in the shared bank.
const SHARED_DISCARDED: &str = "itm-gate-shared-discarded";

/// The item `correct` rewrites.
const CORRECTED: &str = "itm-gate-corrected";

/// The item `hygiene_clean` is pointed at.
const HYGIENE_TARGET: &str = "itm-gate-hygiene";

/// Write one item through the `store` tool at a pinned id and timestamp, so an
/// argument naming it is an argument the store already holds.
///
/// The gist and source text carry the id, so two fixtures in one store are two
/// items rather than one duplicate of the other.
fn seed(handler: &McpHandler, bank: &str, id: &str) {
    let stored = payload(&call(
        handler,
        "store",
        &json!({
            "item": {
                "id": id,
                "subject_id": format!("subject-{id}"),
                "gist": format!("note {id}"),
                "snapshot": {"entity": "g", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": format!("The user has note {id} (1 note since 2024-01-01)."),
            "bank": bank,
            "timestamp": PINNED
        }),
    ));
    assert_eq!(
        stored["ok"],
        json!(true),
        "seed `{id}` in `{bank}`: {stored}"
    );
}

/// The arguments one gated tool needs to reach its `confirm` read, with
/// whatever that read needs stored or named first.
///
/// The derived arguments satisfy every contract; what this adds is the domain
/// prerequisite each handler checks before it reads `confirm`:
/// - `discard` and `shared_discard` resolve the item before the gate, so the
///   item exists, and the shared one lives in the bank the surface routes to
/// - `correct` refuses a request with no `new_value` before the gate
/// - `hygiene_clean` refuses an empty target list before the gate
/// - `repair` refuses an empty action list before the gate
/// - `reindex` defaults to a target that needs an embedding sidecar; the
///   lexical target needs none
pub(super) fn prepare(handler: &McpHandler, tool: &Value) -> Value {
    let name = tool["name"]
        .as_str()
        .expect("a published tool names itself");
    let mut args = contract_args(tool);
    match name {
        "discard" => {
            seed(handler, DEFAULT_BANK, DISCARDED);
            args["item_id"] = json!(DISCARDED);
        }
        "shared_discard" => {
            seed(handler, SHARED_BANK, SHARED_DISCARDED);
            args["item_id"] = json!(SHARED_DISCARDED);
        }
        "correct" => {
            seed(handler, DEFAULT_BANK, CORRECTED);
            args["item_id"] = json!(CORRECTED);
            args["new_value"] = json!("corrected");
        }
        "hygiene_clean" => {
            seed(handler, DEFAULT_BANK, HYGIENE_TARGET);
            args["item_ids"] = json!([HYGIENE_TARGET]);
            args["action"] = json!("archive");
        }
        "repair" => args["actions"] = json!(["rebuild_lexical_index"]),
        "reindex" => args["target"] = json!("lexical"),
        _ => {}
    }
    args
}

/// Assert what one route answered before and after `confirm: true`, for one
/// gated tool.
///
/// The direction is the tool's own: a zero-write preview that names what it
/// would do becomes the call that does it, and a gate refusal becomes a call
/// the gate admitted.
fn assert_direction(name: &str, route: &str, unconfirmed: &Value, confirmed: &Value) {
    match name {
        "discard" | "shared_discard" => {
            assert_eq!(
                payload(unconfirmed)["confirm"],
                json!(false),
                "`{name}` answers a preview on the {route} route: {unconfirmed}"
            );
            assert_eq!(
                payload(confirmed)["confirm"],
                json!(true),
                "`{name}` removes the item on the {route} route: {confirmed}"
            );
            assert!(
                payload(confirmed)["discarded"]["item_id"].is_string(),
                "`{name}` names the removed item on the {route} route: {confirmed}"
            );
        }
        "correct" => {
            assert!(
                payload(unconfirmed)["would_correct"]["item_id"].is_string(),
                "`correct` answers a preview on the {route} route: {unconfirmed}"
            );
            assert_eq!(
                payload(confirmed)["ok"],
                json!(true),
                "`correct` rewrites the item on the {route} route: {confirmed}"
            );
        }
        "hygiene_clean" | "reindex" => {
            assert_eq!(
                payload(unconfirmed)["dry_run"],
                json!(true),
                "`{name}` answers a dry run on the {route} route: {unconfirmed}"
            );
            assert_eq!(
                payload(confirmed)["dry_run"],
                json!(false),
                "`{name}` performs the work on the {route} route: {confirmed}"
            );
        }
        "repair" => {
            assert_eq!(
                payload(unconfirmed)["confirmed"],
                json!(false),
                "`repair` refuses to mutate unconfirmed on the {route} route: {unconfirmed}"
            );
            assert_eq!(
                payload(confirmed)["confirmed"],
                json!(true),
                "`repair` mutates once confirmed on the {route} route: {confirmed}"
            );
        }
        "retention_profile_set" => {
            assert_eq!(
                payload(unconfirmed)["stored"],
                json!(false),
                "`retention_profile_set` previews on the {route} route: {unconfirmed}"
            );
            assert_eq!(
                payload(confirmed)["stored"],
                json!(true),
                "`retention_profile_set` stores on the {route} route: {confirmed}"
            );
        }
        "sync_ack_skip" => {
            assert_eq!(
                payload(unconfirmed)["acked"],
                json!(false),
                "`sync_ack_skip` previews on the {route} route: {unconfirmed}"
            );
            assert!(
                payload(confirmed).get("sync_mutation_id").is_some(),
                "`sync_ack_skip` answers the acknowledging shape on the {route} route: {confirmed}"
            );
        }
        "erase_request" | "subject_request_retry" => {
            let (code, message) = refusal_of(unconfirmed);
            assert_eq!(
                code, "forbidden",
                "`{name}` refuses unconfirmed on the {route} route: {unconfirmed}"
            );
            assert!(
                message.contains("confirm=true"),
                "`{name}` refuses on the confirmation on the {route} route: {message}"
            );
            let (confirmed_code, confirmed_message) = refusal_of(confirmed);
            assert_eq!(
                confirmed_code, "forbidden",
                "`{name}` is past the gate and refused for authorization on the {route} route: \
                 {confirmed}"
            );
            assert!(
                confirmed_message.contains("compliance privilege"),
                "`{name}` requires a privileged actor on the {route} route: {confirmed_message}"
            );
        }
        other => panic!(
            "`{other}` publishes a `confirm` property and needs its gate direction asserted here"
        ),
    }
}

#[test]
fn every_gated_tool_reaches_its_confirmation_gate() {
    let gated = gated_tools();
    assert!(
        !gated.is_empty(),
        "the per-tool gate proof has at least one gated tool to walk"
    );
    let mut changed = 0_usize;
    for tool in &gated {
        let name = tool["name"]
            .as_str()
            .expect("a published tool names itself");
        // One fresh state per route: a confirmed call mutates one store, and
        // the unconfirmed call before it must see that same untouched store.
        let (direct_handler, direct_state) = handler_with("all");
        let (invoke_handler, invoke_state) = handler_with("all");
        let args = prepare(&direct_handler, tool);
        let prepared_for_invoke = prepare(&invoke_handler, tool);
        assert_eq!(
            prepared_for_invoke, args,
            "`{name}` is given the same arguments on both routes"
        );

        let unconfirmed = call(&direct_handler, name, &args);
        let unconfirmed_invoked = call(
            &invoke_handler,
            TOOL_INVOKE,
            &json!({ "name": name, "arguments": args }),
        );
        let direct_view =
            |response: &Value| without_own_identity(&outcome(response), &direct_state);
        let invoke_view =
            |response: &Value| without_own_identity(&outcome(response), &invoke_state);
        assert_eq!(
            invoke_view(&unconfirmed_invoked),
            direct_view(&unconfirmed),
            "`{name}` answers alike unconfirmed"
        );

        let mut confirmed_args = args.clone();
        confirmed_args["confirm"] = json!(true);
        let confirmed = call(&direct_handler, name, &confirmed_args);
        let confirmed_invoked = call(
            &invoke_handler,
            TOOL_INVOKE,
            &json!({ "name": name, "arguments": confirmed_args }),
        );
        assert_eq!(
            invoke_view(&confirmed_invoked),
            direct_view(&confirmed),
            "`{name}` answers alike once confirmed"
        );

        // The confirmation has to reach past the gate on each route in turn,
        // so neither route can pass on the other's answer.
        for (route, before, after) in [
            ("direct", &unconfirmed, &confirmed),
            ("wrapped", &unconfirmed_invoked, &confirmed_invoked),
        ] {
            assert_ne!(
                direct_view(after),
                direct_view(before),
                "`{name}`: a supplied confirmation changes what the {route} route answers"
            );
            assert_direction(name, route, before, after);
        }
        changed += 1;
    }
    assert_eq!(
        changed,
        gated.len(),
        "every gated tool answered differently once confirmed"
    );
}

#[test]
fn a_valid_confirmed_fixture_succeeds_on_both_routes() {
    // These four answer a zero-write preview unconfirmed and do the work once
    // confirmed, so their confirmed answer is a success worth asserting rather
    // than a domain error the comparison would accept either way.
    for name in [
        "discard",
        "shared_discard",
        "correct",
        "retention_profile_set",
    ] {
        let tool = crate::schema::catalog_def(name)
            .expect("a gated tool carries a catalog definition")
            .to_tool();
        let (direct_handler, direct_state) = handler_with("all");
        let (invoke_handler, invoke_state) = handler_with("all");
        let mut args = prepare(&direct_handler, &tool);
        let prepared_for_invoke = prepare(&invoke_handler, &tool);
        assert_eq!(prepared_for_invoke, args, "`{name}` is prepared alike");
        args["confirm"] = json!(true);

        let direct = call(&direct_handler, name, &args);
        let invoked = call(
            &invoke_handler,
            TOOL_INVOKE,
            &json!({ "name": name, "arguments": args }),
        );
        assert_eq!(
            payload(&direct)["ok"],
            json!(true),
            "`{name}` succeeds once confirmed on the direct route: {direct}"
        );
        assert_eq!(
            payload(&invoked)["ok"],
            json!(true),
            "`{name}` succeeds once confirmed on the wrapped route: {invoked}"
        );
        assert_eq!(
            without_own_identity(&outcome(&invoked), &invoke_state),
            without_own_identity(&outcome(&direct), &direct_state),
            "`{name}` hands back the same payload on both routes"
        );
    }
}

/// `erase_request` and `subject_request_retry` gate first and authorize second,
/// and their authorized path is not reachable from a unit test: the confirmed
/// erasure opens the deployment-resolved erasure journal, which an in-process
/// test can neither redirect (the crate forbids `unsafe_code`, so no environment
/// override) nor assume (it is the host's own journal, and its salt fingerprint
/// belongs to that deployment). Their confirmed success therefore stays outside
/// this module; what is asserted is the gate itself, and that a confirmed call
/// gets past it to the privileged-actor refusal.
#[test]
fn the_erasure_tools_gate_before_their_privileged_actor() {
    for name in ["erase_request", "subject_request_retry"] {
        let (handler, _state) = handler_with("all");
        let tool = crate::schema::catalog_def(name)
            .expect("a gated tool carries a catalog definition")
            .to_tool();
        let mut args = prepare(&handler, &tool);
        let unconfirmed = call(&handler, name, &args);
        let (code, message) = refusal_of(&unconfirmed);
        assert_eq!(
            code, "forbidden",
            "`{name}` refuses unconfirmed: {unconfirmed}"
        );
        assert!(
            message.contains("confirm=true"),
            "`{name}` refuses on the confirmation, before any actor check: {message}"
        );

        args["confirm"] = json!(true);
        let confirmed = call(&handler, name, &args);
        let (confirmed_code, confirmed_message) = refusal_of(&confirmed);
        assert_eq!(
            confirmed_code, "forbidden",
            "`{name}` refuses a caller without compliance privilege: {confirmed}"
        );
        assert!(
            confirmed_message.contains("compliance privilege"),
            "`{name}` names the privileged actor it needs: {confirmed_message}"
        );
        assert_ne!(
            confirmed_message, message,
            "`{name}` answers something other than its own gate once confirmed"
        );
    }
}
