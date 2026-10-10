// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compiling [`ItemInspectFilter`] into a
//! bank-scoped, parameterized `WHERE` fragment shared by both storage backends.
//!
//! ## Owns
//! - Filter validation, placeholder numbering, and predicate ordering
//!
//! ## Does not own
//! - Row mapping or `LIMIT`/`OFFSET` binding (each backend binds those itself)
//!
//! ## Boundary
//!
//! Changes to inspector filter semantics should normally require changes here.
//! Changes to one backend's SQL should not require changes here.
//!
//! Keep this module focused on the shared inspector predicate compiler.

use crate::model::{InspectValidity, ItemInspectFilter};
use clio_types::{AmError, ErrorCode};

/// A compiled `WHERE` fragment plus the string parameters it references.
pub(crate) struct CompiledInspect {
    /// Predicates joined with `AND` (always non-empty: bank is required).
    pub where_sql: String,
    /// String parameter values in placeholder order.
    pub params: Vec<String>,
}

/// Compile a filter into predicates for `placeholder` (`'?'` or `'$'`).
///
/// # Errors
/// Empty bank, zero limit, or an unknown `epistemic_kind`.
pub(crate) fn compile_inspect(
    filter: &ItemInspectFilter,
    placeholder: char,
) -> Result<CompiledInspect, AmError> {
    if filter.bank_id.is_empty() {
        return Err(AmError::new(
            ErrorCode::MissingBank,
            "inspect requires a non-empty bank",
        ));
    }
    if filter.limit == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "inspect limit must be positive",
        ));
    }
    if let Some(kind) = &filter.epistemic_kind
        && kind != "fact"
        && kind != "belief"
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("epistemic_kind must be fact|belief, got `{kind}`"),
        ));
    }
    if filter.limit > 200 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("inspect limit is capped at 200, got {}", filter.limit),
        ));
    }
    let mut params: Vec<String> = Vec::new();
    let mut clauses: Vec<String> = Vec::new();
    let bind = |params: &mut Vec<String>, value: String| {
        params.push(value);
        format!("{placeholder}{}", params.len())
    };
    clauses.push(format!(
        "bank_id = {}",
        bind(&mut params, filter.bank_id.clone())
    ));
    if let Some(category) = &filter.category {
        clauses.push(format!(
            "category = {}",
            bind(&mut params, category.clone())
        ));
    }
    if let Some(kind) = &filter.epistemic_kind {
        clauses.push(format!(
            "epistemic_kind = {}",
            bind(&mut params, kind.clone())
        ));
    }
    if let Some(source) = filter.source_type {
        match source.as_str() {
            // The fact rung is exactly the `source_type IS NULL` population,
            // so it needs no bound parameter; the other rungs bind equality.
            Some(value) => clauses.push(format!(
                "source_type = {}",
                bind(&mut params, value.to_owned())
            )),
            None => clauses.push("source_type IS NULL".to_owned()),
        }
    }
    match filter.validity {
        InspectValidity::All => {}
        InspectValidity::Active => clauses.push("tx_until IS NULL".to_owned()),
        InspectValidity::Invalidated => clauses.push("tx_until IS NOT NULL".to_owned()),
        InspectValidity::Discarded => clauses.push("discarded_at IS NOT NULL".to_owned()),
    }
    if let Some(start) = &filter.time_start {
        clauses.push(format!(
            "created_at >= {}",
            bind(&mut params, start.clone())
        ));
    }
    if let Some(end) = &filter.time_end {
        clauses.push(format!("created_at < {}", bind(&mut params, end.clone())));
    }
    if !filter.include_discarded && filter.validity != InspectValidity::Discarded {
        clauses.push("discarded_at IS NULL".to_owned());
    }
    Ok(CompiledInspect {
        where_sql: clauses.join(" AND "),
        params,
    })
}

#[cfg(test)]
#[path = "inspect_query_tests.rs"]
mod inspect_query_tests;
