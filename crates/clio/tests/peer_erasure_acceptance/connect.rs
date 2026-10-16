// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the driver's client calls against a spawned
//! peer process: one denial delivery over the push route, a pending-mutation
//! push from a node's store, a pull into a node's store, and the reconcile
//! pass over the coordinator store with an injectable event writer.
//!
//! ## Owns
//! - `Run::deliver` / `Run::deliver_target`, the one denial delivery call
//! - `push_pending` and `pull_into`, the node-to-peer client calls
//! - `Run::reconcile_with`, the reconcile over an injected event writer
//!
//! ## Does not own
//! - The coordinator fixture and its erase entries (`run`)
//! - The spawned peer process (`sync_wire_support::peer`) and the probe
//!   pairing (`nodes`)
//! - What each scenario asserts (`scenarios_peer`, `scenarios_disposition`)
//!
//! ## Boundary
//!
//! Changes to how the driver calls a peer over the wire should normally
//! require changes here. Changes to scenario assertions should not.
//!
//! Keep this module focused on the driver's synchronous client calls.

#![allow(dead_code)]

use clio::{Actor, AmError};
use clio_compliance::{
    EraseVecSink, ErasureReconcileReport, ErasureResume, ErasureRetryEnv, SubjectRequestRecord,
    reconcile_erasure_actions,
};
use clio_store::{LocalEventStore, Store, SyncStore};
use clio_sync::client::{SyncClientConfig, pull, push};
use clio_sync::delivery::{DeliveryOutcome, DenialTarget, deliver_denial};
use std::sync::Arc;

use super::run::Run;
use super::support::{AcceptanceStore, BANK, NOW};
use crate::sync_wire_support::TOKEN;

impl<T: AcceptanceStore + 'static> Run<T> {
    /// One delivery of this request's denial to `peer_id` at `remote`, riding
    /// `BANK` and covering it, under `target_fingerprint`, presented with an
    /// optional bearer.
    ///
    /// # Errors
    /// The caller's own input: a blank target field or no remote.
    pub fn deliver(
        &self,
        remote: &str,
        peer_id: &str,
        request_ref: &str,
        subject: &str,
        target_fingerprint: &str,
        token: Option<&str>,
    ) -> Result<DeliveryOutcome, AmError> {
        self.deliver_target(
            remote,
            peer_id,
            request_ref,
            subject,
            BANK,
            &[BANK],
            target_fingerprint,
            token,
        )
    }

    /// One delivery with every target field explicit, so a scenario can bind
    /// the wrong peer, the wrong deployment, or a bank the presented
    /// credential cannot reach.
    ///
    /// # Errors
    /// The caller's own input: a blank target field or no remote.
    #[allow(clippy::too_many_arguments)]
    pub fn deliver_target(
        &self,
        remote: &str,
        peer_id: &str,
        request_ref: &str,
        subject: &str,
        bank: &str,
        covering: &[&str],
        target_fingerprint: &str,
        token: Option<&str>,
    ) -> Result<DeliveryOutcome, AmError> {
        let cfg = client(Arc::clone(&self.store), &self.device, remote, token);
        deliver_denial(
            &cfg,
            &DenialTarget {
                request_ref,
                peer_id,
                bank,
                subject_id: subject,
                covering,
                salt_fingerprint: target_fingerprint,
            },
        )
    }

    /// Close the pending erasure and peer-denial actions a restart left open,
    /// reading outcomes through `events`.
    ///
    /// # Errors
    /// A pending or event read failure, a durable load failure, or a store
    /// probe failure.
    pub fn reconcile_with(
        &self,
        events: &dyn LocalEventStore,
        resume: &dyn Fn(&SubjectRequestRecord) -> Option<ErasureResume>,
    ) -> Result<ErasureReconcileReport, AmError> {
        let sink = EraseVecSink::default();
        let env = ErasureRetryEnv {
            store: self.store(),
            peers: self.store(),
            journal_path: &self.journal,
            maint: None,
            actor: Actor::Harness,
            legal_basis: "GDPR_ART_17",
            salt: &self.salt,
            timestamp: NOW,
            sink: &sink,
        };
        reconcile_erasure_actions(self.store(), events, &env, resume)
    }
}

/// Push every pending mutation of `banks` from one node's store.
///
/// # Errors
/// Transport, auth, or transient storage failures.
pub fn push_pending<T: AcceptanceStore + 'static>(
    node: &Arc<T>,
    device: &str,
    remote: &str,
    banks: &[&str],
) -> Result<(), AmError> {
    let cfg = client(Arc::clone(node), device, remote, Some(TOKEN));
    push(&cfg, &owned(banks)).map(|_| ())
}

/// Pull `banks` from the peer into one node's store.
///
/// # Errors
/// Transport, auth, or a refused bank.
pub fn pull_into<T: AcceptanceStore + 'static>(
    node: &Arc<T>,
    device: &str,
    remote: &str,
    banks: &[&str],
) -> Result<(), AmError> {
    let cfg = client(Arc::clone(node), device, remote, Some(TOKEN));
    pull(&cfg, &owned(banks)).map(|_| ())
}

/// One delivery attempt over an owned client configuration, so a scenario can
/// run two deliveries of the same denial concurrently from separate threads.
/// The caller keeps the target strings and hands them over borrowed. The
/// target fields are the `deliver_target` defaults: the request's denial
/// riding `BANK` and covering it.
///
/// # Errors
/// The caller's own input: a blank target field or no remote.
pub fn deliver_owned<T: AcceptanceStore + 'static>(
    store: Arc<T>,
    device: &str,
    remote: &str,
    request_ref: &str,
    peer_id: &str,
    subject: &str,
    target_fingerprint: &str,
) -> Result<DeliveryOutcome, AmError> {
    let cfg = client(store, device, remote, Some(TOKEN));
    deliver_denial(
        &cfg,
        &DenialTarget {
            request_ref,
            peer_id,
            bank: BANK,
            subject_id: subject,
            covering: &[BANK],
            salt_fingerprint: target_fingerprint,
        },
    )
}

/// The one sync client configuration every driver call shares.
fn client<T: AcceptanceStore + 'static>(
    store: Arc<T>,
    device: &str,
    remote: &str,
    token: Option<&str>,
) -> SyncClientConfig {
    SyncClientConfig {
        device_id: device.to_owned(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: store as Arc<dyn SyncStore>,
        remote: Some(remote.to_owned()),
        token: token.map(str::to_owned),
        cipher: Vec::new(),
    }
}

/// Owned bank strings for one client call.
fn owned(banks: &[&str]) -> Vec<String> {
    banks.iter().map(|bank| (*bank).to_owned()).collect()
}
