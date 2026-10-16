// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the coordinator fixture of the peer erasure
//! acceptance target: one backend store with its restart boundary, the
//! request-open and peer-snapshot calls, the erasure and reconcile entries,
//! the one denial-delivery call, and the per-peer lifecycle reads.
//!
//! ## Owns
//! - `Run`, the coordinator fixture over one backend
//! - `Aux`, an extra node store beside the coordinator (a stale writer or a
//!   reader), with the backend resources it keeps alive
//! - The action-id helpers and the resume facts (`erasure_action_id`,
//!   `peer_action_id`, `resume`)
//!
//! ## Does not own
//! - The synthetic constants, scratch directory, and store probes (`support`)
//! - The peer process and its logs (`sync_wire_support::peer`)
//! - What each scenario asserts (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how the coordinator is opened, driven, or restarted should
//! normally require changes here. Changes to scenario assertions should not.
//!
//! Keep this module focused on the coordinator fixture.

// Every scenario module of this target uses a subset of these calls; the
// target compiles as one binary, so an unused-in-one-suite helper is not dead.
#![allow(dead_code)]

use clio::store::{
    DurableRequestStore, LocalDevKms, LocalEventStore, LocalEventView, PeerActionKind,
    PeerActionRow, PeerActionStore, PgTestGuard, PostgresStore, ScopedSchema, SqliteStore,
    lock_pg_test, probe_postgres,
};
use clio::{Actor, AmError, Bank, OpContext};
use clio_compliance::subject_request_store_wiring::{
    erase_from_durable_store, record_to_row, row_to_record,
};
use clio_compliance::{
    EraseArgs, EraseOutcome, EraseSink, EraseVecSink, ErasureReconcileReport, ErasureResume,
    PeerEnablement, PeerState, RegisterOutcome, RequestCommand, RequestKind, RequestScope,
    SubjectRequestLedger, SubjectRequestRecord, derived_peer_states, snapshot_peers,
};
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::support::{
    AcceptanceStore, BANK, SUBJECT, fingerprint, local_kms, postgres_skip_requested, postgres_url,
};

/// One extra node store beside the coordinator: a stale writer or a reader,
/// kept alive with the backend resources it needs.
pub struct Aux<T: AcceptanceStore + 'static> {
    /// The node's store.
    pub store: Arc<T>,
    device_id: String,
    keepalive: Vec<Box<dyn Any + Send>>,
}

impl<T: AcceptanceStore + 'static> Aux<T> {
    /// The node's own device id, distinct from the coordinator's.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device_id
    }
}

/// The backend's auxiliary-store opener: one extra node store per call, with
/// the backend resource it needs kept alive alongside it.
type AuxOpener<T> = Box<dyn Fn(&str) -> Aux<T>>;

/// The coordinator fixture: one backend store, its journal, its deployment
/// salt and fingerprint, and the resources a reopen needs.
pub struct Run<T: AcceptanceStore + 'static> {
    /// The per-test scratch root that owns every file fixture.
    pub scratch: super::support::Scratch,
    /// The coordinator's durable store; a reopen replaces it.
    pub store: Arc<T>,
    /// The coordinator's owner-only erasure journal.
    pub journal: std::path::PathBuf,
    /// The deployment fingerprint both sides verify proofs under.
    pub fingerprint: String,
    /// The deployment salt the erasure hashes subject identities with.
    pub salt: [u8; 16],
    /// This driver's own device id, carried on every operation it originates.
    pub device: String,
    reopen: Box<dyn Fn() -> T>,
    aux: AuxOpener<T>,
    pg_base: Option<String>,
    _pg: Option<(PgTestGuard, ScopedSchema)>,
}

impl Run<SqliteStore> {
    /// Open one SQLite-backed coordinator run.
    #[must_use]
    pub fn sqlite(tag: &str) -> Self {
        let scratch = super::support::Scratch::new(tag);
        let db = scratch.join("coordinator.db");
        let kms: Arc<dyn clio::store::DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open(&db, Arc::clone(&kms)).expect("open the coordinator store");
        let reopen_db = db.clone();
        let reopen_kms = Arc::clone(&kms);
        let aux_dir = db
            .parent()
            .expect("the coordinator db has a parent")
            .to_path_buf();
        Self {
            journal: scratch.join("coordinator-journal.json"),
            fingerprint: fingerprint(),
            salt: rand::random(),
            device: format!("coordinator-{}", std::process::id()),
            reopen: Box::new(move || {
                SqliteStore::open(&reopen_db, Arc::clone(&reopen_kms))
                    .expect("reopen the coordinator store")
            }),
            aux: Box::new(move |tag| {
                // Each auxiliary node is its own store with its own sealed-key
                // space and its own key provider: the coordinator's erasure
                // must never destroy a peer's ability to hold its own copy,
                // and the denial is what must refuse the copy at the sync
                // boundary.
                let dir = aux_dir.join(tag);
                std::fs::create_dir_all(&dir).expect("create the auxiliary node directory");
                Aux {
                    store: Arc::new(
                        SqliteStore::open(
                            &dir.join(format!("{tag}.db")),
                            Arc::new(LocalDevKms::new()),
                        )
                        .expect("open the auxiliary node store"),
                    ),
                    device_id: format!("{tag}-{}", std::process::id()),
                    keepalive: Vec::new(),
                }
            }),
            scratch,
            store: Arc::new(store),
            pg_base: None,
            _pg: None,
        }
    }
}

impl Run<PostgresStore> {
    /// Open one Postgres-backed coordinator run on a private schema.
    ///
    /// An unreachable Postgres panics unless the operator set the explicit
    /// skip; a scenario opened without a backend would report a no-op as a
    /// result.
    #[must_use]
    pub fn postgres(tag: &str) -> Option<Self> {
        let guard = lock_pg_test();
        let base = postgres_url();
        if let Err(detail) = probe_postgres(&base) {
            if postgres_skip_requested() {
                eprintln!(
                    "skip peer erasure acceptance postgres scenario `{tag}`: the explicit \
                     skip is set and no Postgres answers at the configured URL ({detail})"
                );
                return None;
            }
            panic!(
                "the peer erasure acceptance Postgres scenarios require a reachable Postgres: \
                 {detail}"
            );
        }
        let schema = ScopedSchema::new(&base);
        let url = schema.url().to_owned();
        let scratch = super::support::Scratch::new(tag);
        let store = PostgresStore::open(&url, local_kms()).expect("open the coordinator store");
        let reopen_url = url;
        let aux_base = base.clone();
        Some(Self {
            journal: scratch.join("coordinator-journal.json"),
            fingerprint: fingerprint(),
            salt: rand::random(),
            device: format!("coordinator-{}", std::process::id()),
            reopen: Box::new(move || {
                PostgresStore::open(&reopen_url, local_kms()).expect("reopen the coordinator store")
            }),
            aux: Box::new(move |tag| {
                let aux_schema = ScopedSchema::new(&aux_base);
                let store = PostgresStore::open(aux_schema.url(), local_kms())
                    .expect("open the auxiliary node store");
                Aux {
                    store: Arc::new(store),
                    device_id: format!("{tag}-{}", std::process::id()),
                    keepalive: vec![Box::new(aux_schema)],
                }
            }),
            scratch,
            store: Arc::new(store),
            pg_base: Some(base),
            _pg: Some((guard, schema)),
        })
    }
}

impl<T: AcceptanceStore + 'static> Run<T> {
    /// The coordinator store behind this run.
    #[must_use]
    pub fn store(&self) -> &T {
        self.store.as_ref()
    }

    /// The Postgres base URL this run's private schema belongs to, when this
    /// is a Postgres-backed run.
    #[must_use]
    pub fn pg_base(&self) -> Option<&str> {
        self.pg_base.as_deref()
    }

    /// Close the store and open the same backend again: the restart boundary.
    pub fn restart(&mut self) {
        self.store = Arc::new((self.reopen)());
    }

    /// One extra node store beside the coordinator, for a stale writer or a
    /// reader.
    #[must_use]
    pub fn aux(&self, tag: &str) -> Aux<T> {
        (self.aux)(tag)
    }

    /// Open one erasure request over this bank and subject with its explicit
    /// enablement fact and frozen peer labels, persist it, snapshot one
    /// pending denial row per peer, and start its work.
    #[must_use]
    pub fn open_request(&self, request_id: &str, peers: &[&str]) -> SubjectRequestRecord {
        let enablement = if peers.is_empty() {
            PeerEnablement::NotApplicable
        } else {
            PeerEnablement::Required
        };
        let scope = RequestScope::Subject {
            bank_id: BANK.to_owned(),
            subject_id: SUBJECT.to_owned(),
        };
        let mut ledger = SubjectRequestLedger::new();
        let outcome = ledger
            .register_with_enablement(
                request_id,
                RequestKind::Erasure,
                &scope,
                enablement,
                peers,
                &[BANK],
            )
            .expect("the request opens");
        assert_eq!(outcome, RegisterOutcome::Opened, "a fresh request id opens");
        ledger
            .apply(request_id, &RequestCommand::Begin)
            .expect("the work starts");
        let record = ledger
            .record(request_id)
            .expect("the record exists")
            .clone();
        self.store()
            .save(&record_to_row(&record))
            .expect("the record persists");
        snapshot_peers(self.store(), &record).expect("the peers snapshot");
        record
    }

    /// The durable peer row one request and peer currently hold.
    #[must_use]
    pub fn peer_row(&self, request_id: &str, peer_id: &str) -> PeerActionRow {
        self.peer_row_opt(request_id, peer_id)
            .expect("the durable peer row exists")
    }

    /// The durable peer row one request and peer currently hold, if any.
    #[must_use]
    pub fn peer_row_opt(&self, request_id: &str, peer_id: &str) -> Option<PeerActionRow> {
        PeerActionStore::load(self.store(), request_id, peer_id, PeerActionKind::Denial)
            .expect("load the peer row")
    }

    /// The record's peer slots as the fold derives them from the durable rows.
    #[must_use]
    pub fn peer_states(&self, request_id: &str) -> BTreeMap<String, PeerState> {
        let record = self.load_record(request_id);
        let rows = self
            .store()
            .load_for_request(request_id)
            .expect("load the peer rows");
        derived_peer_states(&record, &rows)
    }

    /// The stored request record, reloaded from the durable row.
    #[must_use]
    pub fn load_record(&self, request_id: &str) -> SubjectRequestRecord {
        let row =
            DurableRequestStore::load(self.store(), request_id).expect("load the durable row");
        let row = row.expect("the durable row exists");
        row_to_record(&row).expect("the row decodes")
    }

    /// Every event row one action id holds.
    #[must_use]
    pub fn events_for(&self, action_id: &str) -> Vec<LocalEventView> {
        self.store()
            .list_events()
            .expect("events")
            .into_iter()
            .filter(|event| event.action_id == action_id)
            .collect()
    }

    /// The number of committed tombstones one request's purge wrote.
    #[must_use]
    pub fn tombstones(&self, request_id: &str) -> usize {
        self.store()
            .read_tombstones(BANK, request_id)
            .expect("tombstones")
            .len()
    }

    /// Run the confirmed erasure over the durable record this run holds.
    ///
    /// # Errors
    /// The retry contract's own refusals and failures.
    pub fn erase(&self, request_id: &str) -> Result<EraseOutcome, AmError> {
        self.erase_with(request_id, self.store())
    }

    /// Run the confirmed erasure with an injected event writer.
    ///
    /// # Errors
    /// The retry contract's own refusals and failures.
    pub fn erase_with(
        &self,
        request_id: &str,
        events: &dyn LocalEventStore,
    ) -> Result<EraseOutcome, AmError> {
        let sink = EraseVecSink::default();
        let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
        let live = self
            .store()
            .live_banks(BANK, SUBJECT)
            .expect("the live bank set");
        let args = self.erase_args(&ctx, &sink, request_id);
        erase_from_durable_store(self.store(), events, self.store(), request_id, &args, &live)
    }

    /// The erasure arguments one request's call carries.
    #[must_use]
    pub fn erase_args<'a>(
        &'a self,
        ctx: &'a OpContext,
        sink: &'a dyn EraseSink,
        request_id: &'a str,
    ) -> EraseArgs<'a> {
        EraseArgs {
            store: self.store(),
            journal_path: &self.journal,
            maint: None,
            ctx,
            subject_id: SUBJECT,
            legal_basis: "GDPR_ART_17",
            request_id,
            reason_code: None,
            confirm: true,
            salt: &self.salt,
            timestamp: super::support::NOW,
            sink,
        }
    }

    /// Close the pending erasure actions a restart left open, by action id.
    ///
    /// # Errors
    /// A pending or event read failure, a durable load failure, or a store
    /// probe failure.
    pub fn reconcile(
        &self,
        resume: &dyn Fn(&SubjectRequestRecord) -> Option<ErasureResume>,
    ) -> Result<ErasureReconcileReport, AmError> {
        self.reconcile_with(self.store(), resume)
    }
}

/// The erasure action id this target's requests are keyed by.
#[must_use]
pub fn erasure_action_id(request_id: &str) -> String {
    format!(
        "{}:{request_id}",
        clio_compliance::ProtectedAction::Erasure.producer_id()
    )
}

/// The peer-denial action id one request and peer are keyed by.
#[must_use]
pub fn peer_action_id(request_id: &str, peer_id: &str) -> String {
    clio_compliance::peer_action_id(request_id, peer_id)
}

/// The resume facts a restart supplies for this run's subject.
#[must_use]
pub fn resume() -> ErasureResume {
    ErasureResume {
        subject_id: SUBJECT.to_owned(),
        live_banks: vec![BANK.to_owned()],
    }
}
