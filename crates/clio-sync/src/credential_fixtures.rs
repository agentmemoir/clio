// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test fixtures shared by the sync
//! credential suites: a scratch store file that removes itself, record
//! builders, and generated (never pinned) secrets.
//!
//! ## Owns
//! - `Scratch`: one unique store path per call, removed on drop
//! - `record` / `secret`: the one record shape and the one secret generator
//!   the credential tests build from
//! - `scoped_store`: one scratch store holding one record, the shape the
//!   server test helpers hand to `sync_serve`
//!
//! ## Does not own
//! - What a store or a secret must satisfy (see `clio_config::auth_tokens`)
//! - Any assertion; this module holds no test
//!
//! ## Boundary
//!
//! Changes to the fixture shape should normally require changes here. Changes
//! to production credential behavior should not.
//!
//! Keep this module focused on the shared credential-store fixtures.
//!
//! # Fixtures
//!
//! `tempfile` is not a workspace dependency, so this is the repo's own fixture
//! pattern: a unique path under the OS temp directory plus an explicit cleanup
//! on drop. The pid and a monotonic counter keep parallel tests apart; the
//! nanos keep two runs apart. No test reads the host environment, and no real
//! credential appears here.

use clio_config::auth_tokens::{AuthTokenRecord, digest_hex, save_auth_tokens};
use clio_types::Actor;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Store files written so far in this test binary, so no two tests share one.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One scratch store file, removed on drop of the guard.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A unique scratch path for `name`; nothing is written yet.
    pub(crate) fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "am-sync-cred-{name}-{}-{nanos}-{sequence}.json",
            std::process::id()
        ));
        Self { path }
    }

    /// Write a store the loader will accept: owner-only, since the loader
    /// refuses a file another user can read before it parses anything. The
    /// saver already writes owner-only; the mode is set again so the fixture
    /// does not depend on that remaining true.
    pub(crate) fn write(&self, records: &[AuthTokenRecord]) {
        save_auth_tokens(&self.path, records).expect("save the store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }

    /// Write raw text (malformed or empty fixtures) owner-only.
    pub(crate) fn write_raw(&self, text: &str) {
        fs::write(&self.path, text).expect("write raw");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }

    /// The store path, written or not.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One valid record binding a secret to a bank list.
pub(crate) fn record(label: &str, secret: &str, banks: &[&str]) -> AuthTokenRecord {
    AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: digest_hex(secret),
        actor: Actor::Agent,
        banks: banks.iter().map(|b| (*b).to_owned()).collect(),
        created_at: "2026-10-01T00:00:00Z".into(),
    }
}

/// A scratch store holding one record that binds `secret` to `banks`.
///
/// The server test helpers need a file to hand `sync_serve`; a `Scratch` plus
/// one `record` is exactly that, so no helper repeats either step. The guard
/// owns the file: keep it alive until the server has loaded the store.
pub(crate) fn scoped_store(name: &str, secret: &str, banks: &[&str]) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.write(&[record("scoped-client", secret, banks)]);
    scratch
}

/// A secret the test generates itself, so no fixture pins one.
pub(crate) fn secret(tag: &str) -> String {
    clio_config::auth_tokens::generate_secret()
        .chars()
        .take(8)
        .collect::<String>()
        + tag
}
