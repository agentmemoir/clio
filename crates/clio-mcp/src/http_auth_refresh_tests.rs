// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Credential-store reload tests: a revoke that lands while the file is being
//! read, a store that never settles, the in-step fast path, and a snapshot
//! another reload publishes first.
//!
//! Test-only. Never compiled into production builds.
//!
//! # Fixtures
//!
//! Secrets here are generated per test, never pinned, and each store file lives
//! at a scratch path that removes itself. Nothing in this file reads the host
//! environment, no test sleeps, and no test depends on another's ordering: the
//! writer that races a reload is placed by `refresh_with`'s seam, so every
//! interleaving below happens in one thread, in a fixed order, every run.

use super::*;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex, save_auth_tokens};
use clio_types::Actor;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Scratch files served so far in this test binary, so no two tests collide.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One store file per test, removed on drop.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "am-mcp-refresh-{name}-{}-{nanos}-{sequence}.json",
            std::process::id()
        ));
        Self { path }
    }

    /// Write a store the loader accepts: one owned by this user alone. The
    /// canonical writer sets that mode itself, so a rewrite keeps it.
    fn write(&self, records: &[AuthTokenRecord]) {
        save_auth_tokens(&self.path, records).expect("save the store");
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(clio_config::auth_tokens::lock_file_path(&self.path));
    }
}

fn record(label: &str, secret: &str, actor: Actor, banks: &[&str]) -> AuthTokenRecord {
    AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: digest_hex(secret),
        actor,
        banks: banks.iter().map(|b| (*b).to_owned()).collect(),
        created_at: "2026-10-01T00:00:00Z".into(),
    }
}

/// A secret the test generates itself, so no fixture pins one.
fn secret(tag: &str) -> String {
    clio_config::auth_tokens::generate_secret()
        .chars()
        .take(8)
        .collect::<String>()
        + tag
}

/// A revoke that lands between the read and the stat after it must not be
/// published: the records that read returned still name the revoked secret,
/// and making them current is what kept it alive.
#[test]
fn a_revoke_that_lands_during_the_read_is_not_published() {
    let scratch = Scratch::new("race-revoke");
    let revoked = secret("t");
    let survivor = secret("s");
    let audit = secret("u");
    scratch.write(&[
        record("ci", &revoked, Actor::Harness, &["demo"]),
        record("bot", &survivor, Actor::Agent, &["scratch"]),
    ]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    // The file moves on before the reload starts, so the reload has real work
    // and the revoked record is still in the version it will read.
    scratch.write(&[
        record("ci", &revoked, Actor::Harness, &["demo"]),
        record("bot", &survivor, Actor::Agent, &["scratch"]),
        record("audit", &audit, Actor::Harness, &["logs"]),
    ]);

    let calls = AtomicUsize::new(0);
    let result = store.refresh_with(|| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            scratch.write(&[
                record("bot", &survivor, Actor::Agent, &["scratch"]),
                record("audit", &audit, Actor::Harness, &["logs"]),
            ]);
        }
    });
    assert!(result.is_ok(), "the reload must settle on the current file");
    assert!(
        store.resolve(&revoked).expect("resolve").is_none(),
        "the revoked secret must not resolve from the snapshot the revoke displaced"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the read that met the revoke must be discarded and the store re-read"
    );
    assert!(
        store.resolve(&survivor).expect("resolve").is_some(),
        "the surviving record must still authorize"
    );
    assert!(
        store.resolve(&audit).expect("resolve").is_some(),
        "the record added before the reload must authorize"
    );
}

/// A store that changes on every read never becomes current: the reload is
/// bounded and refuses rather than publishing a snapshot it could not verify.
#[test]
fn a_store_that_never_settles_is_refused_rather_than_published() {
    let scratch = Scratch::new("never-settles");
    let stale = secret("a");
    scratch.write(&[record("a", &stale, Actor::Harness, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");
    scratch.write(&[record("bb", &secret("b"), Actor::Agent, &["scratch"])]);

    let first = secret("f");
    let calls = AtomicUsize::new(0);
    let result = store.refresh_with(|| {
        let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
        // Every attempt writes a store of a different length, so the version
        // the reload compared against is never the version it now sees.
        let label = "c".repeat(attempt + 1);
        let presented = if attempt == 1 {
            first.clone()
        } else {
            secret(&label)
        };
        scratch.write(&[record(&label, &presented, Actor::Harness, &["demo"])]);
    });
    let error = result.expect_err("a store that never settles must refuse");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        REFRESH_ATTEMPTS,
        "the reload must stop at the attempt bound instead of spinning"
    );
    assert!(store.state().degraded, "the refusal must mark the store");
    assert_eq!(
        authorize_request(Some(&store), Some(&format!("Bearer {first}"))).unwrap_err(),
        401,
        "a secret no version of the file holds must not authorize"
    );
}

/// A store already in step with the file is not re-read: the fast path answers
/// from the fingerprint alone, which is the one request that must stay cheap.
#[test]
fn a_store_in_step_with_the_file_is_not_read_again() {
    let scratch = Scratch::new("in-step");
    let known = secret("k");
    scratch.write(&[record("ci", &known, Actor::Harness, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    let calls = AtomicUsize::new(0);
    store
        .refresh_with(|| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .expect("in step");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "an in-step store must not read the file"
    );
    assert!(store.resolve(&known).expect("resolve").is_some());
}

/// A reload that publishes while another read is in flight must not be
/// overwritten by the reader it displaced: the newest snapshot is the one the
/// store keeps.
#[test]
fn a_snapshot_published_during_the_read_is_not_overwritten() {
    let scratch = Scratch::new("supersede");
    let older = secret("o");
    let newer = secret("n");
    scratch.write(&[record("ci", &older, Actor::Harness, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");
    // The file moves on, so the reload below has work to do.
    scratch.write(&[record("ci", &newer, Actor::Harness, &["demo"])]);

    let calls = AtomicUsize::new(0);
    let result = store.refresh_with(|| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            store
                .refresh_with(|| {})
                .expect("the first reload publishes");
        }
    });
    assert!(
        result.is_ok(),
        "the displaced read must re-evaluate, not fail"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the re-evaluated attempt must find the store already in step"
    );
    assert!(
        store.resolve(&newer).expect("resolve").is_some(),
        "the store must hold the snapshot the file was published from"
    );
    assert!(
        store.resolve(&older).expect("resolve").is_none(),
        "the superseded record must not come back"
    );
}
