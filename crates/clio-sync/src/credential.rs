// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync server's credential edge: the
//! one-time load of the shared token store at start, and the per-request
//! resolution of a presented bearer secret to the peer (label plus bank list)
//! its record names.
//!
//! ## Owns
//! - `CredentialStore::load`: the fail-closed read of an operator-named store
//! - `resolve`: digest comparison through the shared constant-time helper
//! - `Peer`: the per-request bank scope, the record's label and banks
//! - `authorize_request`: the one request check the connection layer reads
//!
//! ## Does not own
//! - The record shape, its validation, and the store file itself (see
//!   `clio_config::auth_tokens`)
//! - Where a resolved scope is applied to a bank (see `server`)
//! - The dev-mode anonymous-bind policy (see `server`)
//!
//! ## Boundary
//!
//! Changes to how a presented secret becomes a peer should normally require
//! changes here. Changes to what a peer may reach should not.
//!
//! Keep this module focused on the credential edge and nothing past it.
//!
//! # The store is loaded once
//!
//! A request must not re-read a file: that would make every request's cost
//! depend on the filesystem, and it would make a store edit take effect
//! mid-request. Revocation therefore takes effect at the next server start.
//!
//! # Why the scan does not stop at the match
//!
//! A constant-time byte comparison is not enough on its own: returning as soon
//! as a record matches makes a successful credential do less work than an
//! unknown one, and that difference is measurable. So every record's digest is
//! compared and the match accumulates, and the answer is read back afterwards.
//! Both compared values are fixed-length lowercase hex on every path — the
//! store's record digests and the digest of the presented secret — so the
//! length check inside the shared comparison discloses nothing about the
//! secret.

use clio_config::auth_tokens::{AuthTokenRecord, ct_eq, digest_hex, load_auth_tokens};
use clio_types::{AmError, ErrorCode};
use std::path::Path;

/// One resolved peer: the record's label and the banks it may reach.
///
/// The label is operator-facing and not secret. No production consumer reads
/// it yet: tests do today, and audit recording — the planned use — is not
/// built yet, so nothing in diagnostics identifies the peer through it today.
/// The record also names an actor, but no sync path is actor-gated, so it is
/// deliberately not carried here and no caller may rely on one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Peer {
    pub(crate) label: String,
    /// Exactly the banks the record listed, in the record's order.
    pub(crate) banks: Vec<String>,
}

impl Peer {
    /// Whether `bank` is inside this peer's record scope.
    pub(crate) fn allows(&self, bank: &str) -> bool {
        self.banks.iter().any(|b| b == bank)
    }
}

/// One loaded record: the peer it names plus the only secret-derived value
/// that exists on disk.
#[derive(Debug, Clone)]
struct StoredCredential {
    peer: Peer,
    digest: String,
}

/// The loaded credential store, held for the life of one server.
#[derive(Debug, Clone, Default)]
pub(crate) struct CredentialStore {
    credentials: Vec<StoredCredential>,
}

impl CredentialStore {
    /// Load an operator-named store, failing closed.
    ///
    /// A path that does not exist is refused rather than treated as an empty
    /// store, because the operator asked for a specific file and a missing one
    /// means the server is not the one they configured. A store holding no
    /// record is refused too: it authorizes nobody, so starting with it would
    /// only move the operator's mistake from start-up to the first request.
    /// Every other unusable state is refused by the shared loader.
    ///
    /// # Errors
    /// [`ErrorCode::ConfigCorrupt`] naming the path and the reason;
    /// [`ErrorCode::Forbidden`] for a store another user can read.
    pub(crate) fn load(path: &Path) -> Result<Self, AmError> {
        if !path.is_file() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "credential store {} does not exist; create it with `clio auth token add`",
                    path.display()
                ),
            ));
        }
        let records = load_auth_tokens(path)?;
        if records.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "credential store {} holds no records; create one with `clio auth token add`",
                    path.display()
                ),
            ));
        }
        Ok(Self::from_records(records))
    }

    /// Build a store from already-validated records.
    ///
    /// [`Self::load`] is the production caller and the one that validates the
    /// records; this is also reachable from tests so a store can be built
    /// without a file.
    pub(crate) fn from_records(records: Vec<AuthTokenRecord>) -> Self {
        Self {
            credentials: records
                .into_iter()
                .map(|record| StoredCredential {
                    peer: Peer {
                        label: record.label,
                        banks: record.banks,
                    },
                    digest: record.secret_sha256,
                })
                .collect(),
        }
    }

    /// Resolve a presented secret to the peer its record names.
    ///
    /// `None` for an unknown secret. Every record is compared regardless of an
    /// earlier match, so a recognized credential and an unrecognized one cost
    /// the same.
    #[must_use]
    pub(crate) fn resolve(&self, presented: &str) -> Option<Peer> {
        let digest = digest_hex(presented);
        let mut matched: Option<&StoredCredential> = None;
        for credential in &self.credentials {
            if ct_eq(credential.digest.as_bytes(), digest.as_bytes()) {
                matched = Some(credential);
            }
        }
        matched.map(|c| c.peer.clone())
    }
}

/// What the credential check found for one request.
#[derive(Debug, Clone)]
pub(crate) enum AuthOutcome {
    /// A credential source is configured and this request presented a secret
    /// one of its records names.
    Bound(Peer),
    /// No credential source is configured at all: the dev-mode anonymous
    /// answer, whose effective banks are the server's allow-list.
    Unbound,
}

impl AuthOutcome {
    /// Whether this request may name `bank`, before the server allow-list.
    pub(crate) fn allows(&self, bank: &str) -> bool {
        match self {
            Self::Bound(peer) => peer.allows(bank),
            Self::Unbound => true,
        }
    }
}

/// Decide one request's bank scope from the configured credential store.
///
/// The record's digest comparison is the only credential check: every record
/// is compared through the one shared constant-time helper, and a presented
/// secret that no record names is refused.
///
/// # Errors
/// `401` when a credential source is configured and this request does not
/// present a secret one of its records names. The answer is the same for a
/// missing header, a malformed scheme, and an unknown secret, so a refusal
/// discloses nothing about which credentials exist.
pub(crate) fn authorize_request(
    store: Option<&CredentialStore>,
    dev_mode: bool,
    header: Option<&str>,
) -> Result<AuthOutcome, u16> {
    if let Some(store) = store {
        // The header parse and the record lookup are one chain, so every way
        // they can fail falls through to the single refusal below.
        if let Some(peer) = header
            .and_then(|h| h.strip_prefix("Bearer "))
            .and_then(|secret| store.resolve(secret))
        {
            return Ok(AuthOutcome::Bound(peer));
        }
        return Err(401);
    }
    if dev_mode {
        return Ok(AuthOutcome::Unbound);
    }
    Err(401)
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod credential_tests;
