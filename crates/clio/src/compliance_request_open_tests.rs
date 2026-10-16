// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI-side open of a durable subject request: the
//! configuration-derived peer-enablement fact frozen on the new record, the
//! pending per-peer snapshot persisted with it, the reused-id refusal, and the
//! fail-closed read of the credential store.
//!
//! ## Owns
//! - The injected env lookup and the scratch store every open test drives
//!
//! ## Does not own
//! - The module under test (`compliance_request_open`)
//! - The request and peer-action contracts (see `clio-compliance` /
//!   `clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_compliance::subject_request_store_wiring::row_to_record;
use clio_compliance::{PeerState, RequestState, SubjectRequestRecord};
use clio_config::auth_tokens::{AuthTokenRecord, save_auth_tokens};
use clio_config::context::Actor;
use clio_store::{LocalDevKms, PeerActionDisposition, PeerActionState, SqliteStore};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An injected env lookup over fixed pairs; every unnamed key reads as absent,
/// so no test reaches the host environment (the `compliance_cli_tests` pattern).
fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// A unique scratch directory, removed on drop (the `compliance_cli_tests`
/// scratch pattern).
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-compliance-open-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The scratch SQLite store the open persists into, through the crate's shared
/// test-key install so no real key store is reached (the `setup_support_tests`
/// and `backup_cli_fixtures` store pattern).
fn scratch_store(scratch: &Scratch) -> SqliteStore {
    crate::setup_support_tests::ensure_test_key();
    SqliteStore::open(
        &scratch.path().join("store.db"),
        Arc::new(LocalDevKms::new()),
    )
    .expect("the scratch store opens")
}

/// A subject scope over a named bank and subject.
fn subject_scope(bank: &str, subject: &str) -> RequestScope {
    RequestScope::Subject {
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
    }
}

/// One valid credential record naming `label` and covering `banks`.
fn record(label: &str, banks: &[&str]) -> AuthTokenRecord {
    AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: "0123456789abcdef".repeat(4),
        actor: Actor::Agent,
        banks: banks.iter().map(|bank| (*bank).to_owned()).collect(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

/// The effective configuration of a deployment whose sync is in play.
fn sync_enabled() -> Value {
    json!({"sync": {"omit": false}})
}

/// The record the durable store holds for `request_id`.
fn stored_record(store: &SqliteStore, request_id: &str) -> SubjectRequestRecord {
    // The store implements two `load` traits; this read is the durable request.
    let row = clio_store::DurableRequestStore::load(store, request_id)
        .expect("the durable row loads")
        .expect("the durable row exists");
    row_to_record(&row).expect("the stored record decodes")
}

/// A peers-disabled (single-host) configuration opens a request whose frozen
/// enablement reads not-applicable, and no peer row is written.
#[test]
fn a_peers_disabled_configuration_opens_a_not_applicable_request() {
    let scratch = Scratch::new("single-host");
    let store = scratch_store(&scratch);
    let env = env_from(&[]);
    let scope = subject_scope("bank-a", "subj-1");
    let outcome = open_subject_request(
        &store,
        &store,
        "req-single",
        RequestKind::Erasure,
        &scope,
        &json!({}),
        &env,
    )
    .expect("the request opens");
    assert_eq!(outcome, RegisterOutcome::Opened);
    let record = stored_record(&store, "req-single");
    assert_eq!(record.state, RequestState::Received);
    assert_eq!(record.scope, scope);
    assert_eq!(record.peer_enablement, PeerEnablement::NotApplicable);
    assert!(
        record.peers.is_empty(),
        "no peer work is owed: {:?}",
        record.peers
    );
    assert!(
        record.peer_banks.is_empty(),
        "a not-applicable request freezes no peer bank"
    );
    assert!(
        store
            .load_for_request("req-single")
            .expect("peer rows read")
            .is_empty(),
        "a not-applicable request snapshots nothing"
    );
}

/// An enabled configuration freezes exactly the enabled credential labels and
/// the deduplicated union of the banks they cover, and persists one pending
/// denial row per peer with the stored record.
#[test]
fn an_enabled_configuration_freezes_exactly_the_labeled_peers() {
    let scratch = Scratch::new("enabled");
    let store = scratch_store(&scratch);
    let tokens = scratch.path().join("auth_tokens.json");
    save_auth_tokens(
        &tokens,
        &[
            record("peer-b", &["bank-b"]),
            record("peer-a", &["bank-a", "bank-b"]),
        ],
    )
    .expect("the credential store saves");
    let token_arg = tokens.to_str().expect("utf8 token path").to_owned();
    let pairs = [("CLIO_AUTH_TOKEN_FILE", token_arg.as_str())];
    let env = env_from(&pairs);
    let outcome = open_subject_request(
        &store,
        &store,
        "req-enabled",
        RequestKind::Erasure,
        &subject_scope("bank-a", "subj-2"),
        &sync_enabled(),
        &env,
    )
    .expect("the request opens");
    assert_eq!(outcome, RegisterOutcome::Opened);
    let record = stored_record(&store, "req-enabled");
    assert_eq!(record.peer_enablement, PeerEnablement::Required);
    let labels: Vec<&str> = record.peers.keys().map(String::as_str).collect();
    assert_eq!(
        labels,
        ["peer-a", "peer-b"],
        "the frozen set is exactly the enabled labels"
    );
    let banks: Vec<&str> = record.peer_banks.iter().map(String::as_str).collect();
    assert_eq!(
        banks,
        ["bank-a", "bank-b"],
        "the frozen banks are the deduplicated union of the records' banks"
    );
    assert!(
        record
            .peers
            .values()
            .all(|state| *state == PeerState::Pending),
        "{:?}",
        record.peers
    );
    let rows = store
        .load_for_request("req-enabled")
        .expect("peer rows read");
    let mut peer_ids: Vec<&str> = rows.iter().map(|row| row.peer_id.as_str()).collect();
    peer_ids.sort_unstable();
    assert_eq!(peer_ids, ["peer-a", "peer-b"], "one row per enabled peer");
    assert!(
        rows.iter().all(|row| row.state == PeerActionState::Pending
            && row.disposition == PeerActionDisposition::Unknown),
        "{rows:?}"
    );
}

/// A reused id bound to a different scope is refused and the stored request is
/// left exactly as it was.
#[test]
fn a_reused_id_with_a_different_scope_is_refused_and_keeps_the_stored_request() {
    let scratch = Scratch::new("reused");
    let store = scratch_store(&scratch);
    let env = env_from(&[]);
    let opened = subject_scope("bank-a", "subj-1");
    open_subject_request(
        &store,
        &store,
        "req-reused",
        RequestKind::Erasure,
        &opened,
        &json!({}),
        &env,
    )
    .expect("the first open succeeds");
    let error = open_subject_request(
        &store,
        &store,
        "req-reused",
        RequestKind::Erasure,
        &subject_scope("bank-a", "subj-2"),
        &json!({}),
        &env,
    )
    .expect_err("a changed scope must refuse");
    assert_eq!(error.code(), "forbidden", "{error:?}");
    let record = stored_record(&store, "req-reused");
    assert_eq!(record.scope, opened, "the stored scope is unchanged");
    assert_eq!(record.peer_enablement, PeerEnablement::NotApplicable);
}

/// An enabled configuration whose credential store cannot be read refuses the
/// open, so no request is stored under a guessed peer set.
#[test]
fn an_unreadable_credential_store_refuses_the_open_and_stores_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("unreadable");
    let store = scratch_store(&scratch);
    let tokens = scratch.path().join("auth_tokens.json");
    std::fs::write(&tokens, "not a token store").expect("write the broken store");
    std::fs::set_permissions(&tokens, std::fs::Permissions::from_mode(0o644))
        .expect("widen the token store");
    let token_arg = tokens.to_str().expect("utf8 token path").to_owned();
    let pairs = [("CLIO_AUTH_TOKEN_FILE", token_arg.as_str())];
    let env = env_from(&pairs);
    let error = open_subject_request(
        &store,
        &store,
        "req-unreadable",
        RequestKind::Erasure,
        &subject_scope("bank-a", "subj-3"),
        &sync_enabled(),
        &env,
    )
    .expect_err("an unreadable credential store must refuse");
    assert_eq!(error.code(), "forbidden", "{error:?}");
    assert!(
        clio_store::DurableRequestStore::load(&store, "req-unreadable")
            .expect("the durable store loads")
            .is_none(),
        "nothing may be stored"
    );
}
