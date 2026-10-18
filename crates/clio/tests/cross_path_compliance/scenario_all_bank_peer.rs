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
//! - The stale-replay, unrelated-subject, and partial-bank assertions
//!
//! ## Does not own
//! - The real deployment driver and spawned peer process (see `peer_support`)
//! - The synthetic stores, probes, and Postgres gate (see `fixtures`,
//!   `support`, and `support_pg`)
//!
//! ## Boundary
//!
//! Changes to which peer states this claim proves should normally require
//! changes here. Changes to the peer protocol should not.
//!
//! Keep this module focused on the all-bank-and-peer claim.

use crate::fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER,
    DENIED_SUBJECT, backup_dir, journal_file, open_pg, open_sqlite, scratch, unique, write_item,
};
use crate::peer_support::{Deployment, Peer, PeerProbe};
use crate::support::{Readback, readback, subject_key_destroyed};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::{
    ComplianceStore, DurableRequestStore, LocalEventStore, PeerActionStore, ScopedSchema, Store,
};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use clio_compliance::subject_request_store_wiring::record_to_row;
use clio_compliance::{
    PeerEnablement, RequestCommand, RequestKind, RequestScope, SubjectRequestLedger, snapshot_peers,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

/// The second bank that shares the denied subject's DEK.
const PEER_BANK: &str = "cross-path-peer-bank";
/// The denied subject's item in [`PEER_BANK`].
const PEER_ITEM: &str = "cross-path-peer-item";
/// The frozen peer label of the all-bank arm.
const PEER_LABEL: &str = "cross-path-peer";
/// The one deployment key both this process and every spawned command use, so a
/// store written here and a store the command rewrites share their key slots.
const KEY: [u8; 32] = [0x11; 32];

/// The store surface both arms drive. Every backend implements all of these, so
/// one body runs on SQLite and Postgres.
trait CrossStore:
    Store + ComplianceStore + DurableRequestStore + PeerActionStore + LocalEventStore
{
}

impl<T> CrossStore for T where
    T: Store + ComplianceStore + DurableRequestStore + PeerActionStore + LocalEventStore
{
}

/// One synthetic item carrying `subject` in `bank`.
fn item_in(bank: &str, id: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "synthetic cross-path subject"})),
        gist: Some(gist.to_owned()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Seed the denied subject's item in each sharing bank, and the control item.
fn seed<S: Store>(store: &S) {
    for (bank, id, subject, gist) in [
        (BANK, DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER),
        (
            PEER_BANK,
            PEER_ITEM,
            DENIED_SUBJECT,
            "marker-peer-cross-path",
        ),
        (BANK, CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER),
    ] {
        store
            .create_memory_item(&item_in(bank, id, subject, gist), "harness")
            .expect("write the synthetic item");
    }
}

/// Write this process's key file and install the same key on this thread.
fn install_key(path: PathBuf) -> PathBuf {
    clio_config::secret_seal::test_key::use_key(KEY);
    std::fs::write(&path, KEY).expect("write the harness key file");
    path
}

/// Persist an in-progress erasure request over the denied subject, with the
/// given frozen peer labels and sharing banks, and snapshot its peer rows.
fn seed_request<S: DurableRequestStore + PeerActionStore>(
    store: &S,
    request_id: &str,
    peers: &[&str],
    peer_banks: &[&str],
) {
    let scope = RequestScope::Subject {
        bank_id: BANK.to_owned(),
        subject_id: DENIED_SUBJECT.to_owned(),
    };
    let enablement = if peers.is_empty() {
        PeerEnablement::NotApplicable
    } else {
        PeerEnablement::Required
    };
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register_with_enablement(
            request_id,
            RequestKind::Erasure,
            &scope,
            enablement,
            peers,
            peer_banks,
        )
        .expect("the erasure request registers");
    ledger
        .apply(request_id, &RequestCommand::Begin)
        .expect("the request starts");
    let record = ledger
        .record(request_id)
        .expect("the record exists")
        .clone();
    store
        .save(&record_to_row(&record))
        .expect("the record persists");
    snapshot_peers(store, &record).expect("the peer rows snapshot");
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
    assert!(
        probe
            .dead_letter_reasons()
            .iter()
            .any(|reason| reason == "erased_subject"),
        "the stale replay is quarantined as the erased subject"
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

/// T04 all-bank arm: the exact all-bank erasure runs through the command, a real
/// peer process is offline across it and rejoins stale, the erased marker never
/// returns, and peer reconciliation plus the peer copy disposition control
/// closure without a repeat.
fn all_bank<S: CrossStore>(deployment: &Deployment, open: impl Fn() -> S) {
    let request_id = unique("all-bank");
    let store = open();
    seed(&store);
    seed_request(&store, &request_id, &[PEER_LABEL], &[PEER_BANK]);
    drop(store);

    // A real peer process holds the denied bank. It is offline across the
    // erasure and rejoins stale: its journal learns the denial on restart.
    let mut peer = Peer::start(
        &deployment.peer_dir,
        &deployment.journal,
        &deployment.backups,
        &[BANK],
    );
    peer.offline();

    // The exact all-bank erasure: the live set is both sharing banks.
    assert_first_erasure_pending(deployment, &request_id);

    // The erased marker never returns; the unrelated control still works.
    let store = open();
    let erased = readback(&store, DENIED_ITEM);
    assert!(
        matches!(
            erased,
            Readback::Absent | Readback::DestroyedKey | Readback::KeyMissing
        ),
        "the denied item is never readable again: {erased:?}"
    );
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
        status["peers"][PEER_LABEL],
        json!("pending"),
        "the offline peer holds the copy open: {status}"
    );
    ok_json(&deployment.disposition(&request_id, PEER_LABEL, "deletion-1"));

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
    seed_request(&store, &request_id, &[], &[]);
    drop(store);

    let refused = deployment.retry(&request_id);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a partial-bank confirmation refuses: {stderr}"
    );
    assert!(
        stderr.contains("forbidden"),
        "the refusal names the coordination refusal: {stderr}"
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
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the control is unaffected"
    );
}

/// One deployment fixture over `scratch`, whose store is `db`, with the private
/// peer scratch directory the real-peer arm spawns into.
fn deployment_for(scratch: &crate::common::Scratch, db: String) -> Deployment {
    Deployment {
        journal: journal_file(scratch, "journal/erasure.json"),
        keys: install_key(scratch.join("clio.key")),
        kms: scratch.join("clio.kms.json"),
        backups: backup_dir(scratch),
        db,
        peer_dir: scratch.join("peer"),
    }
}

/// One SQLite deployment whose scratch is also returned for the opener.
fn sqlite_deployment(tag: &str) -> (crate::common::Scratch, Deployment) {
    let scratch = scratch(tag);
    let db = scratch.join("clio.db").to_string_lossy().into_owned();
    let deployment = deployment_for(&scratch, db);
    (scratch, deployment)
}

#[test]
fn sqlite_exact_all_bank_erasure_never_returns_the_marker() {
    let (_scratch, deployment) = sqlite_deployment("sqlite-all-bank");
    let db = Path::new(&deployment.db).to_path_buf();
    let kms = deployment.kms.clone();
    all_bank(&deployment, || open_sqlite(&db, &kms));
}

#[test]
fn sqlite_partial_bank_confirmation_has_no_irreversible_effect() {
    let (_scratch, deployment) = sqlite_deployment("sqlite-partial-bank");
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
    let deployment = deployment_for(&scratch, schema.url().to_owned());
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
    let deployment = deployment_for(&scratch, schema.url().to_owned());
    let db = deployment.db.clone();
    let kms = deployment.kms.clone();
    partial_bank(&deployment, || open_pg(&db, &kms));
}
