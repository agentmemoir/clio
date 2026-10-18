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
//! - The peer process and its wire (see `clio-sync` and `clio-compliance`)
//! - The stores, CLI driver, and Postgres gate (see `fixtures`, `cli`, `support_pg`)
//!
//! ## Boundary
//!
//! Changes to which peer states this claim proves should normally require
//! changes here. Changes to the peer protocol should not.
//!
//! Keep this module focused on the all-bank-and-peer claim.

use crate::cli::clio_command;
use crate::fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER,
    DENIED_SUBJECT, backup_dir, journal_file, open_pg, open_sqlite, scratch, unique,
};
use crate::support::{Readback, readback, subject_key_destroyed};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::ErrorCode;
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
use std::process::{Output, Stdio};

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
        (PEER_BANK, PEER_ITEM, DENIED_SUBJECT, "marker-peer-cross-path"),
        (BANK, CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER),
    ] {
        store
            .create_memory_item(&item_in(bank, id, subject, gist), "harness")
            .expect("write the synthetic item");
    }
}

/// The paths one command run needs, plus the one shared deployment key file.
pub struct Deployment {
    pub journal: PathBuf,
    pub keys: PathBuf,
    pub kms: PathBuf,
    pub backups: PathBuf,
    pub db: String,
}

impl Deployment {
    /// Run one `clio` command against this deployment's store, capturing both
    /// streams so a refusal's envelope is readable.
    fn run(&self, args: &[&str]) -> Output {
        clio_command(&self.journal, &self.kms, &self.backups)
            .env("CLIO_KEY_FILE", &self.keys)
            .env("CLIO_KEY_FILE_ONLY", "1")
            .args(args)
            .args(["--db", self.db.as_str()])
            .stdin(Stdio::null())
            .output()
            .expect("spawn clio")
    }

    /// Run one confirmed erasure through the real command.
    fn retry(&self, request_id: &str) -> Output {
        let mut args = vec![
            "compliance",
            "request-retry",
            "--request-id",
            request_id,
            "--subject-id",
            DENIED_SUBJECT,
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
            "--output",
            "json",
        ];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }

    /// Run one request verb that only needs the actor/bank tail.
    fn verb(&self, verb: &str, request_id: &str) -> Output {
        let mut args = vec!["compliance", verb, "--request-id", request_id, "--output", "json"];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }

    /// Record the peer's deletion evidence, the disposition that settles its copy.
    fn disposition(&self, request_id: &str, proof_ref: &str) -> Output {
        let mut args = vec![
            "compliance",
            "peer-disposition",
            "--request-id",
            request_id,
            "--peer-id",
            PEER_LABEL,
            "--disposition",
            "removal_evidence",
            "--proof-ref",
            proof_ref,
            "--output",
            "json",
        ];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }
}

/// The privileged actor and scope bank every request verb carries.
const ACTOR_TAIL: [&str; 4] = ["--actor", "harness", "--bank", BANK];

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
    let record = ledger.record(request_id).expect("the record exists").clone();
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

/// T04 all-bank arm: the exact all-bank erasure runs through the command, the
/// erased marker never returns, a stale peer replay is refused, and peer
/// reconciliation plus the peer copy disposition control closure without a repeat.
fn all_bank<S: CrossStore>(deployment: &Deployment, open: impl Fn() -> S) {
    let request_id = unique("all-bank");
    let store = open();
    seed(&store);
    seed_request(&store, &request_id, &[PEER_LABEL], &[PEER_BANK]);
    drop(store);

    // The exact all-bank erasure: the live set is both sharing banks.
    let first = ok_json(&deployment.retry(&request_id));
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

    // The erased marker never returns; the unrelated control still works.
    let store = open();
    let erased = readback(&store, DENIED_ITEM);
    assert!(
        matches!(erased, Readback::Absent | Readback::DestroyedKey | Readback::KeyMissing),
        "the denied item is never readable again: {erased:?}"
    );
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the unrelated control subject still works"
    );
    let replay = store
        .create_memory_item(
            &item_in(BANK, &format!("{PEER_ITEM}-stale"), DENIED_SUBJECT, DENIED_MARKER),
            "harness",
        )
        .expect_err("a stale peer replay never resurrects the erased subject");
    assert_eq!(
        replay.code,
        ErrorCode::ErasedSubject,
        "the replay is refused for the erased subject: {replay}"
    );
    drop(store);

    // The frozen peer stays pending until it is reconciled.
    let status = ok_json(&deployment.verb("request-status", &request_id));
    assert_eq!(
        status["peers"][PEER_LABEL],
        json!("pending"),
        "the offline peer holds the copy open: {status}"
    );
    ok_json(&deployment.disposition(&request_id, "deletion-1"));

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

/// One SQLite deployment whose store path is also returned for the opener.
fn sqlite_deployment(tag: &str) -> (crate::common::Scratch, Deployment) {
    let scratch = scratch(tag);
    let kms = scratch.join("clio.kms.json");
    let deployment = Deployment {
        journal: journal_file(&scratch, "journal/erasure.json"),
        keys: install_key(scratch.join("clio.key")),
        kms,
        backups: backup_dir(&scratch),
        db: scratch.join("clio.db").to_string_lossy().into_owned(),
    };
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
    let kms = scratch.join("clio.kms.json");
    let db = schema.url().to_owned();
    let deployment = Deployment {
        journal: journal_file(&scratch, "journal/erasure.json"),
        keys: install_key(scratch.join("clio.key")),
        kms: kms.clone(),
        backups: backup_dir(&scratch),
        db: db.clone(),
    };
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
    let kms = scratch.join("clio.kms.json");
    let db = schema.url().to_owned();
    let deployment = Deployment {
        journal: journal_file(&scratch, "journal/erasure.json"),
        keys: install_key(scratch.join("clio.key")),
        kms: kms.clone(),
        backups: backup_dir(&scratch),
        db: db.clone(),
    };
    partial_bank(&deployment, || open_pg(&db, &kms));
}
