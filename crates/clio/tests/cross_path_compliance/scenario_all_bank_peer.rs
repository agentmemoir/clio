// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the all-bank erasure claim: the exact erasure runs through
//! commands, a restarted offline peer's stale replay never returns the marker,
//! unrelated subjects work, and a partial-bank confirmation has no effect.
//!
//! ## Owns
//! - The all-bank erase command run and its reconcile with the peer
//! - The erased-state, peer-slot, stale-replay, and partial-bank assertions
//!
//! ## Does not own
//! - The real deployment driver and spawned peer process (see `peer_process_test_support`)
//! - The synthetic stores, probes, and Postgres gate (see `case_fixtures`,
//!   `support_test_support`, and `support_pg_test_support`)
//! - The seeded items, the frozen peer set, and the read-back probes (see
//!   `scenario_all_bank_peer_test_support`)
//! - The shared thread-key guard (see `case_fixtures`)
//! - The shared refusal helper and the partial-bank refusal (see `cli_test_support`)
//!
//! ## Boundary
//!
//! Changes to which peer states this claim proves should normally require
//! changes here. Changes to the peer protocol should not.
//!
//! Keep this module focused on the all-bank-and-peer claim.

use crate::case_fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER,
    DENIED_SUBJECT, ThreadKeyGuard, backup_dir, journal_file, open_pg, open_sqlite, scratch,
    unique, write_item,
};
use crate::cli_test_support::{PARTIAL_BANK_REFUSAL, assert_refusal};
use crate::peer_process_test_support::{CREDENTIAL_STORE_FILE, Deployment, Peer, PeerProbe};
use crate::scenario_all_bank_peer_test_support::{
    PEER_BANK, PEER_ITEM, assert_recorded_peer_set, derive_peer_set, erased_item_hashes, item_hash,
    item_read, seed, seed_request,
};
use crate::support_pg_test_support::{lock_pg, pg_url, postgres_reachable, require_postgres};
use crate::support_test_support::{Readback, readback, subject_key_destroyed};
use clio::store::{
    BankScopeStore, ComplianceStore, DurableRequestStore, LocalEventStore, PeerActionStore,
    ScopedSchema, Store,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

/// The one deployment key both this process and every spawned command use, so a
/// store written here and a store the command rewrites share their key slots.
const KEY: [u8; 32] = [0x11; 32];

/// The store surface both arms drive. Every backend implements all of these, so
/// one body runs on SQLite and Postgres.
trait CrossStore:
    Store + ComplianceStore + DurableRequestStore + PeerActionStore + LocalEventStore + BankScopeStore
{
}

impl<T> CrossStore for T where
    T: Store
        + ComplianceStore
        + DurableRequestStore
        + PeerActionStore
        + LocalEventStore
        + BankScopeStore
{
}

/// Write this process's key file and install the same key on this thread.
///
/// The returned guard clears the thread-local key when it drops, so the key
/// never outlives the scenario that installed it.
pub(crate) fn install_key(path: PathBuf) -> (PathBuf, ThreadKeyGuard) {
    clio_config::secret_seal::test_key::use_key(KEY);
    std::fs::write(&path, KEY).expect("write the harness key file");
    (path, ThreadKeyGuard)
}

/// A successful run's JSON payload, or a panic quoting both streams.
fn ok_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "clio failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("clio printed JSON")
}

/// The first all-bank erasure over an offline peer: the copy stays open, the
/// action stays pending, and one effect has run.
fn assert_first_erasure_pending(deployment: &Deployment, request_id: &str) {
    let first = ok_json(&deployment.retry(request_id));
    assert_eq!(
        first["technical_completion"],
        json!("pending"),
        "an unsettled peer holds closure open: {first}"
    );
    assert!(
        !first["pending_operator_actions"]
            .as_array()
            .expect("operator actions")
            .is_empty(),
        "the erasure action stays pending: {first}"
    );
    assert!(
        first["effects_after"].as_u64().unwrap_or(0) >= 1,
        "the all-bank effect ran: {first}"
    );
}

/// Restart the peer over a stale writer: the erased subject's replay is
/// quarantined on the wire and the unrelated subject still applies.
fn assert_restarted_peer_quarantines_the_stale_replay(deployment: &Deployment, peer: &mut Peer) {
    peer.restart();
    let writer_dir = deployment.peer_dir.join("writer");
    std::fs::create_dir_all(&writer_dir).expect("create the writer directory");
    let writer = Arc::new(open_sqlite(
        &writer_dir.join("writer.db"),
        &writer_dir.join("writer.kms.json"),
    ));
    write_item(
        writer.as_ref(),
        &format!("{PEER_ITEM}-stale"),
        DENIED_SUBJECT,
        DENIED_MARKER,
    );
    write_item(
        writer.as_ref(),
        "writer-control",
        CONTROL_SUBJECT,
        CONTROL_MARKER,
    );
    peer.push(&writer, "writer-1", &[BANK])
        .expect("the restarted peer answers the wire");
    let probe = PeerProbe::new(&deployment.peer_dir.join("peer.db"));
    let reasons = probe.dead_letter_reasons();
    assert!(
        reasons.iter().any(|reason| reason == "erased_subject"),
        "the stale replay is quarantined as the erased subject: {reasons:?}"
    );
    assert_eq!(
        probe.subject_items(BANK, DENIED_SUBJECT),
        0,
        "the stale replay never becomes readable at the peer"
    );
    assert_eq!(
        probe.subject_items(BANK, CONTROL_SUBJECT),
        1,
        "the unrelated subject applies at the peer"
    );
    peer.offline();
}

/// The erased state of every bank that holds the subject's data: the purge's
/// own tombstone covers the item in each of them, and the shredded key makes
/// each content unreadable.
fn assert_every_bank_erased<S: CrossStore>(store: &S, request_id: &str) {
    // The purge: the erasure tombstone covers the item in every bank that holds
    // the subject's data, so the fan-out reached the second bank. A shredded key
    // alone writes no tombstone.
    assert_eq!(
        erased_item_hashes(store, BANK, request_id),
        [item_hash(DENIED_ITEM), item_hash(PEER_ITEM)]
            .into_iter()
            .collect(),
        "the erasure covers the item in every bank that shares the subject key"
    );
    // The shred: the content of both items is unreadable, with the production's
    // exact error code.
    assert_eq!(
        item_read(store, BANK, DENIED_ITEM),
        Err("erased_subject".to_owned()),
        "the denied item is unreadable: the subject key is shredded"
    );
    assert_eq!(
        item_read(store, PEER_BANK, PEER_ITEM),
        Err("erased_subject".to_owned()),
        "the second bank's item is unreadable: the subject key is shredded"
    );
}

/// The peer's slot after the disposition, and the request outcome it leaves.
fn assert_disposition_settles_the_peer(
    deployment: &Deployment,
    request_id: &str,
    peer_label: &str,
) {
    let disposed = ok_json(&deployment.disposition(request_id, peer_label, "deletion-1"));
    assert_eq!(
        disposed["peers"][peer_label],
        json!("disposed"),
        "the disposition settles the peer slot: {disposed}"
    );
    assert_eq!(
        disposed["request_outcome"],
        json!("open"),
        "the settled peer leaves the request open: {disposed}"
    );
}

/// T04 all-bank arm: the exact all-bank erasure runs through the command, a real
/// peer process is offline across it and rejoins stale, the erased marker never
/// returns, and peer reconciliation plus the peer copy disposition control
/// closure without a repeat.
fn all_bank<S: CrossStore>(deployment: &Deployment, open: impl Fn() -> S) {
    let request_id = unique("all-bank");
    let store = open();
    seed(&store);
    // The production derives the banks that share the subject key from the live
    // items. The peer serves exactly those banks, so the credential store the
    // production reads names every bank the erasure has to cover.
    let live = store
        .erase_bank_set(BANK, DENIED_SUBJECT)
        .expect("read the live bank set");
    drop(store);

    // A real peer process holds the denied banks. It is offline across the
    // erasure and rejoins stale: its journal learns the denial on restart.
    let bank_refs: Vec<&str> = live.iter().map(String::as_str).collect();
    let mut peer = Peer::start(
        &deployment.peer_dir,
        &deployment.journal,
        &deployment.backups,
        &bank_refs,
    );
    peer.offline();

    // The production derives the peer set and the peer banks from the peer's
    // credential store, and the request freezes exactly that set.
    let derived = derive_peer_set(&peer.credential_store());
    let peer_label = derived
        .labels
        .first()
        .expect("the credential store names the peer")
        .clone();
    let store = open();
    seed_request(&store, &request_id, &derived);
    assert_recorded_peer_set(&store, &request_id, &derived);
    drop(store);

    // The exact all-bank erasure: the live set is both sharing banks.
    assert_first_erasure_pending(deployment, &request_id);

    // The erased marker never returns; the unrelated control still works.
    let store = open();
    assert_every_bank_erased(&store, &request_id);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the unrelated control subject still works"
    );
    drop(store);

    // The restarted peer refuses the stale replay of the erased subject on the
    // wire, and applies the unrelated subject.
    assert_restarted_peer_quarantines_the_stale_replay(deployment, &mut peer);

    // The frozen peer stays pending until it is reconciled.
    let status = ok_json(&deployment.verb("request-status", &request_id));
    assert_eq!(
        status["peers"][peer_label.as_str()],
        json!("pending"),
        "the offline peer holds the copy open: {status}"
    );
    // The disposition settles the peer slot, and the request stays open: the
    // operator has not closed it.
    assert_disposition_settles_the_peer(deployment, &request_id, &peer_label);

    // Reconciliation without a resume proves nothing: the action stays pending.
    let reconciled = ok_json(&deployment.verb("peer-retry", &request_id));
    assert_eq!(
        reconciled["request_id"],
        json!(request_id.as_str()),
        "the reconcile reports the request"
    );
    let action = format!(
        "{}:{request_id}",
        clio_compliance::ProtectedAction::Erasure.producer_id()
    );
    let store = open();
    assert!(
        store.pending_actions().expect("pending").contains(&action),
        "reconciliation without resume facts claims nothing"
    );
    drop(store);

    // The settled peer lets the next retry close the action once, no repeat.
    let settled = ok_json(&deployment.retry(&request_id));
    assert!(
        settled["action_id"].is_string(),
        "the evidenced peer names the settled action: {settled}"
    );
    assert_eq!(
        settled["effects_before"], settled["effects_after"],
        "the closure repeats no protected effect: {settled}"
    );
    assert_eq!(
        settled["request_outcome"],
        json!("open"),
        "the closed technical work leaves the request open: {settled}"
    );
    assert!(
        settled["pending_operator_actions"]
            .as_array()
            .expect("operator actions")
            .is_empty(),
        "the settled request owes no operator action: {settled}"
    );
    let store = open();
    assert!(
        !store.pending_actions().expect("pending").contains(&action),
        "the action settles exactly once"
    );
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the control still works after closure"
    );
}

/// T04 partial-bank arm: a one-bank confirmation over a shared key is refused early.
fn partial_bank<S: CrossStore>(deployment: &Deployment, open: impl Fn() -> S) {
    let request_id = unique("partial-bank");
    let store = open();
    seed(&store);
    // This deployment runs no peer, so the production derives no peer set from
    // its empty credential store.
    let derived = derive_peer_set(&deployment.peer_dir.join(CREDENTIAL_STORE_FILE));
    seed_request(&store, &request_id, &derived);
    drop(store);

    let refused = deployment.retry(&request_id);
    assert_refusal(
        refused.status.code(),
        &String::from_utf8_lossy(&refused.stderr),
        &PARTIAL_BANK_REFUSAL,
    );

    // No irreversible effect: the key is intact, no intent is durable, and the
    // denied item is still readable.
    let store = open();
    assert!(
        !subject_key_destroyed(&store, DENIED_SUBJECT),
        "a refused partial-bank erasure destroys no key"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "a refused erasure writes no intent"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Readable,
        "the denied item survives a refused partial-bank erasure"
    );
    assert_eq!(
        item_read(&store, PEER_BANK, PEER_ITEM),
        Ok(Some(PEER_ITEM.to_owned())),
        "the second bank's item survives a refused partial-bank erasure"
    );
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the control is unaffected"
    );
}

/// One deployment fixture over `scratch`, whose store is `db`, with the private
/// peer scratch directory the real-peer arm spawns into.
fn deployment_for(scratch: &crate::common::Scratch, db: String) -> (Deployment, ThreadKeyGuard) {
    let (keys, key_guard) = install_key(scratch.join("clio.key"));
    let deployment = Deployment {
        journal: journal_file(scratch, "journal/erasure.json"),
        keys,
        kms: scratch.join("clio.kms.json"),
        backups: backup_dir(scratch),
        db,
        peer_dir: scratch.join("peer"),
    };
    (deployment, key_guard)
}

/// One SQLite deployment whose scratch is also returned for the opener.
fn sqlite_deployment(tag: &str) -> (crate::common::Scratch, Deployment, ThreadKeyGuard) {
    let scratch = scratch(tag);
    let db = scratch.join("clio.db").to_string_lossy().into_owned();
    let (deployment, key_guard) = deployment_for(&scratch, db);
    (scratch, deployment, key_guard)
}

#[test]
fn sqlite_exact_all_bank_erasure_never_returns_the_marker() {
    let (_scratch, deployment, _key_guard) = sqlite_deployment("sqlite-all-bank");
    let db = Path::new(&deployment.db).to_path_buf();
    let kms = deployment.kms.clone();
    all_bank(&deployment, || open_sqlite(&db, &kms));
}

#[test]
fn sqlite_partial_bank_confirmation_has_no_irreversible_effect() {
    let (_scratch, deployment, _key_guard) = sqlite_deployment("sqlite-partial-bank");
    let db = Path::new(&deployment.db).to_path_buf();
    let kms = deployment.kms.clone();
    partial_bank(&deployment, || open_sqlite(&db, &kms));
}

#[test]
fn postgres_exact_all_bank_erasure_never_returns_the_marker() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let schema = ScopedSchema::new(&base);
    let scratch = scratch("pg-all-bank");
    let (deployment, _key_guard) = deployment_for(&scratch, schema.url().to_owned());
    let db = deployment.db.clone();
    let kms = deployment.kms.clone();
    all_bank(&deployment, || open_pg(&db, &kms));
}

#[test]
fn postgres_partial_bank_confirmation_has_no_irreversible_effect() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let schema = ScopedSchema::new(&base);
    let scratch = scratch("pg-partial-bank");
    let (deployment, _key_guard) = deployment_for(&scratch, schema.url().to_owned());
    let db = deployment.db.clone();
    let kms = deployment.kms.clone();
    partial_bank(&deployment, || open_pg(&db, &kms));
}
