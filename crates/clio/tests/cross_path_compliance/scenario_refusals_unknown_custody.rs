// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unknown-copy-custody refusal of the
//! cross-path compliance proof: a copy whose holder is not known refuses the
//! completion claim even when its technical state is done, and the request
//! closes nothing.
//!
//! ## Owns
//! - The unknown-custody copy case and the named refusal it returns
//! - The assertion that the request outcome stays open
//! - The same case on SQLite and on loopback Postgres
//!
//! ## Does not own
//! - The request ledger and the copy-disposition fold (see `clio-compliance`)
//! - The CLI driver, the fixtures, and the shared request environment (see
//!   `cli`, `fixtures`, and `refusals_test_support`)
//!
//! ## Boundary
//!
//! Changes to which refused state this claim proves should normally require
//! changes here. Changes to the request ledger should not.
//!
//! Keep this module focused on the unknown-custody refusal.

use crate::fixtures::{
    DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, open_pg, open_sqlite, seed_pg, seed_sqlite,
};
use crate::refusals_test_support::{Env, assert_forbidden, assert_open_copy, ok, seed_done_path};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::{CopyInventoryStore, DurableRequestStore, ScopedSchema};
use clio_compliance::subject_request_store_wiring::copy_row_disposition;
use clio_compliance::{CopyDispositionOutcome, copy_closure_facts};
use serde_json::json;

/// The store surface the unknown-custody arm drives on either backend.
trait CustodyStore: CopyInventoryStore + DurableRequestStore {}
impl<T: CopyInventoryStore + DurableRequestStore> CustodyStore for T {}

/// The flags of one copy disposition: the copy, the class naming its holder,
/// the action it still owes, and its technical state.
fn copy_rest(request: &str, copy_id: &str, custody: &str, action: &str, state: &str) -> String {
    format!(
        "--actor harness --copy-id {copy_id} --request-id {request} --custody {custody} \
         --owner harness --required-action {action} --state {state}"
    )
}

/// The unknown-custody refusal on one deployment.
///
/// The copy's technical state is done, so the only thing it still owes is its
/// classification. The production fold refuses any completion claim for an
/// unknown holder, the closure gate refuses the open action with the
/// production's own message, and the request stays open.
fn unknown_custody_refuses_closure<S: CustodyStore>(env: &Env, store: &S, request: &str) {
    ok(&env.start(request, DENIED_SUBJECT), "request-start");

    // A delivered copy under a known holder: it is closed, so it is not what
    // refuses the completion.
    let delivered_id = format!("copy-delivered-{request}");
    ok(
        &env.command(
            "request-copy",
            &copy_rest(request, &delivered_id, "clio", "delivery", "done"),
        ),
        "request-copy delivered",
    );

    // The unknown-custody copy: the holder is not known.
    let unknown_id = format!("copy-unknown-{request}");
    ok(
        &env.command(
            "request-copy",
            &copy_rest(request, &unknown_id, "unknown", "erase", "done"),
        ),
        "request-copy unknown",
    );

    // The stored row carries the unknown custody the fold reads.
    let rows = CopyInventoryStore::load_all(store).expect("load the copy rows");
    let unknown = rows
        .iter()
        .find(|row| row.copy_id == unknown_id)
        .expect("the unknown-custody copy row");
    assert_eq!(unknown.custody, "unknown", "the stored custody: {rows:?}");

    // The production fold names the unknown holder as a refusal: the delivered
    // copy closes, the unknown one stays open, and no completion claim of any
    // kind is supported.
    let delivered = rows
        .iter()
        .find(|row| row.copy_id == delivered_id)
        .expect("the delivered copy row");
    let facts = copy_closure_facts(
        &[
            copy_row_disposition(delivered).expect("parse the delivered copy row"),
            copy_row_disposition(unknown).expect("parse the unknown copy row"),
        ],
        true,
        false,
    );
    assert_eq!(
        facts.outcome,
        CopyDispositionOutcome::Refused,
        "an unknown holder refuses the completion claim"
    );
    assert_eq!(
        facts.inputs.open_copy_actions, 1,
        "only the unknown-custody copy stays open"
    );

    // The product path is done, so the closure's only blocker is the copy.
    seed_done_path(store, request, "erasure");
    let close = env.command(
        "request-close",
        &format!("--request-id {request} --product-path erasure"),
    );
    assert_forbidden(&close);
    assert_open_copy(&close);
    let stderr = String::from_utf8_lossy(&close.stderr).into_owned();
    assert!(
        !stderr.contains("UnknownProductPath"),
        "the product path is known, so the copy refuses: {stderr}"
    );

    // Nothing unsafe completes: the request stays open.
    let status = env.status(request);
    assert_eq!(
        status["request_outcome"],
        json!("open"),
        "the request closes nothing: {status}"
    );
}

/// The unknown-custody refusal on SQLite.
#[test]
fn scenario_refusals_unknown_custody_refuses_closure() {
    let env = Env::new("refusals-unknown-custody");
    seed_sqlite(
        env.path(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    let store = open_sqlite(env.path(), &env.kms);
    unknown_custody_refuses_closure(&env, &store, "req-unknown-custody");
}

/// The same unknown-custody refusal on loopback Postgres.
#[test]
fn scenario_refusals_unknown_custody_refuses_closure_postgres() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let env = Env::postgres("refusals-unknown-custody-pg", target.url());
    seed_pg(
        env.db(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    let store = open_pg(env.db(), &env.kms);
    unknown_custody_refuses_closure(&env, &store, "req-unknown-custody-pg");
}
