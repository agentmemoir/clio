// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the local event store contract: one durable,
//! ordered record per protected local action, written as an intent before the
//! protected effect and completed by an outcome write, with manual pruning that
//! keeps request and denial state.
//!
//! ## Owns
//! - `LocalEventStore`, its intent type, and the read-back view/state types
//! - The per-backend SQL for the intent insert, outcome update, pending
//!   listing, ordered read, and prune (both drivers, in this module)
//!
//! ## Does not own
//! - The `local_events` DDL text (`sql/003_local_compliance_slots.sql`, loaded
//!   through `migrate`)
//! - The protected effects themselves, and any redaction of the result text
//!
//! ## Boundary
//!
//! Changes to which rows the writer writes, reads, or prunes should normally
//! require changes here. Changes to what a protected effect does should not.
//!
//! Keep this module focused on the local event persistence contract.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::OptionalExtension;

/// The state of one local event row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEventState {
    /// The intent is durable and the outcome is not yet recorded.
    Pending,
    /// The protected effect ran and was allowed.
    Allowed,
    /// The protected effect was refused.
    Denied,
}

impl LocalEventState {
    /// The stored CHECK-domain spelling.
    fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }

    /// Read the stored spelling back, failing closed on anything else.
    fn from_db(state: &str) -> Result<Self, AmError> {
        match state {
            "pending" => Ok(Self::Pending),
            "allowed" => Ok(Self::Allowed),
            "denied" => Ok(Self::Denied),
            other => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("local_events row has unknown outcome_state `{other}`"),
            )),
        }
    }
}

/// The intent half of one local event: written before the protected effect.
///
/// `action_id` is the caller's unique id for the action; the outcome write,
/// its duplicate tolerance, and restart reconciliation all key on it. `scope`
/// is opaque and stored verbatim. The outcome's `result` text is the caller's
/// redaction responsibility — the store never filters it.
#[derive(Debug, Clone)]
pub struct LocalEventIntent<'a> {
    /// Caller-unique id for the protected action.
    pub action_id: &'a str,
    /// Attribution actor.
    pub actor: &'a str,
    /// Opaque scope string, stored verbatim.
    pub scope: &'a str,
    /// Event kind label.
    pub kind: &'a str,
    /// Event time (UTC text), caller-supplied so tests control the clock.
    pub event_time: &'a str,
}

/// One local event row as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEventView {
    /// Caller-unique id for the protected action.
    pub action_id: String,
    /// Store-assigned order position.
    pub seq: i64,
    /// Attribution actor.
    pub actor: String,
    /// Opaque scope string.
    pub scope: String,
    /// Event kind label.
    pub kind: String,
    /// Event time (UTC text).
    pub event_time: String,
    /// Pending/allowed/denied state.
    pub state: LocalEventState,
    /// Redacted outcome text; `None` while pending.
    pub result: Option<String>,
}

/// Persistence contract for the local event path.
///
/// Implementations MUST persist the intent before the protected effect runs,
/// assign the order position in the insert transaction, and keep the pending
/// record durable when the outcome write fails after the effect.
pub trait LocalEventStore: Send + Sync {
    /// Persist the intent as an outcome-pending record.
    ///
    /// Call this before the protected effect: any error means the action must
    /// be refused, and nothing durable is left half-written. A repeated
    /// `action_id` is refused. Two concurrent writers can collide on the order
    /// position; one insert then fails on the `seq` uniqueness and may retry.
    ///
    /// # Errors
    /// Empty required fields, a repeated `action_id`, or storage failures.
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError>;

    /// Complete the pending record for `action_id`.
    ///
    /// Duplicate-tolerant by design: an already-completed row is left
    /// untouched and reported as success, so a repeated write — or the
    /// reconcile of a pending record after a restart — cannot double-write.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] when no intent row exists for the action, or
    /// storage failures.
    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError>;

    /// Action ids whose outcome is still pending, in order position — the
    /// restart reconciliation queue.
    ///
    /// # Errors
    /// Storage failures.
    fn pending_actions(&self) -> Result<Vec<String>, AmError>;

    /// Every event row in order position (the restart-ordering read).
    ///
    /// # Errors
    /// Storage failures.
    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError>;

    /// Manual pruning: delete completed allowed rows whose `event_time` is
    /// before `cutoff`. Request (pending) and denial rows always stay.
    /// Returns the deleted row count.
    ///
    /// # Errors
    /// Storage failures.
    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError>;
}

/// Refuse an intent that is missing a required field.
fn validate_intent(intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
    for (name, value) in [
        ("action_id", intent.action_id),
        ("actor", intent.actor),
        ("scope", intent.scope),
        ("kind", intent.kind),
        ("event_time", intent.event_time),
    ] {
        if value.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("local event {name} is required"),
            ));
        }
    }
    Ok(())
}

/// Refuse an outcome write for an action with no intent record.
fn unknown_action(action_id: &str) -> AmError {
    AmError::new(
        ErrorCode::NotFound,
        format!("no local event intent exists for action `{action_id}`"),
    )
}

fn validate_action_id(action_id: &str) -> Result<(), AmError> {
    if action_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "local event action_id is required",
        ));
    }
    Ok(())
}

fn outcome_state(denied: bool) -> LocalEventState {
    if denied {
        LocalEventState::Denied
    } else {
        LocalEventState::Allowed
    }
}

impl LocalEventStore for SqliteStore {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        validate_intent(intent)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("local event begin")?;
            let lookup = "SELECT 1 FROM local_events WHERE action_id = ?1";
            let exists: Option<bool> = tx
                .query_row(lookup, rusqlite::params![intent.action_id], |r| r.get(0))
                .optional()
                .am("local event intent lookup")?;
            if exists.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "local event action `{}` already has an intent record",
                        intent.action_id
                    ),
                ));
            }
            tx.execute(
                "INSERT INTO local_events
                    (action_id, seq, actor, scope, kind, event_time, outcome_state, result)
                 SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3, ?4, ?5, 'pending', NULL
                 FROM local_events",
                rusqlite::params![
                    intent.action_id,
                    intent.actor,
                    intent.scope,
                    intent.kind,
                    intent.event_time
                ],
            )
            .am("local event intent insert")?;
            tx.commit().am("local event commit")
        })
    }

    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        let state = outcome_state(denied);
        self.with_conn(|conn| {
            let tx = conn.transaction().am("local event begin")?;
            let updated = tx
                .execute(
                    "UPDATE local_events SET outcome_state = ?1, result = ?2
                     WHERE action_id = ?3 AND outcome_state = 'pending'",
                    rusqlite::params![state.as_db(), result, action_id],
                )
                .am("local event outcome update")?;
            if updated == 0 {
                let lookup = "SELECT 1 FROM local_events WHERE action_id = ?1";
                let exists: Option<bool> = tx
                    .query_row(lookup, rusqlite::params![action_id], |r| r.get(0))
                    .optional()
                    .am("local event outcome lookup")?;
                // Duplicate-tolerant: a completed row stays; a missing row means no intent.
                if exists.is_none() {
                    return Err(unknown_action(action_id));
                }
            }
            tx.commit().am("local event commit")
        })
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT action_id FROM local_events WHERE outcome_state = 'pending' ORDER BY seq")
                .am("local event pending prepare")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .am("local event pending rows")?;
            rows.collect::<Result<Vec<_>, _>>().am("local event pending rows")
        })
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT action_id, seq, actor, scope, kind, event_time,
                            outcome_state, result
                     FROM local_events ORDER BY seq",
                )
                .am("local event list prepare")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                })
                .am("local event list rows")?;
            let mut events = Vec::new();
            for row in rows {
                let (action_id, seq, actor, scope, kind, event_time, state, result) =
                    row.am("local event list row")?;
                events.push(LocalEventView {
                    action_id,
                    seq,
                    actor,
                    scope,
                    kind,
                    event_time,
                    state: LocalEventState::from_db(&state)?,
                    result,
                });
            }
            Ok(events)
        })
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM local_events WHERE outcome_state = 'allowed' AND event_time < ?1",
                    rusqlite::params![cutoff],
                )
                .am("local event prune")?;
            Ok(n)
        })
    }
}

impl LocalEventStore for PostgresStore {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        validate_intent(intent)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("local event begin")?;
            let lookup = "SELECT 1 FROM local_events WHERE action_id = $1";
            let existing = tx
                .query_opt(lookup, &[&intent.action_id])
                .am("local event intent lookup")?;
            if existing.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "local event action `{}` already has an intent record",
                        intent.action_id
                    ),
                ));
            }
            tx.execute(
                "INSERT INTO local_events
                    (action_id, seq, actor, scope, kind, event_time, outcome_state, result)
                 SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4, $5, 'pending', NULL
                 FROM local_events",
                &[
                    &intent.action_id,
                    &intent.actor,
                    &intent.scope,
                    &intent.kind,
                    &intent.event_time,
                ],
            )
            .am("local event intent insert")?;
            tx.commit().am("local event commit")
        })
    }

    fn record_outcome(&self, action_id: &str, denied: bool, result: &str) -> Result<(), AmError> {
        validate_action_id(action_id)?;
        let state = outcome_state(denied);
        self.with_client(|client| {
            let mut tx = client.transaction().am("local event begin")?;
            let updated = tx
                .execute(
                    "UPDATE local_events SET outcome_state = $1, result = $2
                     WHERE action_id = $3 AND outcome_state = 'pending'",
                    &[&state.as_db(), &result, &action_id],
                )
                .am("local event outcome update")?;
            if updated == 0 {
                let lookup = "SELECT 1 FROM local_events WHERE action_id = $1";
                let existing = tx
                    .query_opt(lookup, &[&action_id])
                    .am("local event outcome lookup")?;
                // Duplicate-tolerant: a completed row stays; a missing row means no intent.
                if existing.is_none() {
                    return Err(unknown_action(action_id));
                }
            }
            tx.commit().am("local event commit")
        })
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query("SELECT action_id FROM local_events WHERE outcome_state = 'pending' ORDER BY seq", &[])
                .am("local event pending rows")?;
            Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
        })
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT action_id, seq, actor, scope, kind, event_time,
                            outcome_state, result
                     FROM local_events ORDER BY seq",
                    &[],
                )
                .am("local event list rows")?;
            rows.iter()
                .map(|row| {
                    Ok(LocalEventView {
                        action_id: row.get(0),
                        seq: row.get(1),
                        actor: row.get(2),
                        scope: row.get(3),
                        kind: row.get(4),
                        event_time: row.get(5),
                        state: LocalEventState::from_db(&row.get::<_, String>(6))?,
                        result: row.get(7),
                    })
                })
                .collect()
        })
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.with_client(|client| {
            let n = client
                .execute(
                    "DELETE FROM local_events WHERE outcome_state = 'allowed' AND event_time < $1",
                    &[&cutoff],
                )
                .am("local event prune")?;
            Ok(usize::try_from(n).unwrap_or(0))
        })
    }
}

#[cfg(test)]
#[path = "local_events_tests.rs"]
mod local_events_tests;
