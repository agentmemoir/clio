// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres hygiene persistence:
//! hygiene flag writes, archive marker writes, the retrieve-eligibility probe,
//! and the durable `hygiene_audit_logs` table, each mutation paired with an
//! `audit_events` row.
//!
//! ## Owns
//! - Postgres implementations of the [`crate::HygieneStore`] trait
//!
//! ## Does not own
//! - Tool-level confirmation gating or noise scoring (see `clio-hygiene`)
//! - SQLite hygiene SQL (see `sqlite_hygiene`)
//!
//! ## Boundary
//!
//! Changes to Postgres hygiene SQL should normally require changes here.
//! Changes to SQLite hygiene SQL should not require changes here.

use crate::hygiene_store::{HygieneLogEntry, validate_log_action};
use crate::migrate::{IntoAmError, now_iso};
use crate::model::ItemIdentity;
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};

impl PostgresStore {
    pub(crate) fn item_is_archived_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<bool, AmError> {
        self.with_client(|client| {
            let n: i64 = client
                .query_one(
                    "SELECT COUNT(*)::bigint FROM items
                     WHERE id = $1 AND bank_id = $2 AND archived_at IS NOT NULL",
                    &[&item_id, &bank_id],
                )
                .am("hygiene archived probe")?
                .get(0);
            Ok(n > 0)
        })
    }

    pub(crate) fn set_hygiene_flag_impl(
        &self,
        bank_id: &str,
        item_id: &str,
        note: Option<&str>,
        actor: &str,
        timestamp: &str,
    ) -> Result<ItemIdentity, AmError> {
        hygiene_item_tx(self, bank_id, item_id, actor, "hygiene_flag", |tx| {
            let n = tx
                .execute(
                    "UPDATE items SET hygiene_flag = $1, updated_at = $2
                     WHERE id = $3 AND bank_id = $4 AND discarded_at IS NULL",
                    &[&note, &timestamp, &item_id, &bank_id],
                )
                .am("hygiene flag update")?;
            Ok(n)
        })
    }

    pub(crate) fn archive_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
        archived_at: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError> {
        hygiene_item_tx(self, bank_id, item_id, actor, "archive", |tx| {
            let n = tx
                .execute(
                    "UPDATE items SET archived_at = $1, updated_at = $2
                     WHERE id = $3 AND bank_id = $4 AND discarded_at IS NULL
                       AND archived_at IS NULL",
                    &[&archived_at, &now_iso(), &item_id, &bank_id],
                )
                .am("hygiene archive update")?;
            Ok(n)
        })
    }

    pub(crate) fn unarchive_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError> {
        hygiene_item_tx(self, bank_id, item_id, actor, "unarchive", |tx| {
            let n = tx
                .execute(
                    "UPDATE items SET archived_at = NULL, updated_at = $1
                     WHERE id = $2 AND bank_id = $3 AND archived_at IS NOT NULL",
                    &[&now_iso(), &item_id, &bank_id],
                )
                .am("hygiene unarchive update")?;
            Ok(n)
        })
    }

    pub(crate) fn insert_hygiene_log_impl(&self, entry: &HygieneLogEntry) -> Result<(), AmError> {
        validate_log_action(&entry.action)?;
        self.with_client(|client| {
            client
                .execute(
                    "INSERT INTO hygiene_audit_logs (
                        id, bank_id, item_id, noise_score, action, actor,
                        confirmed, dry_run, detail_json, created_at
                     ) VALUES ($1, $2, $3, $4::float8, $5, $6, $7, $8, $9, $10)",
                    &[
                        &entry.id,
                        &entry.bank_id,
                        &entry.item_id,
                        &entry.noise_score,
                        &entry.action,
                        &entry.actor,
                        &i32::from(entry.confirmed),
                        &i32::from(entry.dry_run),
                        &entry.detail.to_string(),
                        &entry.created_at,
                    ],
                )
                .am("hygiene log insert")?;
            Ok(())
        })
    }

    pub(crate) fn list_hygiene_logs_impl(
        &self,
        bank_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<HygieneLogEntry>, AmError> {
        hygiene_log_scope(limit)?;
        let limit_i = i64::try_from(limit).unwrap_or(i64::MAX);
        let offset_i = i64::try_from(offset).unwrap_or(i64::MAX);
        self.with_client(|client| {
            let rows = match bank_id.filter(|b| !b.is_empty()) {
                Some(bank) => client
                    .query(
                        "SELECT id, bank_id, item_id, noise_score::float8, action, actor,
                                confirmed::int8, dry_run::int8, detail_json, created_at
                         FROM hygiene_audit_logs WHERE bank_id = $1
                         ORDER BY created_at DESC, id DESC
                         LIMIT $2 OFFSET $3",
                        &[&bank, &limit_i, &offset_i],
                    )
                    .am("hygiene log rows")?,
                None => client
                    .query(
                        "SELECT id, bank_id, item_id, noise_score::float8, action, actor,
                                confirmed::int8, dry_run::int8, detail_json, created_at
                         FROM hygiene_audit_logs
                         ORDER BY created_at DESC, id DESC
                         LIMIT $1 OFFSET $2",
                        &[&limit_i, &offset_i],
                    )
                    .am("hygiene log rows")?,
            };
            Ok(rows
                .iter()
                .map(|r| HygieneLogEntry {
                    id: r.get(0),
                    bank_id: r.get(1),
                    item_id: r.get(2),
                    noise_score: r.get(3),
                    action: r.get(4),
                    actor: r.get(5),
                    confirmed: r.get::<_, i64>(6) != 0,
                    dry_run: r.get::<_, i64>(7) != 0,
                    detail: serde_json::from_str(&r.get::<_, String>(8))
                        .unwrap_or(serde_json::Value::Null),
                    created_at: r.get(9),
                })
                .collect())
        })
    }
}

/// Shared hygiene-mutation transaction: pre-check identity, apply the update
/// through `apply`, and record the audit row in the same transaction.
///
/// `apply` returns the affected row count; a zero count after the live
/// pre-check means a concurrent writer won the race (or the item is not in the
/// required state), surfaced as the documented repeat-action rejection.
fn hygiene_item_tx(
    store: &PostgresStore,
    bank_id: &str,
    item_id: &str,
    actor: &str,
    operation: &'static str,
    apply: impl Fn(&mut postgres::Transaction<'_>) -> Result<u64, AmError>,
) -> Result<ItemIdentity, AmError> {
    if bank_id.is_empty() || item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "bank_id and item_id are required",
        ));
    }
    if actor.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "actor is required on hygiene writes",
        ));
    }
    store.with_client(|client| {
        let mut tx = client.transaction().am("hygiene begin")?;
        let row = tx
            .query_opt(
                "SELECT category, epistemic_kind FROM items
                 WHERE id = $1 AND bank_id = $2 AND discarded_at IS NULL",
                &[&item_id, &bank_id],
            )
            .am("hygiene lookup")?
            .map(|r| (r.get::<_, Option<String>>(0), r.get::<_, String>(1)));
        let Some((category, epistemic_kind)) = row else {
            return Err(AmError::new(
                ErrorCode::ItemNotFound,
                format!("item `{item_id}` not found in bank"),
            ));
        };
        if apply(&mut tx)? == 0 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("item `{item_id}` is not in a state that allows `{operation}`"),
            ));
        }
        let ts = now_iso();
        tx.execute(
            "INSERT INTO audit_events (
                id, bank_id, operation, item_id, category, epistemic_kind,
                confidence, admission_score, actor, hygiene_action, detail_json, created_at
             ) VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, $7, $8, '{}', $9)",
            &[
                &format!(
                    "audit-{operation}-{item_id}-{ts}-{}",
                    crate::hygiene_seq::next(),
                ),
                &bank_id,
                &operation,
                &item_id,
                &category,
                &epistemic_kind,
                &actor,
                &operation,
                &ts,
            ],
        )
        .am("hygiene audit insert")?;
        tx.commit().am("hygiene commit")?;
        Ok(ItemIdentity {
            item_id: item_id.to_owned(),
            category,
            epistemic_kind,
        })
    })
}

/// Range-validate the log-list limit (shared with the SQLite driver).
fn hygiene_log_scope(limit: usize) -> Result<(), AmError> {
    if limit == 0 || limit > 200 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("hygiene log limit must be in 1..=200, got {limit}"),
        ));
    }
    Ok(())
}

impl crate::HygieneStore for PostgresStore {
    fn set_hygiene_flag(
        &self,
        bank_id: &str,
        item_id: &str,
        note: Option<&str>,
        actor: &str,
        timestamp: &str,
    ) -> Result<ItemIdentity, AmError> {
        self.set_hygiene_flag_impl(bank_id, item_id, note, actor, timestamp)
    }

    fn archive_item(
        &self,
        bank_id: &str,
        item_id: &str,
        archived_at: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError> {
        self.archive_item_impl(bank_id, item_id, archived_at, actor)
    }

    fn unarchive_item(
        &self,
        bank_id: &str,
        item_id: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError> {
        self.unarchive_item_impl(bank_id, item_id, actor)
    }

    fn insert_hygiene_log(&self, entry: &HygieneLogEntry) -> Result<(), AmError> {
        self.insert_hygiene_log_impl(entry)
    }

    fn list_hygiene_logs(
        &self,
        bank_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<HygieneLogEntry>, AmError> {
        self.list_hygiene_logs_impl(bank_id, limit, offset)
    }
}

#[cfg(test)]
#[path = "postgres_hygiene_tests.rs"]
mod postgres_hygiene_tests;
