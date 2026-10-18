// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that each incomplete or refused state
//! closes nothing unsafe: an unknown family link, an open copy action, a changed
//! request scope, a damaged journal, and an offline local peer each return their
//! named state instead of a full closure or an unsafe restore.
//!
//! ## Owns
//! - The five incomplete or refused states and their named returns
//! - The assertions that no full closure and no unsafe restore follows
//! - The concurrent two-local-peer closure refusal on both backends
//!
//! ## Does not own
//! - The request ledger and journal state model (see `clio-compliance`)
//! - The CLI driver and the Postgres gate (see `cli` and `support_pg`)
//!
//! ## Boundary
//!
//! Changes to which incomplete states this claim proves should normally require
//! changes here. Changes to the request ledger should not.
//!
//! Keep this module focused on the refused-and-incomplete claim.

use crate::cli;
use crate::fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER,
    DENIED_SUBJECT, backup_dir, journal_file, open_pg, open_sqlite, scratch, seed_pg, seed_sqlite,
    write_item,
};
use crate::peer_support::{Peer, live_then_offline};
use crate::scenario_all_bank_peer::install_key;
use crate::support::{Readback, archive_sqlite_with_authority, readback};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::{DurableRequestStore, PeerActionStore, ScopedSchema, Store};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{
    JournalState, PathOwner, PathSlot, PathState, PeerEnablement, RegisterOutcome, RequestCommand,
    RequestKind, RequestScope, SubjectRequestLedger, initialize_new_empty, load_journal,
    snapshot_peers,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

const DENIED: [(&str, &str, &str); 1] = [(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)];
const CONTROL: [(&str, &str, &str); 1] = [(CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER)];
const DAMAGED: &[u8] = b"{\"schema_version\":1,\"deployment_id\":\"truncated\"}";
/// The store surface the two-peer arm drives on either backend.
trait PeerStore: Store + DurableRequestStore + PeerActionStore {}
impl<T: Store + DurableRequestStore + PeerActionStore> PeerStore for T {}

struct Env {
    scratch: crate::common::Scratch,
    deployment: PathBuf,
    kms: PathBuf,
    journal: PathBuf,
    backups: PathBuf,
    db: String,
    key: Option<PathBuf>,
}
impl Env {
    fn new(tag: &str) -> Self {
        let scratch = scratch(tag);
        Self {
            deployment: scratch.join("no-such-deployment.json"),
            kms: scratch.join("clio.kms.json"),
            journal: journal_file(&scratch, "journal/erasure-journal.json"),
            backups: backup_dir(&scratch),
            db: scratch.join("clio.db").to_string_lossy().into_owned(),
            key: None,
            scratch,
        }
    }

    fn db(&self) -> &str {
        &self.db
    }

    fn path(&self) -> &Path {
        Path::new(&self.db)
    }

    fn spawn(&self, kms: &Path, args: &[&str]) -> Output {
        let mut cmd = cli::clio_command(&self.journal, kms, &self.backups);
        cmd.env("CLIO_DEPLOYMENT_CONFIG", &self.deployment);
        if let Some(key) = &self.key {
            cmd.env("CLIO_KEY_FILE", key).env("CLIO_KEY_FILE_ONLY", "1");
        }
        cmd.args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run clio")
    }

    fn command(&self, sub: &str, rest: &str) -> Output {
        let line = format!("compliance {sub} {rest} --db {} -o json", self.db());
        let args: Vec<&str> = line.split_whitespace().collect();
        self.spawn(&self.kms, &args)
    }

    fn value(&self, sub: &str, rest: &str) -> Value {
        stdout_of(&self.command(sub, rest))
    }

    fn start(&self, request: &str, subject: &str) -> Output {
        let rest = format!("--request-id {request} --kind erasure --subject-id {subject}");
        self.command("request-start", &rest)
    }

    fn status(&self, request: &str) -> Value {
        self.value("request-status", &format!("--request-id {request}"))
    }

    fn peer_status(&self, request: &str) -> Value {
        self.value("peer-status", &format!("--request-id {request}"))
    }

    fn inventory(&self) -> Value {
        self.value("inventory", "")
    }

    fn restore(&self, kms: &Path, id: &str) -> Output {
        let line = format!("backup restore {id} --yes --db {} -o json", self.db());
        let args: Vec<&str> = line.split_whitespace().collect();
        self.spawn(kms, &args)
    }
}
fn stdout_of(out: &Output) -> Value {
    let bytes = &out.stdout;
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): out={} err={}",
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
fn ok(out: &Output, what: &str) {
    let (out_text, err_text) = streams(out);
    assert!(out.status.success(), "{what} failed: {out_text}{err_text}");
}
fn streams(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
fn envelope(out: &Output) -> Value {
    serde_json::from_slice(&out.stderr).unwrap_or_else(|error| panic!("refusal not JSON: {error}"))
}
fn assert_forbidden(out: &Output) {
    let (_, stderr) = streams(out);
    assert_eq!(out.status.code(), Some(1), "must refuse (exit 1): {stderr}");
    assert_eq!(
        envelope(out)["code"],
        json!("forbidden"),
        "must be forbidden"
    );
}
fn assert_refused_exit2(out: &Output, needle: &str) {
    let (_, stderr) = streams(out);
    assert_eq!(out.status.code(), Some(2), "must refuse (exit 2): {stderr}");
    assert!(stderr.contains(needle), "the refusal must name {needle:?}");
}
/// The open copy action's own refusal, the named state the closure gate reports.
fn assert_open_copy(out: &Output) {
    let (_, stderr) = streams(out);
    assert!(
        stderr.contains("OpenCopyAction"),
        "the named action is open"
    );
}
fn seed_done_path(store: &dyn DurableRequestStore, request_id: &str, path: &str) {
    let row = store.load(request_id).expect("load").expect("the row");
    let mut record = row_to_record(&row).expect("decode the request row");
    let done = PathSlot {
        owner: PathOwner::Clio,
        state: PathState::Done,
    };
    record.paths.insert(path.to_owned(), done);
    store.save(&record_to_row(&record)).expect("save the row");
}
/// The changed-scope, open-copy, and unknown-family-link refusals: the request
/// closes nothing and its stored state stays open.
fn refuse_scope_copy_close(env: &Env, request: &str, store: &dyn DurableRequestStore) {
    ok(&env.start(request, DENIED_SUBJECT), "request-start");
    assert_forbidden(&env.start(request, CONTROL_SUBJECT));
    assert_eq!(env.status(request)["state"], json!("InProgress"), "open");
    seed_done_path(store, request, "erasure");
    let copy = env.command("request-copy", &copy_rest(request));
    ok(&copy, "request-copy");
    let close = env.command("request-close", &close_rest(request));
    assert_forbidden(&close);
    assert_open_copy(&close);
    assert_eq!(env.status(request)["request_outcome"], json!("open"));
    let inventory = env.inventory();
    assert_eq!(inventory["complete"], json!(false), "incomplete scope");
    let rows = inventory["rows"].as_array().expect("the inventory rows");
    let unknown = rows.iter().any(|row| row["subject_link"].is_null());
    assert!(unknown, "one family link stays open and unknown");
}
fn copy_rest(request: &str) -> String {
    format!(
        "--actor harness --copy-id copy-{request} --request-id {request} --custody caller \
         --owner harness --required-action erase --state pending"
    )
}
fn close_rest(request: &str) -> String {
    format!("--request-id {request} --product-path erasure")
}
fn register_required_peers<S: DurableRequestStore + PeerActionStore>(
    store: &S,
    request: &str,
    peers: &[&str],
    bank: &str,
) {
    let mut ledger = SubjectRequestLedger::new();
    let scope = RequestScope::Subject {
        bank_id: BANK.to_owned(),
        subject_id: DENIED_SUBJECT.to_owned(),
    };
    let outcome = ledger
        .register_with_enablement(
            request,
            RequestKind::Erasure,
            &scope,
            PeerEnablement::Required,
            peers,
            &[bank],
        )
        .expect("register");
    assert_eq!(outcome, RegisterOutcome::Opened, "fresh opens");
    ledger
        .apply(request, &RequestCommand::Begin)
        .expect("start");
    let record = ledger.record(request).expect("record").clone();
    store.save(&record_to_row(&record)).expect("persist");
    snapshot_peers(store, &record).expect("snapshot peers");
}
/// T04 concurrent two-local-peer arm: two live serve peers both hold the bank
/// across the erasure, so closure waits for both, and the erased copy never
/// returns while both peers are present.
fn two_live_peers<S: PeerStore>(env: &Env, request: &str, open: impl Fn() -> S) {
    {
        let store = open();
        write_item(&store, DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER);
        write_item(&store, CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER);
        register_required_peers(&store, request, &["peer-a", "peer-b"], BANK);
    }
    let _a = Peer::start(
        &env.scratch.join("peer-a"),
        &env.journal,
        &env.backups,
        &[BANK],
    );
    let _b = Peer::start(
        &env.scratch.join("peer-b"),
        &env.journal,
        &env.backups,
        &[BANK],
    );
    let tail = format!(
        "--request-id {request} --subject-id {DENIED_SUBJECT} --legal-basis GDPR_ART_17 \
         --confirm --actor harness --bank {BANK}"
    );
    let first = env.value("request-retry", &tail);
    assert_eq!(first["technical_completion"], json!("pending"), "{first}");
    assert!(
        first["pending_operator_actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()),
        "{first}"
    );
    let peers = env.peer_status(request);
    assert_eq!(peers["peer-a"], json!("pending"), "peer a holds a copy");
    assert_eq!(peers["peer-b"], json!("pending"), "peer b holds a copy");
    let store = open();
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "control"
    );
    assert_ne!(readback(&store, DENIED_ITEM), Readback::Readable, "denied");
}
#[test]
fn scenario_refusals_incomplete_peers_disabled_sqlite() {
    let env = Env::new("refusals-core");
    seed_sqlite(env.path(), &env.kms, &DENIED);
    seed_sqlite(env.path(), &env.kms, &CONTROL);
    let store = open_sqlite(env.path(), &env.kms);
    refuse_scope_copy_close(&env, "req-core", &store);
    // A damaged journal refuses the restore route before any placement.
    let sdb = env.scratch.join("source/clio.db");
    let skms = env.scratch.join("source/clio.kms.json");
    seed_sqlite(&sdb, &skms, &DENIED);
    initialize_new_empty(&env.journal).expect("initialize the journal");
    let auth = Some(current_authority(&env.journal));
    let a = archive_sqlite_with_authority(&env.backups, &sdb, &skms, "dmg", auth);
    let tdb = env.scratch.join("target/clio.db");
    let tkms = env.scratch.join("target/clio.kms.json");
    seed_sqlite(&tdb, &tkms, &CONTROL);
    std::fs::write(&env.journal, DAMAGED).expect("plant the damaged journal");
    assert_refused_exit2(&env.restore(&tkms, &a.id), "journal is damaged");
    let target = open_sqlite(&tdb, &tkms);
    assert_eq!(
        readback(&target, CONTROL_ITEM),
        Readback::Readable,
        "control readable"
    );
    assert_eq!(
        readback(&target, DENIED_ITEM),
        Readback::Absent,
        "no unsafe restore"
    );

    let peers = env.peer_status("req-core");
    let no_peer = peers
        .as_object()
        .expect("peer object")
        .keys()
        .all(|k| k.as_str() == "ok");
    assert!(no_peer, "a peers-disabled deployment names no peer");
}
#[test]
fn scenario_refusals_incomplete_peers_disabled_postgres() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let env = Env {
        db: target.url().to_owned(),
        ..Env::new("refusals-core-pg")
    };
    seed_pg(env.db(), &env.kms, &DENIED);
    seed_pg(env.db(), &env.kms, &CONTROL);
    let store = open_pg(env.db(), &env.kms);
    refuse_scope_copy_close(&env, "req-core-pg", &store);
    let peers = env.peer_status("req-core-pg");
    let no_peer = peers
        .as_object()
        .expect("peer object")
        .keys()
        .all(|k| k.as_str() == "ok");
    assert!(no_peer, "a peers-disabled deployment names no peer");
}
fn current_authority(journal: &Path) -> ManifestAuthority {
    let JournalState::Current(j) = load_journal(journal) else {
        panic!("expected a current journal");
    };
    ManifestAuthority {
        deployment_id: j.deployment_id,
        generation: j.generation,
        salt_fingerprint: j.salt_fingerprint,
    }
}
/// The offline required peer stays pending, and the request closes nothing.
#[test]
fn scenario_refusals_incomplete_offline_peer() {
    let env = Env::new("refusals-peer");
    seed_sqlite(env.path(), &env.kms, &DENIED);
    let request = "req-offline-peer";
    let peer_label = "peer-offline-a";
    {
        let store = open_sqlite(env.path(), &env.kms);
        register_required_peers(&store, request, &[peer_label], BANK);
    }

    // A real local peer process, spawned and bound, then taken offline: the
    // offline window. The live peer answers the wire; once offline it refuses
    // it. No CLI verb enables a peer, so the frozen set is registered here over
    // the same durable store the CLI reads.
    let mut live = Peer::start(
        &env.scratch.join("peer"),
        &env.journal,
        &env.backups,
        &[BANK],
    );
    live_then_offline(&mut live, &env.scratch, BANK);
    assert_eq!(
        env.peer_status(request)[peer_label],
        json!("pending"),
        "peer stays pending"
    );

    let store = open_sqlite(env.path(), &env.kms);
    seed_done_path(&store, request, "erasure");
    let close = env.command("request-close", &close_rest(request));
    assert_forbidden(&close);
    assert_open_copy(&close);

    ok(
        &env.command("peer-retry", &format!("--request-id {request}")),
        "peer-retry",
    );
    assert_eq!(
        env.peer_status(request)[peer_label],
        json!("pending"),
        "still pending"
    );
}
/// The same refused request states on loopback Postgres.
#[test]
fn scenario_refusals_incomplete_postgres() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let env = Env {
        db: target.url().to_owned(),
        ..Env::new("refusals-pg")
    };
    ok(&env.start("req-pg", DENIED_SUBJECT), "request-start");
    assert_forbidden(&env.start("req-pg", CONTROL_SUBJECT));
    assert_eq!(
        env.inventory()["complete"],
        json!(false),
        "the scope is incomplete"
    );
}
/// The concurrent two-local-peer closure refusal on SQLite.
#[test]
fn scenario_refusals_incomplete_two_live_peers_sqlite() {
    let mut env = Env::new("refusals-two-peer");
    env.key = Some(install_key(env.scratch.join("clio.key")));
    two_live_peers(&env, "req-two-peer", || open_sqlite(env.path(), &env.kms));
}
/// The concurrent two-local-peer closure refusal on loopback Postgres.
#[test]
fn scenario_refusals_incomplete_two_live_peers_postgres() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let mut env = Env {
        db: target.url().to_owned(),
        ..Env::new("refusals-two-peer-pg")
    };
    env.key = Some(install_key(env.scratch.join("clio.key")));
    two_live_peers(&env, "req-two-peer-pg", || open_pg(env.db(), &env.kms));
}
