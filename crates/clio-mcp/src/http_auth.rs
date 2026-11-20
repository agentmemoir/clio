// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the network transport's credential edge: the
//! load of the token store, its revalidation as the file changes, and the
//! per-request resolution of a presented bearer secret to the principal its
//! record names.
//!
//! ## Owns
//! - `CredentialStore::load`: the fail-closed read of an operator-named store
//! - `CredentialStore::resolve`: the snapshot check and the reload that follows
//!   a changed file
//! - The request-shaped check over one request's `Authorization` header
//! - `AuthOutcome`: the credential that matched, or the unbound dev-mode answer
//!
//! ## Does not own
//! - The identity rule a resolved principal applies (see `principal`)
//! - The record shape, its validation, and the store file itself (see
//!   `clio_config::auth_tokens`)
//! - Origin validation and session lifecycle (see `http_guard`)
//!
//! ## Boundary
//!
//! Changes to how a presented credential becomes a principal should normally
//! require changes here. Changes to what that principal may do should not.
//!
//! Keep this module focused on the credential edge and nothing past it.
//!
//! # The store is revalidated per request
//!
//! A request does not re-read the file, but it does `stat` it. The device,
//! inode, modification time, and length are compared with the values captured
//! when the records were loaded. When they differ, the shared loader runs again
//! and the new records replace the old snapshot, so a secret revoked by the
//! token management verbs stops resolving on the running server. The cost is
//! one `stat` per authorized request.
//!
//! A reload that fails refuses the request rather than serving the previous
//! snapshot: a stale snapshot would keep a revoked secret alive. The reason is
//! reported once, when the store enters that state, not once per request.
//!
//! # Why the scan does not stop at the match
//!
//! A constant-time byte comparison is not enough on its own: returning as soon
//! as a record matches makes a successful credential do less work than an
//! unknown one, and that difference is measurable. So every record's digest is
//! compared and the match accumulates, and the answer is read back afterwards.
//! Both digests are fixed-length lowercase hex, so the length check inside the
//! shared comparison discloses nothing about the secret.

use crate::principal::Principal;
use clio_config::auth_tokens::{AuthTokenRecord, ct_eq, digest_hex, load_auth_tokens};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// One loaded record: the principal it names plus the only secret-derived value
/// that exists on disk.
#[derive(Debug, Clone)]
struct StoredCredential {
    principal: Principal,
    digest: String,
}

/// Identity of one version of the store file.
///
/// The inode is part of it because the writer replaces the file with a
/// temporary file and a rename: a rewrite in the same clock tick changes the
/// inode while the modification time may stay the same.
#[cfg(unix)]
type Fingerprint = (u64, u64, i64, i64, u64);

#[cfg(not(unix))]
type Fingerprint = (Option<std::time::SystemTime>, u64);

fn fingerprint(metadata: &std::fs::Metadata) -> Fingerprint {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.len(),
        )
    }
    #[cfg(not(unix))]
    {
        (metadata.modified().ok(), metadata.len())
    }
}

/// The records of one store version and the file identity that produced them.
#[derive(Debug, Clone, Default)]
struct StoreState {
    credentials: Vec<StoredCredential>,
    fingerprint: Option<Fingerprint>,
    degraded: bool,
}

impl StoreState {
    fn from_records(records: Vec<AuthTokenRecord>) -> Self {
        let mut state = Self::default();
        state.replace(records);
        state
    }

    /// Swap in a freshly loaded set of records.
    fn replace(&mut self, records: Vec<AuthTokenRecord>) {
        self.credentials = records
            .into_iter()
            .map(|record| StoredCredential {
                principal: Principal::new(record.label, record.actor, record.banks),
                digest: record.secret_sha256,
            })
            .collect();
    }
}

/// The loaded credential store, held for the life of one server.
///
/// Public so a host can load the store itself and hand the result to
/// [`crate::http::HttpConfig`]; the store file itself stays owned by
/// `clio_config::auth_tokens`.
#[derive(Debug, Default)]
pub struct CredentialStore {
    /// The file the records came from; absent for a store built without one.
    path: Option<PathBuf>,
    state: Mutex<StoreState>,
}

impl Clone for CredentialStore {
    /// Copy the current snapshot. The copy keeps the same file identity, so it
    /// revalidates against the same path.
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            state: Mutex::new(self.state().clone()),
        }
    }
}

impl CredentialStore {
    /// Load an operator-named store, failing closed.
    ///
    /// A path that does not exist is refused here rather than treated as an
    /// empty store, because the operator asked for a specific file and a
    /// missing one means the server is not the one they configured. Every other
    /// unusable state — malformed JSON, a file another user can read, a record
    /// the writer would have refused — is refused by the shared loader.
    ///
    /// # Errors
    /// [`clio_types::ErrorCode::ConfigCorrupt`] naming the path and the reason.
    pub fn load(path: &Path) -> Result<Self, AmError> {
        if !path.is_file() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "credential store {} does not exist; create it with `clio auth token add`",
                    path.display()
                ),
            ));
        }
        let before = stat_fingerprint(path)?;
        let records = load_auth_tokens(path)?;
        let after = stat_fingerprint(path)?;
        let mut state = StoreState::from_records(records);
        // A file that changed while it was being read is re-read on the first
        // request: no fingerprint means "not in step with the file".
        state.fingerprint = (before == after).then_some(after);
        Ok(Self {
            path: Some(path.to_path_buf()),
            state: Mutex::new(state),
        })
    }

    /// Build a store from already-validated records.
    ///
    /// Public only to the tests that need a store without a file; production
    /// callers go through [`Self::load`], which is what validates the records.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_records(records: Vec<AuthTokenRecord>) -> Self {
        Self {
            path: None,
            state: Mutex::new(StoreState::from_records(records)),
        }
    }

    /// Whether the store holds no record at all.
    ///
    /// A store with no record authorizes nothing, so it is not a usable
    /// credential source for a bind that requires one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state().credentials.is_empty()
    }

    /// Whether this store is a credential source for a request.
    ///
    /// A store loaded from a file stays a source even when every record is
    /// gone, so a live reload to an empty file refuses a request without a
    /// header instead of falling back to the unbound answer.
    fn is_credential_source(&self) -> bool {
        self.path.is_some() || !self.is_empty()
    }

    /// Resolve a presented secret to the principal its record names.
    ///
    /// `None` for an unknown secret. Every record is compared regardless of an
    /// earlier match, so a recognized credential and an unrecognized one cost
    /// the same.
    ///
    /// # Errors
    /// The reload error when the store file changed and can no longer be read.
    /// The caller refuses the request; serving the previous snapshot would keep
    /// a revoked secret alive.
    pub(crate) fn resolve(&self, presented: &str) -> Result<Option<Principal>, AmError> {
        self.refresh()?;
        let digest = digest_hex(presented);
        let state = self.state();
        let mut matched: Option<&StoredCredential> = None;
        for credential in &state.credentials {
            if ct_eq(credential.digest.as_bytes(), digest.as_bytes()) {
                matched = Some(credential);
            }
        }
        Ok(matched.map(|c| c.principal.clone()))
    }

    /// Reload the records when the file no longer matches the snapshot.
    fn refresh(&self) -> Result<(), AmError> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(self.degrade(format!(
                    "credential store {} cannot be read: {error}",
                    path.display()
                )));
            }
        };
        let current = fingerprint(&metadata);
        let in_step = self.state().fingerprint.as_ref() == Some(&current);
        if in_step {
            return Ok(());
        }
        match load_auth_tokens(path) {
            Ok(records) => {
                let mut state = self.state();
                state.replace(records);
                state.fingerprint = Some(current);
                state.degraded = false;
                Ok(())
            }
            Err(error) => Err(self.degrade(error.message)),
        }
    }

    /// Mark the store degraded, report the reason once, and return the error.
    fn degrade(&self, reason: String) -> AmError {
        let mut state = self.state();
        if !state.degraded {
            state.degraded = true;
            eprintln!("clio-mcp: refusing requests, credential store reload failed: {reason}");
        }
        AmError::new(ErrorCode::ConfigCorrupt, reason)
    }

    fn state(&self) -> MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Read the identity of the store file, refusing a file that cannot be stat'ed.
fn stat_fingerprint(path: &Path) -> Result<Fingerprint, AmError> {
    std::fs::metadata(path)
        .map(|metadata| fingerprint(&metadata))
        .map_err(|error| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("credential store {} cannot be read: {error}", path.display()),
            )
        })
}

/// What the credential check found for one request.
#[derive(Debug, Clone)]
pub(crate) enum AuthOutcome {
    /// A credential source is configured and this request presented a secret one
    /// of its records names.
    Bound(Principal),
    /// No credential source is configured, which is reachable only when the
    /// operator passed `--insecure-no-auth`.
    ///
    /// The unbound contract applies, exactly as it does for the local
    /// transports. This is a configuration, not a per-request answer, so it is
    /// decided once at start rather than per request.
    Unbound,
}

/// Decide one request's principal from the configured token store.
///
/// # Errors
/// `401` when a credential source is configured and this request does not
/// present a secret one of its records names, or when the store file can no
/// longer be read. The answer is the same for a missing header, a malformed
/// scheme, and an unknown secret, so a refusal discloses nothing about which
/// credentials exist.
pub(crate) fn authorize_request(
    store: Option<&CredentialStore>,
    header: Option<&str>,
) -> Result<AuthOutcome, u16> {
    let Some(store) = store else {
        return Ok(AuthOutcome::Unbound);
    };
    let Some(secret) = header
        .map(str::trim)
        .and_then(|presented| presented.strip_prefix("Bearer "))
    else {
        return credential_required(store);
    };
    match store.resolve(secret) {
        Ok(Some(principal)) => Ok(AuthOutcome::Bound(principal)),
        Ok(None) => credential_required(store),
        // A store that changed and can no longer be read refuses the request;
        // the answer stays the same as for a bad secret.
        Err(_) => Err(401),
    }
}

/// `401` when the server has a credential source, otherwise the unbound answer.
///
/// Keeping this in one place is what makes "a request without credentials never
/// reaches a tool" true on a bound server: there is no other path from a
/// missing header to a dispatch.
fn credential_required(store: &CredentialStore) -> Result<AuthOutcome, u16> {
    if store.is_credential_source() {
        return Err(401);
    }
    Ok(AuthOutcome::Unbound)
}

#[cfg(test)]
#[path = "http_auth_tests.rs"]
mod http_auth_tests;
