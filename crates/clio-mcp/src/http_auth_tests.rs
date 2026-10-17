// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Credential-edge tests: the fail-closed store load, digest resolution, and
//! the request check's `401` matrix.
//!
//! Test-only. Never compiled into production builds.
//!
//! # Fixtures
//!
//! Secrets here are generated per test, never pinned, and the store files live
//! under one scratch directory per test that removes itself. Nothing in this
//! file reads the host environment, and no real credential appears in it.

use super::*;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex, save_auth_tokens};
use clio_types::{Actor, ErrorCode};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so no two tests share a scratch name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One scratch directory per call, removed on drop of the guard below.
///
/// `tempfile` is not a workspace dependency, so this is the repo's own fixture
/// pattern: a unique path under the OS temp directory plus an explicit cleanup
/// at the end of the test. The pid and a monotonic counter keep parallel tests
/// apart; the nanos keep two runs apart.
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
            "am-mcp-auth-{name}-{}-{nanos}-{sequence}.json",
            std::process::id()
        ));
        Self { path }
    }

    /// Write a store the loader will accept: owner-only, since the loader
    /// refuses a file another user can read before it parses anything.
    fn write(&self, records: &[AuthTokenRecord]) {
        save_auth_tokens(&self.path, records).expect("save the store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }

    fn write_raw(&self, text: &str) {
        fs::write(&self.path, text).expect("write raw");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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

#[test]
fn a_loaded_record_resolves_its_own_secret_to_its_own_principal() {
    let scratch = Scratch::new("resolve");
    let secret = secret("a");
    scratch.write(&[record("ci", &secret, Actor::Harness, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    let principal = store.resolve(&secret).expect("resolves");
    assert_eq!(principal.label, "ci");
    assert_eq!(principal.actor, Actor::Harness);
    assert_eq!(principal.banks, vec!["demo".to_owned()]);
}

#[test]
fn an_unknown_secret_resolves_to_nothing() {
    let scratch = Scratch::new("unknown");
    let known = secret("known");
    scratch.write(&[record("ci", &known, Actor::Harness, &["demo"])]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    assert!(store.resolve(&secret("other")).is_none());
    assert!(store.resolve("").is_none());
    // A prefix of a real secret is not a secret.
    assert!(store.resolve(&known[..known.len() - 1]).is_none());
}

#[test]
fn each_record_keeps_its_own_actor_and_banks() {
    let scratch = Scratch::new("multi");
    let harness_secret = secret("h");
    let agent_secret = secret("g");
    scratch.write(&[
        record("ci", &harness_secret, Actor::Harness, &["demo", "logs"]),
        record("bot", &agent_secret, Actor::Agent, &["scratch"]),
    ]);
    let store = CredentialStore::load(scratch.path()).expect("load");

    let harness = store.resolve(&harness_secret).expect("harness resolves");
    let agent = store.resolve(&agent_secret).expect("agent resolves");
    assert_eq!(harness.actor, Actor::Harness);
    assert!(harness.authorize_bank("logs").is_ok());
    assert_eq!(agent.actor, Actor::Agent);
    assert!(agent.authorize_bank("scratch").is_ok());
    assert!(
        agent.authorize_bank("logs").is_err(),
        "banks do not leak between records"
    );
}

#[test]
fn resolution_never_returns_a_secret_or_a_digest() {
    // The returned principal is what a caller may learn; the secret and the
    // digest are not in it, so a downstream log cannot leak either.
    let scratch = Scratch::new("nosecret");
    let secret = secret("s");
    let record = record("ci", &secret, Actor::Harness, &["demo"]);
    scratch.write(std::slice::from_ref(&record));
    let store = CredentialStore::load(scratch.path()).expect("load");

    let principal = store.resolve(&secret).expect("resolves");
    let rendered = format!("{principal:?}");
    assert!(!rendered.contains(&secret), "{rendered}");
    assert!(!rendered.contains(&record.secret_sha256), "{rendered}");
}

#[test]
fn an_absent_store_path_is_refused_rather_than_read_as_empty() {
    let scratch = Scratch::new("absent");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("does not exist"), "{}", err.message);
    // The message names the path and the fix; it names no stored value.
    assert!(err.message.contains("auth token add"), "{}", err.message);
}

#[test]
fn a_malformed_store_is_refused() {
    let scratch = Scratch::new("malformed");
    scratch.write_raw("not json at all");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn an_empty_file_is_refused() {
    let scratch = Scratch::new("empty");
    scratch.write_raw("");
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

#[test]
fn an_empty_store_loads_but_authorizes_nobody() {
    let scratch = Scratch::new("norecords");
    scratch.write(&[]);
    let store = CredentialStore::load(scratch.path()).expect("load");
    assert!(
        store.is_empty(),
        "an empty record file loads no credentials"
    );
    assert!(store.resolve(&secret("any")).is_none());
}

#[test]
#[cfg(unix)]
fn a_world_readable_store_is_refused_before_it_is_parsed() {
    let scratch = Scratch::new("loose");
    scratch.write_raw("[]");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o644)).expect("loosen");
    }
    let err = CredentialStore::load(scratch.path()).expect_err("refused");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("owner-only"), "{}", err.message);
}

#[test]
fn no_credential_source_means_the_unbound_dev_default() {
    let outcome = authorize_request(None, None).expect("unbound");
    assert!(matches!(outcome, AuthOutcome::Unbound));
    // Even with a header present: with nothing to compare against there is no
    // credential, so the answer is the same.
    let with_header = authorize_request(None, Some("Bearer anything")).expect("unbound");
    assert!(matches!(with_header, AuthOutcome::Unbound));
}

#[test]
fn a_matching_record_secret_yields_that_record_s_principal() {
    let secret = secret("m");
    let store =
        CredentialStore::from_records(vec![record("ci", &secret, Actor::Harness, &["demo"])]);
    let outcome =
        authorize_request(Some(&store), Some(&format!("Bearer {secret}"))).expect("bound");
    match outcome {
        AuthOutcome::Bound(principal) => {
            assert_eq!(principal.label, "ci");
            assert_eq!(principal.actor, Actor::Harness);
        }
        AuthOutcome::Unbound => panic!("a matching credential must be bound"),
    }
}

#[test]
fn a_missing_or_malformed_header_is_401_when_a_store_is_configured() {
    let secret = secret("m");
    let store =
        CredentialStore::from_records(vec![record("ci", &secret, Actor::Harness, &["demo"])]);
    for header in [
        None,
        Some(""),
        Some("not-a-scheme"),
        Some("Basic dXNlcjpwYXNz"),
        Some("Bearer "),
        Some(&format!("bearer {secret}")),
    ] {
        assert_eq!(
            authorize_request(Some(&store), header).unwrap_err(),
            401,
            "header {header:?} must be 401"
        );
    }
}

#[test]
fn an_unknown_secret_is_401_and_the_answer_matches_a_missing_one() {
    let secret = secret("m");
    let store =
        CredentialStore::from_records(vec![record("ci", &secret, Actor::Harness, &["demo"])]);
    let unknown = authorize_request(Some(&store), Some("Bearer not-the-secret")).unwrap_err();
    let missing = authorize_request(Some(&store), None).unwrap_err();
    // Identical status and no body difference: a refusal discloses nothing
    // about whether a credential exists.
    assert_eq!(unknown, missing);
    assert_eq!(unknown, 401);
}

#[test]
fn an_empty_store_is_not_a_credential_source_and_authorizes_nobody() {
    // An empty store is the same as no store at the credential edge: both are
    // the unbound answer. That is also what keeps a non-loopback bind with an
    // empty store failing closed at start rather than serving 401-only.
    let empty = CredentialStore::default();
    assert!(matches!(
        authorize_request(Some(&empty), None).expect("unbound"),
        AuthOutcome::Unbound
    ));
    assert!(matches!(
        authorize_request(Some(&empty), Some("Bearer anything")).expect("unbound"),
        AuthOutcome::Unbound
    ));
}

#[test]
fn a_secret_in_a_query_string_never_resolves() {
    // The token belongs in the header. A query string carrying the same secret
    // must not resolve, because the header is the only place a secret is read.
    let secret = secret("q");
    let store =
        CredentialStore::from_records(vec![record("ci", &secret, Actor::Harness, &["demo"])]);
    assert!(authorize_request(Some(&store), Some(&format!("?token={secret}"))).is_err());
}

#[test]
fn a_local_owner_store_resolves_its_secret_to_the_owning_principal() {
    let secret = secret("owner");
    let store = CredentialStore::for_local_owner(&secret, vec!["demo".into(), "shared".into()]);

    let principal = store.resolve(&secret).expect("the minted secret resolves");
    assert_eq!(principal.label, "local-owner");
    assert_eq!(principal.actor, Actor::System, "the local owner is administrative");
    assert_eq!(principal.banks, vec!["demo".to_owned(), "shared".to_owned()]);
    assert!(principal.authorize_bank("demo").is_ok());
    assert!(
        principal.authorize_bank("other").is_err(),
        "the minted list is not widened past what it names"
    );
}

#[test]
fn a_local_owner_store_refuses_every_other_secret() {
    let owner_secret = secret("owner");
    let store = CredentialStore::for_local_owner(&owner_secret, vec!["demo".into()]);

    assert!(!store.is_empty(), "the minted store holds its one record");
    assert!(store.resolve(&secret("other")).is_none());
    assert!(store.resolve("").is_none());
    assert!(store.resolve(&owner_secret[..owner_secret.len() - 1]).is_none());
}
