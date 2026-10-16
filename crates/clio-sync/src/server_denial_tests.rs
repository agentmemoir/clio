// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync server's denial surface over real TCP: the pull
//! refusal for a bank a recorded denial covers while an unrelated bank still
//! serves, the refusal of a pull whose authority is unreadable, the refusal a
//! pull racing a recording denial observes, the durable proof a push
//! acknowledgement carries for a denial operation, its redelivery, and the
//! refusal a server still reports after the store and the authority are
//! reopened.
//!
//! ## Owns
//! - Behavior coverage for the pull-side bank guard (including the bank lock
//!   its check and read hold) and the push-side proof
//! - The push-during-pull race coverage the pausing fixture drives, and the
//!   scratch directory the durability case creates
//!
//! ## Does not own
//! - The apply-side guards at the engine level (`apply_denial_tests`)
//! - The wire vocabulary in isolation (`denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when the routes' denial behavior changes; they never
//! change it.
//!
//! Keep this module focused on the server's denial surface.

use crate::credential_fixtures::{Scratch, record, secret};
use crate::denial::{
    DenialCarrier, DenialProof, DenialProofBinding, denial_mutation, denied_bank_marker,
};
use crate::denial_fixtures::{FakeDenial, FileDenial, PausingDenial};
use crate::protocol::{SyncContent, SyncMutation};
use crate::server::server_tests::{http, item_value, mutation, push_body, sqlite, wait_closed};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{DekProvider, ErasureDenial, LocalDevKms, SqliteStore, Store, SyncStore};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// Server options over `store` and the credential store at `store_path`.
fn options(
    store: &Arc<SqliteStore>,
    kms: Arc<dyn DekProvider>,
    store_path: &Path,
) -> SyncServerOptions {
    SyncServerOptions {
        bind: "127.0.0.1:0".into(),
        store: Arc::clone(store) as Arc<dyn Store>,
        sync: Arc::clone(store) as Arc<dyn SyncStore>,
        kms,
        banks: vec!["b-a".into(), "b-b".into(), "b-c".into()],
        token_file: Some(store_path.to_path_buf()),
        dev_mode: false,
        ttl_secs: None,
        deadline_secs: None,
        device_id: "srv-1".into(),
    }
}

/// A scoped credential for all three banks, and the token it presents.
fn credential() -> (Scratch, String) {
    let scratch = Scratch::new("denial-serve");
    let token = secret("t");
    scratch.write(&[record("client-a", &token, &["b-a", "b-b", "b-c"])]);
    (scratch, token)
}

/// A running server with the authority wired, its store, the authority, and
/// the credential the tests present.
fn serve_denied() -> (
    SyncServer,
    Arc<SqliteStore>,
    Arc<FakeDenial>,
    Scratch,
    String,
) {
    let (scratch, token) = credential();
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let server = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, store, denial, scratch, token)
}

/// Ceiling on a signal that must arrive (the recording push reaching its
/// marker record). Generous: only a hung server can miss it.
const SIGNAL_BUDGET: Duration = Duration::from_secs(10);
/// Ceiling on the pull's guard signal while the recording push holds the bank
/// lock. On the locked server this signal can never arrive before the release,
/// so the bound is what turns "the pull waits on the lock" into a finite wait;
/// a server that checks without the lock signals well inside it.
const BLOCKED_BUDGET: Duration = Duration::from_secs(2);

/// A scratch directory for the durability case's store and journal, removed on
/// drop: the repo's temp-file fixture pattern one level up, so the SQLite
/// sidecars the file leaves beside itself go with it.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "am-sync-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self { path }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// One push of `mutations` under `bank`.
fn push(addr: &str, token: &str, bank: &str, mutations: &[SyncMutation]) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/push",
        Some(token),
        Some(&push_body(bank, mutations)),
    )
}

/// One pull of `bank`.
fn pull(addr: &str, token: &str, bank: &str) -> (u16, Value) {
    http(
        addr,
        "POST",
        "/sync/pull",
        Some(token),
        Some(&json!({ "bank": bank, "cursor": "", "limit": 10 })),
    )
}

/// The one denial operation a request sends, covering `banks` and riding `bank`.
fn denial_op(bank: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        "s-held",
        &DenialCarrier::new("req-1", banks),
        "dev-a",
        "2026-01-01T00:00:00Z",
    )
}

#[test]
fn a_denied_bank_is_refused_while_an_unrelated_bank_serves() {
    let (server, _store, _denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    // One row per bank, pushed before the denial. Item ids are store-wide, so
    // each bank gets its own.
    for (bank, item, mid) in [("b-a", "i-a", "m-a"), ("b-c", "i-c", "m-c")] {
        let mut m = mutation(mid, bank, "2026-01-01T00:00:00Z", "g");
        m.entity_id = item.into();
        m.content = SyncContent::Plain(item_value(bank, item, "2026-01-01T00:00:00Z", "g"));
        let (status, value) = push(&addr, &token, bank, &[m]);
        assert_eq!(status, 200, "push {bank}: {value}");
    }
    let (status, ack) = push(&addr, &token, "b-a", &[denial_op("b-a", &["b-a", "b-b"])]);
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["accepted"], json!(["denial:req-1"]));

    let (status, value) = pull(&addr, &token, "b-a");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");
    let (status, value) = pull(&addr, &token, "b-b");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");

    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 200, "{value}");
    assert_eq!(
        value["mutations"].as_array().expect("mutations").len(),
        1,
        "an unrelated bank's rows are served: {value}"
    );
    server.stop();
}

#[test]
fn an_unreadable_authority_refuses_the_pull_for_an_unrelated_bank() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 200, "{value}");
    denial.damage();
    let (status, value) = pull(&addr, &token, "b-c");
    assert_eq!(status, 500, "{value}");
    assert_eq!(value["code"], "config_corrupt");
    server.stop();
}

#[test]
fn a_push_acknowledges_a_denial_with_its_durable_proof() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", &["b-a", "b-b"]);
    let (status, ack) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    assert_eq!(status, 200, "{ack}");
    let acks = ack["denial_acks"].as_array().expect("denial acks");
    assert_eq!(acks.len(), 1, "{ack}");
    assert_eq!(acks[0]["sync_mutation_id"], "denial:req-1");
    let proof: DenialProof =
        serde_json::from_value(acks[0]["proof"].clone()).expect("proof parses");
    assert_eq!(proof.request_ref, "req-1");
    assert_eq!(proof.peer_id, "srv-1");
    assert_eq!(proof.generation, denial.generation());
    assert_eq!(denial.records_for("s-held"), 1, "one durable record");

    // The proof verifies against this deployment's own fingerprint and binds
    // the request, the peer, and the subject; a proof answered under another
    // fingerprint is refused rather than acknowledged.
    let carrier = DenialCarrier::new("req-1", &["b-a", "b-b"]);
    let binding = DenialProofBinding::for_carrier(&op, &carrier, "srv-1", denial.fingerprint());
    assert_eq!(proof.verify(&binding), Ok(()));
    let elsewhere = DenialProofBinding::for_carrier(&op, &carrier, "srv-1", "fp-other");
    assert!(proof.verify(&elsewhere).is_err());
    server.stop();
}

#[test]
fn a_redelivered_denial_push_repeats_no_record_and_answers_the_current_generation() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let addr = server.addr().to_owned();
    let op = denial_op("b-a", &["b-a"]);
    let (_, first) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    let (_, second) = push(&addr, &token, "b-a", std::slice::from_ref(&op));
    assert_eq!(second["accepted"], json!(["denial:req-1"]));
    let first_proof: DenialProof =
        serde_json::from_value(first["denial_acks"][0]["proof"].clone()).expect("proof parses");
    let second_proof: DenialProof =
        serde_json::from_value(second["denial_acks"][0]["proof"].clone()).expect("proof parses");
    assert_eq!(second_proof.request_ref, first_proof.request_ref);
    assert_eq!(second_proof.peer_id, first_proof.peer_id);
    assert_eq!(second_proof.subject_hash, first_proof.subject_hash);
    assert_eq!(second_proof.salt_fingerprint, first_proof.salt_fingerprint);
    // The seam's bound for a repeat: no change, and the current generation —
    // never a generation older than the first record's.
    assert!(
        second_proof.generation >= first_proof.generation,
        "a repeat answers the current generation: {second_proof:?} vs {first_proof:?}"
    );
    assert_eq!(
        denial.generation(),
        first_proof.generation,
        "the redelivery recorded nothing, so the generation never moved"
    );
    assert_eq!(denial.records_for("s-held"), 1, "one durable record");
    server.stop();
}

/// A second server object over the same in-memory store and the same authority
/// object still refuses: the refusal is the shared authority's, not either
/// server's own process state. A real reopen is covered below.
#[test]
fn a_second_server_over_the_same_authority_still_refuses_a_denied_bank() {
    let (scratch, token) = credential();
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let first = sync_serve_with_denial(
        options(&store, Arc::clone(&kms), scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let (status, ack) = push(first.addr(), &token, "b-a", &[denial_op("b-a", &["b-a"])]);
    assert_eq!(status, 200, "{ack}");
    first.stop();

    let second = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let (status, value) = pull(second.addr(), &token, "b-a");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");
    second.stop();
}

/// A denial recorded before a restart still refuses after both the store and
/// the authority are reopened from their files: the durable journal record,
/// not any shared process state, carries the refusal.
#[test]
fn a_reopened_store_and_journal_refuse_a_bank_denied_before_the_restart() {
    let scratch = Scratch::new("denial-durable");
    let token = secret("t");
    scratch.write(&[record("client-a", &token, &["b-a"])]);
    let dir = ScratchDir::new("denial-durable");
    let store_path = dir.path.join("store.db");
    let journal = dir.path.join("erasure-journal.json");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());

    {
        let store = Arc::new(SqliteStore::open(&store_path, Arc::clone(&kms)).expect("open"));
        let denial = FileDenial::new(journal.clone(), "fp-1");
        let server = sync_serve_with_denial(
            options(&store, Arc::clone(&kms), scratch.path()),
            Arc::clone(&denial) as Arc<dyn ErasureDenial>,
        )
        .expect("serve");
        let addr = server.addr().to_owned();
        let (status, ack) = push(&addr, &token, "b-a", &[denial_op("b-a", &["b-a"])]);
        assert_eq!(status, 200, "{ack}");
        server.stop();
        wait_closed(&addr, 5, "the first durable server");
    }

    // A second server over a reopened store and a fresh authority object over
    // the same journal file: only the files can answer.
    let store = Arc::new(SqliteStore::open(&store_path, Arc::clone(&kms)).expect("reopen"));
    assert!(
        store.journal_max_seq("b-a").expect("journal") > 0,
        "the reopened store still holds the pre-denial journal row"
    );
    let denial = FileDenial::new(journal, "fp-1");
    let server = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let (status, value) = pull(server.addr(), &token, "b-a");
    assert_eq!(status, 403, "{value}");
    assert_eq!(value["code"], "erased_subject");
    server.stop();
}

/// A pull that races a denial being recorded must be refused, not served. The
/// recording push holds its bank's lock from before its apply, so the pull
/// cannot pass its own guard and read rows while the denial is in flight: it
/// waits on the lock, then refuses the now-covered bank.
#[test]
fn a_pull_racing_a_recording_denial_is_refused_not_served() {
    let (scratch, token) = credential();
    let (store, kms) = sqlite();
    let (denial, entered, guard) =
        PausingDenial::new(FakeDenial::new("fp-1"), &denied_bank_marker("b-a"));
    let server = sync_serve_with_denial(
        options(&store, kms, scratch.path()),
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    let addr = server.addr().to_owned();
    // One pre-denial row, so a pull that slipped past its guard would serve a
    // real page instead of an empty one.
    let (status, value) = push(
        &addr,
        &token,
        "b-a",
        &[mutation("m-1", "b-a", "2026-01-01T00:00:00Z", "g")],
    );
    assert_eq!(status, 200, "{value}");

    let push_addr = addr.clone();
    let push_token = token.clone();
    let push_worker = thread::spawn(move || {
        push(
            &push_addr,
            &push_token,
            "b-a",
            &[denial_op("b-a", &["b-a"])],
        )
    });
    entered
        .recv_timeout(SIGNAL_BUDGET)
        .expect("the recording push reaches the marker record");
    let pull_addr = addr;
    let pull_token = token;
    let pull_worker = thread::spawn(move || pull(&pull_addr, &pull_token, "b-a"));
    // The pull cannot reach its guard while the push holds the bank lock. A
    // server that checked without the lock reaches the guard here, reads the
    // pre-denial page, and answers 200.
    let guard_reached = guard.recv_timeout(BLOCKED_BUDGET).is_ok();
    denial.release();
    let (push_status, ack) = push_worker.join().expect("push thread");
    let (pull_status, page) = pull_worker.join().expect("pull thread");
    assert_eq!(push_status, 200, "{ack}");
    assert!(
        !guard_reached,
        "the pull read a verdict while the recording push held the bank lock"
    );
    assert_eq!(pull_status, 403, "{page}");
    server.stop();
}
