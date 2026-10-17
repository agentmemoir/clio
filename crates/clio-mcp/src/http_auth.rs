// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the network transport's credential edge: the
//! one-time load of the token store at server start, and the per-request
//! resolution of a presented bearer secret to the principal its record names.
//!
//! ## Owns
//! - `CredentialStore::load`: the fail-closed read of an operator-named store
//! - `CredentialStore::for_local_owner`: the one-credential store a loopback
//!   server mints for itself when no `--token-file` was given
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
//! # The store is loaded once
//!
//! A request must not re-read a file: that would make every request's cost
//! depend on the filesystem, and it would make a store edit take effect
//! mid-request. Revocation therefore takes effect at the next server start,
//! which is what the store's management verbs already tell the operator.
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
use clio_types::{Actor, AmError};
use std::path::Path;

/// The label the auto-generated local-owner credential is recorded under.
///
/// Fixed so the loopback server's one principal is nameable in an audit row
/// without leaking anything secret, and so a minted credential never carries a
/// label an operator record could hold.
const LOCAL_OWNER_LABEL: &str = "local-owner";

/// One loaded record: the principal it names plus the only secret-derived value
/// that exists on disk.
#[derive(Debug, Clone)]
struct StoredCredential {
    principal: Principal,
    digest: String,
}

/// The loaded credential store, held for the life of one server.
///
/// Public so a host can load the store itself and hand the result to
/// [`crate::http::HttpConfig`]; the store file itself stays owned by
/// `clio_config::auth_tokens`.
#[derive(Debug, Clone, Default)]
pub struct CredentialStore {
    credentials: Vec<StoredCredential>,
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
                clio_types::ErrorCode::ConfigCorrupt,
                format!(
                    "credential store {} does not exist; create it with `clio auth token add`",
                    path.display()
                ),
            ));
        }
        Ok(Self::from_records(load_auth_tokens(path)?))
    }

    /// Build a store from already-validated records.
    ///
    /// Public only to the tests that need a store without a file; production
    /// callers go through [`Self::load`], which is what validates the records.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_records(records: Vec<AuthTokenRecord>) -> Self {
        Self {
            credentials: records
                .into_iter()
                .map(|record| StoredCredential {
                    principal: Principal::new(record.label, record.actor, record.banks),
                    digest: record.secret_sha256,
                })
                .collect(),
        }
    }

    /// Build the one-credential store a loopback server mints for itself.
    ///
    /// Used when `clio mcp http` runs on loopback with no `--token-file`: the
    /// owner still has to present a bearer secret, and this store resolves the
    /// generated one to the local-owner principal. That principal is
    /// administrative and reaches every bank, matching the authority the owner
    /// already has over the store. Built exactly as [`Self::from_records`]
    /// builds a loaded record — the same `StoredCredential` with the same
    /// digest — so [`Self::resolve`] keeps its constant-time scan and discloses
    /// nothing new.
    #[must_use]
    pub fn for_local_owner(secret: &str) -> Self {
        Self {
            credentials: vec![StoredCredential {
                principal: Principal::local_owner(LOCAL_OWNER_LABEL.to_owned(), Actor::System),
                digest: digest_hex(secret),
            }],
        }
    }

    /// Whether the store holds no record at all.
    ///
    /// A store with no record authorizes nothing, so it is not a usable
    /// credential source for a bind that requires one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    /// Resolve a presented secret to the principal its record names.
    ///
    /// `None` for an unknown secret. Every record is compared regardless of an
    /// earlier match, so a recognized credential and an unrecognized one cost
    /// the same.
    #[must_use]
    pub(crate) fn resolve(&self, presented: &str) -> Option<Principal> {
        let digest = digest_hex(presented);
        let mut matched: Option<&StoredCredential> = None;
        for credential in &self.credentials {
            if ct_eq(credential.digest.as_bytes(), digest.as_bytes()) {
                matched = Some(credential);
            }
        }
        matched.map(|c| c.principal.clone())
    }
}

/// What the credential check found for one request.
#[derive(Debug, Clone)]
pub(crate) enum AuthOutcome {
    /// A credential source is configured and this request presented a secret one
    /// of its records names.
    Bound(Principal),
    /// No credential source is configured at all: a loopback bind in dev mode.
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
/// present a secret one of its records names. The answer is the same for a
/// missing header, a malformed scheme, and an unknown secret, so a refusal
/// discloses nothing about which credentials exist.
pub(crate) fn authorize_request(
    store: Option<&CredentialStore>,
    header: Option<&str>,
) -> Result<AuthOutcome, u16> {
    let Some(presented) = header.map(str::trim) else {
        return credential_required(store);
    };
    let Some(secret) = presented.strip_prefix("Bearer ") else {
        return credential_required(store);
    };
    match store.and_then(|s| s.resolve(secret)) {
        Some(principal) => Ok(AuthOutcome::Bound(principal)),
        None => credential_required(store),
    }
}

/// `401` when the server has a credential source, otherwise the unbound answer.
///
/// Keeping this in one place is what makes "a request without credentials never
/// reaches a tool" true on a bound server: there is no other path from a
/// missing header to a dispatch.
fn credential_required(store: Option<&CredentialStore>) -> Result<AuthOutcome, u16> {
    if store.is_some_and(|s| !s.is_empty()) {
        return Err(401);
    }
    Ok(AuthOutcome::Unbound)
}

#[cfg(test)]
#[path = "http_auth_tests.rs"]
mod http_auth_tests;
