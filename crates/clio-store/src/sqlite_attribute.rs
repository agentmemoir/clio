// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite attribute-schema and continuous-state persistence.
//!
//! ## Owns
//! - `put/get_attribute_schema`, `observe_continuous`, `continuous_series` on SQLite
//! - Serialized observe RMW (immediate transaction over the single connection)
//!
//! ## Does not own
//! - formulas (see `attribute_map` / `clio-types`)
//! - The preference-context lookup for the series read (`sqlite_attribute_context`)
//! - Postgres locking or statements (see `postgres_attribute`)
//!
//! ## Boundary
//!
//! Changes to SQLite continuous SQL should normally require changes here.
//! Changes to formula math or the mutator facade should not require changes here.
//!
//! Keep this module focused on SQLite attribute-schema and continuous-state persistence.

use crate::attribute_map::{
    audit_ts, cols_to_schema, compute_next, parse_window, rebuild_window_from_points, window_json,
};
use crate::item_persist::require_actor;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{
    AmError, AttributeSchema, ContinuousPoint, ContinuousState, ErrorCode, UpdateRule,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic suffix so two observations in the same millisecond cannot collide.
static OBS_ID_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_obs_id(attr_key: &str, as_of: &str) -> String {
    let seq = OBS_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "obs-{attr_key}-{as_of}-{nanos}-{seq}-{}",
        std::process::id()
    )
}

impl SqliteStore {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn put_attribute_schema_impl(
        &self,
        schema: &AttributeSchema,
        actor: &str,
    ) -> Result<(), AmError> {
        schema.validate()?;
        require_actor(actor)?;
        let clamp = i64::from(schema.clamp_values);
        self.with_conn(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT attr_class FROM attribute_schemas
                     WHERE bank_id = ?1 AND attr_key = ?2",
                    params![schema.bank_id, schema.attr_key],
                    |r| r.get(0),
                )
                .optional()
                .am("attribute schema lookup")?;
            if let Some(class) = &existing
                && UpdateRule::parse(class)? != schema.update_rule
            {
                return Err(AmError::new(
                    ErrorCode::Forbidden,
                    format!(
                        "attribute `{}` is declared `{class}`; update_rule is immutable \
                             (explicit schema-change path required)",
                        schema.attr_key
                    ),
                ));
            }
            conn.execute(
                "INSERT INTO attribute_schemas (
                    bank_id, attr_key, attr_class, alpha, trend_window,
                    bounds_lo, bounds_hi, clamp_values, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(bank_id, attr_key) DO UPDATE SET
                   alpha = excluded.alpha,
                   trend_window = excluded.trend_window,
                   bounds_lo = excluded.bounds_lo,
                   bounds_hi = excluded.bounds_hi,
                   clamp_values = excluded.clamp_values,
                   updated_at = excluded.updated_at",
                params![
                    schema.bank_id,
                    schema.attr_key,
                    schema.update_rule.as_str(),
                    schema.alpha,
                    schema.trend_window.map(i64::from),
                    schema.bounds_lo,
                    schema.bounds_hi,
                    clamp,
                    schema.created_at,
                    schema.updated_at,
                ],
            )
            .am("attribute schema upsert")?;
            Ok(())
        })
    }

    pub(crate) fn get_attribute_schema_impl(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<AttributeSchema>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT bank_id, attr_key, attr_class, alpha, trend_window,
                        bounds_lo, bounds_hi, clamp_values, created_at, updated_at
                 FROM attribute_schemas WHERE bank_id = ?1 AND attr_key = ?2",
                params![bank_id, attr_key],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get::<_, i64>(7)? != 0,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .optional()
            .am("attribute schema read")?
            .map(cols_to_schema)
            .transpose()
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn observe_continuous_impl(
        &self,
        bank_id: &str,
        attr_key: &str,
        value: f64,
        as_of: &str,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<ContinuousState, AmError> {
        if bank_id.is_empty() || attr_key.is_empty() || as_of.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id, attr_key, and as_of are required",
            ));
        }
        require_actor(actor)?;
        self.with_conn(|conn| {
            // Immediate lock serializes same-key RMW before any read.
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .am("continuous observe begin")?;
            let schema: Option<AttributeSchema> = tx
                .query_row(
                    "SELECT bank_id, attr_key, attr_class, alpha, trend_window,
                            bounds_lo, bounds_hi, clamp_values, created_at, updated_at
                     FROM attribute_schemas WHERE bank_id = ?1 AND attr_key = ?2",
                    params![bank_id, attr_key],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get::<_, i64>(7)? != 0,
                            r.get(8)?,
                            r.get(9)?,
                        ))
                    },
                )
                .optional()
                .am("attribute schema read")?
                .map(cols_to_schema)
                .transpose()?;
            let schema = schema.ok_or_else(|| {
                AmError::new(
                    ErrorCode::WrongUpdateRule,
                    format!("attribute `{attr_key}` has no update_rule declaration in this bank"),
                )
            })?;
            let trend_window = schema
                .trend_window
                .unwrap_or(clio_types::DEFAULT_TREND_WINDOW);
            let row: Option<String> = tx
                .query_row(
                    "SELECT window_json FROM continuous_state
                     WHERE bank_id = ?1 AND attr_key = ?2",
                    params![bank_id, attr_key],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .am("continuous state read")?;
            let prior_window =
                if let Some(window) = row.as_deref().and_then(|w| parse_window(w, trend_window)) {
                    window
                } else {
                    // Missing row or corrupt window: rebuild from retained points.
                    let mut stmt = tx
                        .prepare(
                            "SELECT as_of, state, trend FROM continuous_observations
                             WHERE bank_id = ?1 AND attr_key = ?2
                             ORDER BY as_of DESC, id DESC LIMIT ?3",
                        )
                        .am("continuous series prepare")?;
                    let mut points: Vec<ContinuousPoint> = stmt
                        .query_map(params![bank_id, attr_key, trend_window], |r| {
                            Ok(ContinuousPoint {
                                as_of: r.get(0)?,
                                state: r.get(1)?,
                                trend: r.get(2)?,
                                context: None,
                            })
                        })
                        .am("continuous series query")?
                        .collect::<Result<Vec<_>, _>>()
                        .am("continuous series row")?;
                    points.reverse();
                    rebuild_window_from_points(points, usize::try_from(trend_window).unwrap_or(1))
                };
            let next = compute_next(&schema, &prior_window, value, as_of)?;
            tx.execute(
                "INSERT INTO continuous_state (
                    bank_id, attr_key, state, trend, as_of, window_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(bank_id, attr_key) DO UPDATE SET
                   state = excluded.state,
                   trend = excluded.trend,
                   as_of = excluded.as_of,
                   window_json = excluded.window_json,
                   updated_at = excluded.updated_at",
                params![
                    bank_id,
                    attr_key,
                    next.state,
                    next.trend,
                    next.as_of,
                    window_json(&next.window),
                    audit_ts(),
                ],
            )
            .am("continuous state upsert")?;
            let obs_id = unique_obs_id(attr_key, as_of);
            tx.execute(
                "INSERT INTO continuous_observations (
                    id, bank_id, attr_key, observed_value, state, trend, as_of, actor, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    obs_id,
                    bank_id,
                    attr_key,
                    value,
                    next.state,
                    next.trend,
                    next.as_of,
                    actor,
                    audit_ts(),
                ],
            )
            .am("continuous observation insert")?;
            let detail = serde_json::json!({
                "attr_key": attr_key,
                "observed_value": value,
                "state": next.state,
                "trend": next.trend,
                "as_of": next.as_of,
                "reason": reason,
            })
            .to_string();
            // A reasoned observation is a correction (the telemetry
            // record carries operation `correct`); a plain observation stays
            // `continuous_observe`.
            let operation = if reason.is_some() {
                "correct"
            } else {
                "continuous_observe"
            };
            tx.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, category, epistemic_kind,
                    confidence, admission_score, actor, detail_json, created_at
                 ) VALUES (?1, ?2, ?3, NULL, NULL, NULL,
                           NULL, NULL, ?4, ?5, ?6)",
                params![
                    format!("audit-{obs_id}"),
                    bank_id,
                    operation,
                    actor,
                    detail,
                    audit_ts()
                ],
            )
            .am("continuous audit insert")?;
            tx.commit().am("continuous observe commit")?;
            Ok(next)
        })
    }

    pub(crate) fn continuous_series_impl(
        &self,
        bank_id: &str,
        attr_key: &str,
        as_of: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuousPoint>, AmError> {
        if bank_id.is_empty() || attr_key.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and attr_key are required",
            ));
        }
        let capped = limit.min(10_000) as i64;
        // One context per preference key, effective from `context_as_of` onward
        // (not per observation): a point carries it only once its own `as_of`
        // reaches that effective time.
        let stored = self.preference_stored_context(bank_id, attr_key)?;
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT as_of, state, trend FROM continuous_observations
                     WHERE bank_id = ?1 AND attr_key = ?2
                       AND (?3 IS NULL OR as_of <= ?3)
                     ORDER BY as_of ASC, id ASC
                     LIMIT ?4",
                )
                .am("continuous series prepare")?;
            let rows = stmt
                .query_map(params![bank_id, attr_key, as_of, capped], |r| {
                    let as_of: String = r.get(0)?;
                    let context = stored
                        .as_ref()
                        .filter(|(_, eff)| as_of.as_str() >= eff.as_str())
                        .map(|(text, _)| text.clone());
                    Ok(ContinuousPoint {
                        as_of,
                        state: r.get(1)?,
                        trend: r.get(2)?,
                        context,
                    })
                })
                .am("continuous series query")?
                .collect::<Result<Vec<_>, _>>()
                .am("continuous series row")?;
            Ok(rows)
        })
    }
}

/// Store-trait wiring for the continuous-attribute surface (SQLite).
///
/// Separate from the main `Store` impl so the continuous surface stays in this module.
impl crate::continuous::ContinuousStore for SqliteStore {
    fn put_attribute_schema(
        &self,
        schema: &clio_types::AttributeSchema,
        actor: &str,
    ) -> Result<(), AmError> {
        self.put_attribute_schema_impl(schema, actor)
    }

    fn get_attribute_schema(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<clio_types::AttributeSchema>, AmError> {
        self.get_attribute_schema_impl(bank_id, attr_key)
    }

    fn observe_continuous(
        &self,
        bank_id: &str,
        attr_key: &str,
        value: f64,
        as_of: &str,
        actor: &str,
    ) -> Result<clio_types::ContinuousState, AmError> {
        self.observe_continuous_with_reason(bank_id, attr_key, value, as_of, actor, None)
    }

    fn observe_continuous_with_reason(
        &self,
        bank_id: &str,
        attr_key: &str,
        value: f64,
        as_of: &str,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<clio_types::ContinuousState, AmError> {
        self.observe_continuous_impl(bank_id, attr_key, value, as_of, actor, reason)
    }

    fn continuous_series(
        &self,
        bank_id: &str,
        attr_key: &str,
        as_of: Option<&str>,
        limit: usize,
    ) -> Result<Vec<clio_types::ContinuousPoint>, AmError> {
        self.continuous_series_impl(bank_id, attr_key, as_of, limit)
    }
}
