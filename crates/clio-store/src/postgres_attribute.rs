// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres attribute-schema and continuous-state persistence.
//!
//! ## Owns
//! - `put/get_attribute_schema`, `observe_continuous`, `continuous_series` on Postgres
//! - Advisory-lock serialized observe RMW (`FOR UPDATE` row lock + xact advisory lock)
//!
//! ## Does not own
//! - formula math (see `attribute_map` / `clio-types`)
//! - SQLite locking or statements (see `sqlite_attribute`)
//!
//! ## Boundary
//!
//! Changes to Postgres continuous SQL or locking should normally require changes here.
//! Changes to formula math or the mutator facade should not require changes here.
//!
//! Keep this module focused on Postgres attribute-schema and continuous-state persistence.

use crate::attribute_map::{
    SchemaCols, audit_ts, cols_to_schema, compute_next, parse_window, rebuild_window_from_points,
    window_json,
};
use crate::dek::DekProvider;
use crate::item_persist::require_actor;
use crate::migrate::IntoAmError;
use crate::persona_store::open_persona_context;
use crate::postgres::PostgresStore;
use clio_types::{
    AmError, AttributeSchema, ContinuousPoint, ContinuousState, ErrorCode, UpdateRule,
};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic suffix so two observations in the same nanosecond cannot collide.
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

const SCHEMA_COLUMNS: &str = "bank_id, attr_key, attr_class, alpha::float8, trend_window,
                   bounds_lo::float8, bounds_hi::float8, clamp_values, created_at, updated_at";

fn schema_cols(r: &postgres::Row) -> SchemaCols {
    // trend_window is stored as INT4; widen to the portable Option<i64> shape.
    let window = r.get::<_, Option<i32>>(4).map(i64::from);
    (
        r.get(0),
        r.get(1),
        r.get(2),
        r.get(3),
        window,
        r.get(5),
        r.get(6),
        r.get::<_, i32>(7) != 0,
        r.get(8),
        r.get(9),
    )
}

fn map_schema(r: &postgres::Row) -> Result<AttributeSchema, AmError> {
    cols_to_schema(schema_cols(r))
}

impl PostgresStore {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn put_attribute_schema_impl(
        &self,
        schema: &AttributeSchema,
        actor: &str,
    ) -> Result<(), AmError> {
        schema.validate()?;
        require_actor(actor)?;
        let clamp = i32::from(schema.clamp_values);
        let rule = schema.update_rule.as_str();
        let window: Option<i32> = schema
            .trend_window
            .map(|n| i32::try_from(n).unwrap_or(i32::MAX));
        self.with_client(|client| {
            let existing = client
                .query_opt(
                    "SELECT attr_class FROM attribute_schemas
                     WHERE bank_id = $1 AND attr_key = $2",
                    &[&schema.bank_id, &schema.attr_key],
                )
                .am("attribute schema lookup")?;
            if let Some(row) = &existing {
                let class: String = row.get(0);
                if UpdateRule::parse(&class)? != schema.update_rule {
                    return Err(AmError::new(
                        ErrorCode::Forbidden,
                        format!(
                            "attribute `{}` is declared `{class}`; update_rule is immutable \
                             (explicit schema-change path required)",
                            schema.attr_key
                        ),
                    ));
                }
            }
            client
                .execute(
                    "INSERT INTO attribute_schemas (
                        bank_id, attr_key, attr_class, alpha, trend_window,
                        bounds_lo, bounds_hi, clamp_values, created_at, updated_at
                     ) VALUES ($1, $2, $3, $4::float8, $5, $6::float8, $7::float8, $8, $9, $10)
                     ON CONFLICT (bank_id, attr_key) DO UPDATE SET
                       alpha = EXCLUDED.alpha,
                       trend_window = EXCLUDED.trend_window,
                       bounds_lo = EXCLUDED.bounds_lo,
                       bounds_hi = EXCLUDED.bounds_hi,
                       clamp_values = EXCLUDED.clamp_values,
                       updated_at = EXCLUDED.updated_at",
                    &[
                        &schema.bank_id,
                        &schema.attr_key,
                        &rule,
                        &schema.alpha,
                        &window,
                        &schema.bounds_lo,
                        &schema.bounds_hi,
                        &clamp,
                        &schema.created_at,
                        &schema.updated_at,
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
        self.with_client(|client| {
            let row = client
                .query_opt(
                    &format!(
                        "SELECT {SCHEMA_COLUMNS} FROM attribute_schemas
                     WHERE bank_id = $1 AND attr_key = $2"
                    ),
                    &[&bank_id, &attr_key],
                )
                .am("attribute schema read")?;
            row.map(|r| map_schema(&r)).transpose()
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("continuous observe begin")?;
            // Per-key advisory lock serializes cold-start + RMW for concurrent clients.
            tx.execute(
                "SELECT pg_advisory_xact_lock(hashtext($1 || ':' || $2))",
                &[&bank_id, &attr_key],
            )
            .am("continuous observe lock")?;
            let schema = tx
                .query_opt(
                    &format!(
                        "SELECT {SCHEMA_COLUMNS} FROM attribute_schemas
                              WHERE bank_id = $1 AND attr_key = $2"
                    ),
                    &[&bank_id, &attr_key],
                )
                .am("attribute schema read")?
                .map(|r| map_schema(&r))
                .transpose()?
                .ok_or_else(|| {
                    AmError::new(
                        ErrorCode::WrongUpdateRule,
                        format!(
                            "attribute `{attr_key}` has no update_rule declaration in this bank"
                        ),
                    )
                })?;
            let trend_window = schema
                .trend_window
                .unwrap_or(clio_types::DEFAULT_TREND_WINDOW);
            let row: Option<String> = tx
                .query_opt(
                    "SELECT window_json FROM continuous_state
                     WHERE bank_id = $1 AND attr_key = $2 FOR UPDATE",
                    &[&bank_id, &attr_key],
                )
                .am("continuous state read")?
                .map(|r| r.get(0));
            let prior_window =
                if let Some(window) = row.as_ref().and_then(|w| parse_window(w, trend_window)) {
                    window
                } else {
                    let rows = tx
                        .query(
                            "SELECT as_of, state::float8, trend::float8 FROM continuous_observations
                             WHERE bank_id = $1 AND attr_key = $2
                             ORDER BY as_of DESC, id DESC LIMIT $3",
                            &[&bank_id, &attr_key, &i64::from(trend_window)],
                        )
                        .am("continuous series query")?;
                    let mut points: Vec<ContinuousPoint> = rows
                        .iter()
                        .map(|r| ContinuousPoint {
                            as_of: r.get(0),
                            state: r.get(1),
                            trend: r.get(2),
                            context: None,
                        })
                        .collect();
                    points.reverse();
                    rebuild_window_from_points(points, usize::try_from(trend_window).unwrap_or(1))
                };
            let next = compute_next(&schema, &prior_window, value, as_of)?;
            tx.execute(
                "INSERT INTO continuous_state (
                    bank_id, attr_key, state, trend, as_of, window_json, created_at, updated_at
                 ) VALUES ($1, $2, $3::float8, $4::float8, $5, $6, $7, $7)
                 ON CONFLICT (bank_id, attr_key) DO UPDATE SET
                   state = EXCLUDED.state,
                   trend = EXCLUDED.trend,
                   as_of = EXCLUDED.as_of,
                   window_json = EXCLUDED.window_json,
                   updated_at = EXCLUDED.updated_at",
                &[
                    &bank_id,
                    &attr_key,
                    &next.state,
                    &next.trend,
                    &next.as_of,
                    &window_json(&next.window),
                    &audit_ts(),
                ],
            )
            .am("continuous state upsert")?;
            let obs_id = unique_obs_id(attr_key, as_of);
            tx.execute(
                "INSERT INTO continuous_observations (
                    id, bank_id, attr_key, observed_value, state, trend, as_of, actor, created_at
                 ) VALUES ($1, $2, $3, $4::float8, $5::float8, $6::float8, $7, $8, $9)",
                &[
                    &obs_id,
                    &bank_id,
                    &attr_key,
                    &value,
                    &next.state,
                    &next.trend,
                    &next.as_of,
                    &actor,
                    &audit_ts(),
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
                 ) VALUES ($1, $2, $3, NULL, NULL, NULL,
                           NULL, NULL, $4, $5, $6)",
                &[
                    &format!("audit-{obs_id}"),
                    &bank_id,
                    &operation,
                    &actor,
                    &detail,
                    &audit_ts(),
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
        self.with_client(|client| {
            let stored = stored_preference_context(client, self.kms().as_ref(), bank_id, attr_key)?;
            let rows = client
                .query(
                    "SELECT as_of, state::float8, trend::float8 FROM continuous_observations
                     WHERE bank_id = $1 AND attr_key = $2
                       AND ($3::text IS NULL OR as_of <= $3)
                     ORDER BY as_of ASC, id ASC
                     LIMIT $4",
                    &[&bank_id, &attr_key, &as_of, &capped],
                )
                .am("continuous series query")?;
            Ok(rows
                .iter()
                .map(|r| {
                    let as_of: String = r.get(0);
                    let context = stored
                        .as_ref()
                        .filter(|(_, eff)| as_of.as_str() >= eff.as_str())
                        .map(|(text, _)| text.clone());
                    ContinuousPoint {
                        as_of,
                        state: r.get(1),
                        trend: r.get(2),
                        context,
                    }
                })
                .collect())
        })
    }
}

/// Open the stored persona-preference context for one key, tolerantly: no row,
/// no sealed value, or an unreadable body all yield `None` so the series read
/// keeps working after erasure (same policy as persona reads). Returns the
/// opened text plus its effective time (`context_as_of`, or the row's `as_of`
/// for a legacy row without the column).
fn stored_preference_context(
    client: &mut postgres::Client,
    kms: &dyn DekProvider,
    bank_id: &str,
    attr_key: &str,
) -> Result<Option<(String, String)>, AmError> {
    let row: Option<(Option<String>, Option<String>, String)> = client
        .query_opt(
            "SELECT context_cipher, context_as_of, as_of FROM persona_preferences
             WHERE bank_id = $1 AND attr_key = $2",
            &[&bank_id, &attr_key],
        )
        .am("preference context read")?
        .map(|r| (r.get(0), r.get(1), r.get(2)));
    let Some((cipher, context_as_of, row_as_of)) = row else {
        return Ok(None);
    };
    let Some(text) = open_persona_context(kms, cipher.as_deref()) else {
        return Ok(None);
    };
    Ok(Some((text, context_as_of.unwrap_or(row_as_of))))
}

/// Store-trait wiring for the continuous-attribute surface (Postgres).
///
/// Disjoint from the main `Store` impl in `postgres_store` (a separate trait).
impl crate::continuous::ContinuousStore for PostgresStore {
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

#[cfg(test)]
#[path = "postgres_attribute_tests.rs"]
mod postgres_attribute_tests;
