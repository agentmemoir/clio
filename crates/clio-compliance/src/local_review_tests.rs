// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the privileged local event review:
//! the ordered page and its keyset cursor, the visible pending gap that review
//! never reconciles, the actor refusal, the invalid limit and cursor arms, the
//! content-free rows that omit the outcome result text, the output-route
//! inventory, and the review-output custody record.
//!
//! ## Owns
//! - The in-memory store fixture, the query helper, and the traversal helper
//!
//! ## Does not own
//! - The review module and the durable writer under test
//! - The internal file-export writer (see `local_review_export_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the review page shape, the cursor, the actor gate, the
//! output-route inventory, or the custody call changes.
//!
//! Test-only. Never compiled into production builds.

use super::{
    DEFAULT_REVIEW_LIMIT, LocalReviewQuery, MAX_REVIEW_LIMIT, ReviewOutputRoute,
    record_review_output_custody, review_local_events, review_output_routes,
};
use clio_store::{
    DekProvider, LocalDevKms, LocalEventIntent, LocalEventState, LocalEventStore, SqliteStore,
};
use clio_types::{Actor, Bank, ErrorCode, OpContext};
use std::sync::Arc;

/// Event time the tests pass to every intent.
const EVENT_TIME: &str = "2026-01-01T00:00:00Z";

/// In-memory SQLite store (the `local_events_tests` fixture shape).
fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// An operation context for one actor.
fn ctx(actor: Actor) -> OpContext {
    OpContext::new(Bank::new("review-bank"), actor)
}

/// A review query with a chosen limit and optional cursor.
fn query(limit: usize, cursor: Option<&str>) -> LocalReviewQuery {
    LocalReviewQuery {
        limit,
        cursor: cursor.map(str::to_owned),
    }
}

/// An intent with fixed field values.
fn intent(action_id: &str) -> LocalEventIntent<'_> {
    LocalEventIntent {
        action_id,
        actor: "tester",
        scope: "scope-opaque",
        kind: "local_probe",
        event_time: EVENT_TIME,
    }
}

/// Persist one intent, naming the step on failure.
fn want_intent(store: &dyn LocalEventStore, action_id: &str) {
    store
        .record_intent(&intent(action_id))
        .unwrap_or_else(|err| panic!("intent {action_id} failed: {err}"));
}

/// Complete one intent, naming the step on failure.
fn want_outcome(store: &dyn LocalEventStore, action_id: &str, denied: bool) {
    store
        .record_outcome(action_id, denied, "redacted")
        .unwrap_or_else(|err| panic!("outcome {action_id} failed: {err}"));
}

/// Page to the end and collect the ordered action ids.
fn traverse(store: &dyn LocalEventStore, limit: usize) -> Vec<String> {
    let ctx = ctx(Actor::Harness);
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let page = review_local_events(&ctx, store, &query(limit, cursor.as_deref()))
            .expect("review page");
        ids.extend(page.rows.iter().map(|row| row.action_id.clone()));
        if !page.has_more {
            return ids;
        }
        cursor = page.next_cursor;
    }
}

/// A pending gap shows its action id and sequence, and review leaves it open.
#[test]
fn a_pending_gap_is_visible_and_review_leaves_it_open() {
    let store = store();
    want_intent(&store, "act-1");
    want_intent(&store, "act-2");
    want_intent(&store, "act-3");
    want_outcome(&store, "act-1", false);
    want_outcome(&store, "act-3", true);

    let page = review_local_events(&ctx(Actor::Harness), &store, &query(10, None))
        .expect("operator review");
    let rows: Vec<(i64, String, LocalEventState)> = page
        .rows
        .iter()
        .map(|row| (row.seq, row.action_id.clone(), row.state))
        .collect();
    assert_eq!(
        rows,
        vec![
            (1, "act-1".to_owned(), LocalEventState::Allowed),
            (2, "act-2".to_owned(), LocalEventState::Pending),
            (3, "act-3".to_owned(), LocalEventState::Denied),
        ]
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec!["act-2".to_owned()],
        "review must not reconcile the gap"
    );

    want_outcome(&store, "act-2", false);
    let page = review_local_events(&ctx(Actor::Harness), &store, &query(10, None))
        .expect("review after reconcile");
    assert_eq!(
        page.rows[1].state,
        LocalEventState::Allowed,
        "the gap closes only when a matching outcome is reconciled"
    );
}

/// An unprivileged actor is refused for both non-privileged roles.
#[test]
fn an_unprivileged_actor_is_refused() {
    let store = store();
    want_intent(&store, "act-1");
    for actor in [Actor::Agent, Actor::User] {
        let err = review_local_events(&ctx(actor), &store, &query(10, None))
            .expect_err("unprivileged review must refuse");
        assert_eq!(err.code, ErrorCode::Forbidden, "actor {actor:?}");
    }
    assert_eq!(
        store.list_events().expect("list").len(),
        1,
        "a refused review produces no page and leaves the store unchanged"
    );
}

/// A system actor may review, alongside the harness actor.
#[test]
fn a_privileged_actor_may_review() {
    let store = store();
    want_intent(&store, "act-1");
    let page =
        review_local_events(&ctx(Actor::System), &store, &query(10, None)).expect("system review");
    assert_eq!(page.rows.len(), 1);
}

/// Two traversals visit every action id once in order, and the gap stays.
#[test]
fn paging_visits_each_action_id_once_in_order_and_keeps_the_gap() {
    let store = store();
    want_intent(&store, "act-1");
    want_intent(&store, "act-2");
    want_intent(&store, "act-3");
    want_outcome(&store, "act-1", false);
    want_outcome(&store, "act-3", false);

    let first = traverse(&store, 1);
    let second = traverse(&store, 1);
    assert_eq!(
        first,
        vec!["act-1".to_owned(), "act-2".to_owned(), "act-3".to_owned()]
    );
    assert_eq!(
        second, first,
        "each traversal visits every action id once in order"
    );

    let page = review_local_events(&ctx(Actor::Harness), &store, &query(10, None)).expect("review");
    assert_eq!(
        page.rows[1].state,
        LocalEventState::Pending,
        "the gap stays visible across traversals"
    );
}

/// A page reports its next cursor and whether more rows remain.
#[test]
fn a_page_reports_its_next_cursor_and_has_more() {
    let store = store();
    want_intent(&store, "act-1");
    want_intent(&store, "act-2");

    let first =
        review_local_events(&ctx(Actor::Harness), &store, &query(1, None)).expect("first page");
    assert_eq!(first.rows[0].action_id, "act-1");
    assert_eq!(first.next_cursor.as_deref(), Some("1"));
    assert!(first.has_more);

    let last = review_local_events(&ctx(Actor::Harness), &store, &query(1, Some("1")))
        .expect("second page");
    assert_eq!(last.rows[0].action_id, "act-2");
    assert_eq!(last.next_cursor, None);
    assert!(!last.has_more);
}

/// A limit of zero or above the maximum is refused.
#[test]
fn a_limit_outside_the_bounds_is_invalid_argument() {
    let store = store();
    for limit in [0, MAX_REVIEW_LIMIT + 1] {
        let err = review_local_events(&ctx(Actor::Harness), &store, &query(limit, None))
            .expect_err("limit out of range");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "limit {limit}");
    }
    for limit in [DEFAULT_REVIEW_LIMIT, MAX_REVIEW_LIMIT] {
        let page = review_local_events(&ctx(Actor::Harness), &store, &query(limit, None))
            .unwrap_or_else(|err| panic!("limit {limit} should be accepted: {err}"));
        assert!(
            page.rows.is_empty(),
            "an empty store has no rows at limit {limit}"
        );
        assert!(
            !page.has_more,
            "an empty store has no more rows at limit {limit}"
        );
        assert_eq!(page.next_cursor, None, "an empty store has no next cursor");
    }
}

/// The privileged review rows carry no outcome result text: the store holds
/// the stored outcome text, but the review page omits it, matching the
/// content-free file export.
#[test]
fn the_review_rows_omit_the_outcome_result_text() {
    let store = store();
    want_intent(&store, "act-1");
    let marker = "SECRET-TOKEN sk-live-key identity-marker";
    store
        .record_outcome("act-1", false, marker)
        .expect("outcome with marker result text");
    assert_eq!(
        store.list_events().expect("list")[0].result,
        Some(marker.to_owned()),
        "the store holds the marker text, so review must be the layer that strips it"
    );

    let page = review_local_events(&ctx(Actor::Harness), &store, &query(10, None))
        .expect("operator review");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(
        page.rows[0].result, None,
        "a review row carries no outcome result text"
    );
    let rendered = format!("{page:?}");
    for piece in ["SECRET-TOKEN", "sk-live-key", "identity-marker"] {
        assert!(
            !rendered.contains(piece),
            "the review page must not carry the marker {piece:?}"
        );
    }
}

/// A non-decimal, fractional, or negative cursor is refused.
#[test]
fn a_bad_cursor_is_invalid_argument() {
    let store = store();
    for cursor in ["abc", "-1", "1.5", ""] {
        let err = review_local_events(&ctx(Actor::Harness), &store, &query(10, Some(cursor)))
            .expect_err("bad cursor");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "cursor {cursor:?}");
    }
}

/// The route inventory marks only the operator stdout route available.
#[test]
fn the_output_route_inventory_marks_only_operator_stdout_available() {
    let routes = review_output_routes();
    assert_eq!(
        routes,
        [
            ReviewOutputRoute::OperatorStdout,
            ReviewOutputRoute::FileExport
        ]
    );
    assert!(ReviewOutputRoute::OperatorStdout.available());
    assert!(
        !ReviewOutputRoute::FileExport.available(),
        "the file export stays unavailable"
    );
}

/// Custody records the operator copy with a sha256 digest and the clio producer.
#[test]
fn custody_records_the_operator_copy_of_review_output() {
    let manifest = record_review_output_custody(
        &ctx(Actor::Harness),
        b"",
        "operator",
        EVENT_TIME,
        EVENT_TIME,
    )
    .expect("custody");
    assert_eq!(manifest.producer, "clio");
    assert_eq!(manifest.receiver, "operator");
    assert_eq!(
        manifest.copy_digest,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "the digest is sha256 over the copy bytes"
    );
}

/// Custody refuses an unprivileged actor.
#[test]
fn custody_refuses_an_unprivileged_actor() {
    let err =
        record_review_output_custody(&ctx(Actor::Agent), b"x", "operator", EVENT_TIME, EVENT_TIME)
            .expect_err("unprivileged custody");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// Custody refuses a timestamp that is not RFC 3339 UTC.
#[test]
fn custody_refuses_a_bad_timestamp() {
    let err = record_review_output_custody(
        &ctx(Actor::Harness),
        b"x",
        "operator",
        "not-a-time",
        EVENT_TIME,
    )
    .expect_err("bad timestamp");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}
