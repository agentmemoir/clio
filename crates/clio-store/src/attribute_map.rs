// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared continuous-attribute row mapping and
//! the serialized observe computation used by both SQL backends.
//!
//! ## Owns
//! - `attribute_schemas` / `continuous_state` / `continuous_observations` row mapping
//! - The shared observe computation (config resolve → apply_observation)
//!
//! ## Does not own
//! - Backend transactions, locking, or the `update` rule splitter
//!
//! ## Boundary
//!
//! Changes to continuous column layout or formula resolution should normally require changes here.
//! Changes to driver locking or the mutator facade should not require changes here.
//!
//! Keep this module focused on shared continuous row mapping and the observe computation.

use crate::migrate::now_iso8601;
use clio_types::{
    AmError, AttributeSchema, ContinuousEngineConfig, ContinuousPoint, ContinuousState, ErrorCode,
    UpdateRule, apply_observation,
};
use serde_json::Value;

/// Column tuple read off `attribute_schemas` (portable across both drivers).
pub(crate) type SchemaCols = (
    String,
    String,
    String,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<f64>,
    bool,
    String,
    Option<String>,
);

/// Map an `attribute_schemas` row into a validated `AttributeSchema`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn map_schema_row(
    bank_id: String,
    attr_key: String,
    attr_class: &str,
    alpha: Option<f64>,
    trend_window: Option<i64>,
    bounds_lo: Option<f64>,
    bounds_hi: Option<f64>,
    clamp_values: bool,
    created_at: String,
    updated_at: Option<String>,
) -> Result<AttributeSchema, AmError> {
    let schema = AttributeSchema {
        bank_id,
        attr_key,
        update_rule: UpdateRule::parse(attr_class)?,
        alpha,
        trend_window: trend_window.map(|n| u32::try_from(n.max(1)).unwrap_or(1)),
        bounds_lo,
        bounds_hi,
        clamp_values,
        created_at,
        updated_at,
    };
    schema.validate()?;
    Ok(schema)
}

/// Convert a raw 10-column tuple into a validated schema (driver-agnostic path).
pub(crate) fn cols_to_schema(cols: SchemaCols) -> Result<AttributeSchema, AmError> {
    map_schema_row(
        cols.0, cols.1, &cols.2, cols.3, cols.4, cols.5, cols.6, cols.7, cols.8, cols.9,
    )
}

/// Serialize the recent-state window (bounded by N) as portable JSON text.
#[must_use]
pub fn window_json(window: &[f64]) -> String {
    serde_json::to_string(window).unwrap_or_else(|_| "[]".into())
}

/// Parse a stored window. Returns `None` on parse failure **or an empty array**
/// — the caller then rebuilds the window from retained observation points
/// instead of failing the observe (failure handling: corrupt history
/// degrades, never deletes). A stored window always has ≥1 state after any
/// applied observation, so `[]` means a corrupt/truncated row, not a valid
/// cold start; rebuilding from the retained series preserves history.
#[must_use]
pub fn parse_window(raw: &str, trend_window: u32) -> Option<Vec<f64>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let arr = value.as_array()?;
    if arr.is_empty() || arr.iter().any(|v| !v.is_number()) {
        return None;
    }
    let mut window: Vec<f64> = arr
        .iter()
        .map(|v| v.as_f64().expect("checked number above"))
        .filter(|f| f.is_finite())
        .collect();
    let keep = trend_window as usize;
    if window.len() > keep {
        window.drain(..window.len() - keep);
    }
    Some(window)
}

/// Rebuild the prior window from retained observation points (corrupt-window recovery).
///
/// Panics only on a poisoned backend driver lock (same policy as the rest of the store).
pub(crate) fn rebuild_window_from_points(points: Vec<ContinuousPoint>, keep: usize) -> Vec<f64> {
    let mut window: Vec<f64> = points.into_iter().map(|p| p.state).collect();
    if window.len() > keep {
        window.drain(..window.len() - keep);
    }
    window
}

/// Compute the next continuous state from schema knobs + prior window.
///
/// Shared by both backends so the formula lives in exactly one place; drivers
/// only add locking, row I/O, and audit.
pub(crate) fn compute_next(
    schema: &AttributeSchema,
    prior_window: &[f64],
    value: f64,
    as_of: &str,
) -> Result<ContinuousState, AmError> {
    if schema.update_rule != UpdateRule::Continuous {
        return Err(AmError::new(
            ErrorCode::WrongUpdateRule,
            format!(
                "attribute `{}` is discrete; continuous EMA path cannot update it",
                schema.attr_key
            ),
        ));
    }
    let config = ContinuousEngineConfig::from_schema(schema);
    apply_observation(&config, prior_window, value, as_of)
}

/// Current-time stamp for audit rows.
pub(crate) fn audit_ts() -> String {
    now_iso8601()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(rule: UpdateRule) -> AttributeSchema {
        AttributeSchema {
            bank_id: "b".into(),
            attr_key: "affinity".into(),
            update_rule: rule,
            alpha: Some(0.5),
            trend_window: Some(3),
            bounds_lo: None,
            bounds_hi: None,
            clamp_values: false,
            created_at: "2024-01-01".into(),
            updated_at: None,
        }
    }

    #[test]
    fn window_json_roundtrip_and_corrupt_recovery() {
        let json = window_json(&[1.0, 0.5]);
        assert_eq!(parse_window(&json, 3).unwrap(), vec![1.0, 0.5]);
        assert_eq!(parse_window(&json, 1).unwrap(), vec![0.5]);
        assert!(parse_window("not json", 3).is_none());
        assert!(parse_window("[1,\"x\"]", 3).is_none());
        assert!(parse_window("[1,null]", 3).is_none());
        assert!(parse_window("[]", 3).is_none(), "empty window is corrupt");
        assert_eq!(parse_window(&json, 3), Some(vec![1.0, 0.5]));
    }

    #[test]
    fn rebuild_and_map_helpers() {
        let points = vec![
            ContinuousPoint {
                as_of: "t1".into(),
                state: 1.0,
                trend: 1.0,
                context: None,
            },
            ContinuousPoint {
                as_of: "t2".into(),
                state: 0.5,
                trend: 0.75,
                context: None,
            },
        ];
        assert_eq!(rebuild_window_from_points(points, 3), vec![1.0, 0.5]);
        assert_eq!(rebuild_window_from_points(vec![], 3), Vec::<f64>::new());
        let ok = map_schema_row(
            "b".into(),
            "affinity".into(),
            "continuous",
            Some(0.5),
            Some(3),
            None,
            None,
            false,
            "2024-01-01".into(),
            None,
        )
        .unwrap();
        assert_eq!(ok.update_rule, UpdateRule::Continuous);
        let bad_class = map_schema_row(
            "b".into(),
            "affinity".into(),
            "fact",
            None,
            None,
            None,
            None,
            false,
            "2024-01-01".into(),
            None,
        )
        .unwrap_err();
        assert_eq!(bad_class.code, ErrorCode::WrongUpdateRule);
        let negative_window = map_schema_row(
            "b".into(),
            "affinity".into(),
            "continuous",
            None,
            Some(-9),
            None,
            None,
            false,
            "2024-01-01".into(),
            None,
        )
        .unwrap();
        assert_eq!(negative_window.trend_window, Some(1));
        assert_eq!(
            compute_next(&schema(UpdateRule::Discrete), &[], 1.0, "t")
                .unwrap_err()
                .code,
            ErrorCode::WrongUpdateRule
        );
        let out = compute_next(&schema(UpdateRule::Continuous), &[], 1.0, "t").unwrap();
        assert!((out.state - 1.0).abs() < 1e-9);
        let stamp = audit_ts();
        assert!(!stamp.is_empty(), "{stamp}");
    }
}

#[cfg(test)]
#[path = "attribute_map_tests.rs"]
mod attribute_map_tests;
