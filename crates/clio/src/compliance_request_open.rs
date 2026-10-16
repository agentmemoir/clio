// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the CLI-side opening of one durable subject
//! request: deriving the deployment's explicit peer-enablement fact from its
//! effective configuration and enabled credential records, and persisting the
//! request with one pending peer-denial row per enabled peer before any
//! erasure effect.
//!
//! ## Owns
//! - `enabled_peers`: the enabled credential records' peer labels and the
//!   banks they cover
//! - `peer_enablement` and `peer_enablement_facts`: the explicit
//!   `PeerEnablement` fact with its frozen labels and covered banks
//! - `open_subject_request`: the register-or-replay, the record persistence,
//!   and the peer snapshot, in that order
//!
//! ## Does not own
//! - The request state machine, the frozen record vocabulary, and the reused-id
//!   refusal (see `clio_compliance`'s `subject_request`)
//! - The durable request row and the per-peer action row contracts (see
//!   `clio-store`)
//! - The credential record shape and its fail-closed loading (see
//!   `clio_config::auth_tokens`)
//! - Which command exposes this open, and its flags or usage text (see
//!   `compliance_cli` / `cli_command_registry_compliance`)
//!
//! ## Boundary
//!
//! Changes to how the CLI derives peer enablement or orders the request open
//! should normally require changes here. Changes to the request state machine,
//! the durable row contracts, or the credential record should not.
//!
//! The CLI's shipping erasure command runs the in-process `erase_request`
//! tool, which does not bind the durable request store; this open path is the
//! seam a request-opening command binds, and its tests drive it today.
//!
//! Keep this module focused on the CLI-side request open.

use crate::cli_error::CliError;
use clio_compliance::subject_request_store_wiring::{record_to_row, reload_ledger};
use clio_compliance::{PeerEnablement, RegisterOutcome, RequestKind, RequestScope, snapshot_peers};
use clio_store::{DurableRequestStore, PeerActionStore};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::collections::BTreeSet;

/// The labels and covered banks of the deployment's enabled credential
/// records.
///
/// The store is the one the auth verbs manage: `CLIO_AUTH_TOKEN_FILE` when it
/// names a path, else the store under the config directory. A missing file is
/// an empty store; every other unreadable state refuses, so a request never
/// opens under a peer set the deployment's own records do not support. Each
/// record contributes its label as one peer identity and every bank it names
/// as reach; the bank union is deduplicated and ordered, so one read carries
/// both facts.
///
/// # Errors
/// No credential store path can be resolved, or the store cannot be read and
/// validated.
fn enabled_peers(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(Vec<String>, BTreeSet<String>), CliError> {
    let Some(path) = clio_config::auth_tokens::resolve_token_store_path_with(None, get) else {
        return Err(CliError::from_am_error(&AmError::new(
            ErrorCode::ConfigCorrupt,
            "no credential store path could be resolved, so the enabled peers cannot be read",
        )));
    };
    let records = clio_config::auth_tokens::load_auth_tokens(&path)
        .map_err(|error| CliError::from_am_error(&error))?;
    let mut labels = Vec::with_capacity(records.len());
    let mut banks = BTreeSet::new();
    for record in records {
        labels.push(record.label);
        banks.extend(record.banks);
    }
    Ok((labels, banks))
}

/// The deployment's peer-enablement fact, with its frozen labels and covered
/// banks.
///
/// Sync omitted (or explicitly disabled) is a single-host deployment, so peer
/// work is not applicable: a first-class fact, not an unknown value. Sync in
/// play makes each enabled credential record one enabled peer, and its label
/// is part of the required set while its banks are the banks the peer covers;
/// an enabled deployment whose store holds no record names no peer at all, so
/// it too reads not-applicable — a required set would be empty, and the record
/// contract refuses a required fact with no peer.
///
/// # Errors
/// The credential store cannot be resolved or read, so the enabled peer set is
/// unknown and a request must not open on a guessed set.
fn peer_enablement_facts(
    effective: &Value,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(PeerEnablement, Vec<String>, BTreeSet<String>), CliError> {
    if crate::setup_sync_config::single_host(effective) {
        return Ok((PeerEnablement::NotApplicable, Vec::new(), BTreeSet::new()));
    }
    let (labels, banks) = enabled_peers(get)?;
    if labels.is_empty() {
        return Ok((PeerEnablement::NotApplicable, labels, banks));
    }
    Ok((PeerEnablement::Required, labels, banks))
}

/// The deployment's explicit peer-enablement fact, with its frozen labels.
///
/// # Errors
/// The credential store cannot be resolved or read, so the enabled peer set is
/// unknown and a request must not open on a guessed set.
pub(crate) fn peer_enablement(
    effective: &Value,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<(PeerEnablement, Vec<String>), CliError> {
    let (enablement, labels, _) = peer_enablement_facts(effective, get)?;
    Ok((enablement, labels))
}

/// Open one durable subject request, or replay the stored one, with the
/// configuration-derived enablement fact and covered peer banks frozen on it.
///
/// The request record is persisted before the per-peer snapshot, and both
/// precede any erasure effect. A reused request id bound to the same kind and
/// scope replays its stored record and applies nothing; a reused id bound to a
/// different kind or scope is refused, and nothing changes. The snapshot is
/// idempotent per peer, so a replayed or retried open writes only the rows a
/// failed earlier attempt left unwritten.
///
/// `effective` is the effective configuration of a `clio_config::Runtime`
/// built with the same `get` lookup, so the two reads can never disagree about
/// the deployment.
///
/// # Errors
/// The enablement fact cannot be derived, the request id is already bound to a
/// different kind or scope, or the record or the snapshot cannot be persisted.
#[allow(
    dead_code,
    reason = "the request-opening command binds this seam; the CLI tests drive it today"
)]
pub(crate) fn open_subject_request(
    store: &dyn DurableRequestStore,
    peers: &dyn PeerActionStore,
    request_id: &str,
    kind: RequestKind,
    scope: &RequestScope,
    effective: &Value,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<RegisterOutcome, CliError> {
    let (enablement, labels, banks) = peer_enablement_facts(effective, get)?;
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let banks: Vec<&str> = banks.iter().map(String::as_str).collect();
    let mut ledger = reload_ledger(store).map_err(|error| CliError::from_am_error(&error))?;
    let outcome = ledger
        .register_with_enablement(request_id, kind, scope, enablement, &labels, &banks)
        .map_err(|error| CliError::from_am_error(&error))?;
    let record = ledger.record(request_id).cloned().ok_or_else(|| {
        CliError::operational(
            "internal",
            "the opened request is missing from the reloaded ledger",
        )
    })?;
    store
        .save(&record_to_row(&record))
        .map_err(|error| CliError::from_am_error(&error))?;
    snapshot_peers(peers, &record).map_err(|error| CliError::from_am_error(&error))?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "compliance_request_open_tests.rs"]
mod compliance_request_open_tests;
