// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared triple row mapping and as-of filtering.
//!
//! ## Owns
//! - Mapping SQL columns → `TripleRecord`
//! - Parameterized `WHERE` composition for `triple_query` pushdown (shared by both backends)
//! - In-memory filter for `TripleQuery` after bank-scoped fetch
//!
//! ## Does not own
//! - Backend transactions or gated write orchestration
//!
//! ## Boundary
//!
//! Changes to triple column layout or query filter semantics should normally require changes here.

use crate::migrate::now_iso8601;
use clio_types::{
    AmError, EpistemicKind, ErrorCode, SourceType, TimeAxis, TripleQuery, TripleRecord,
};
use std::fmt::Write;

/// Build a `TripleRecord` from column values.
///
/// The carrier item's sealed `context` is NOT part of this mapping: backend
/// read paths attach `record.context` from `items.content_ciphertext` via
/// `open_context` (None when absent or unreadable). The same is true of
/// `object`: the row stores the asserted VALUE sealed under the subject DEK
/// So callers open it with `triple_cipher::open_object` first and
/// pass the plaintext here.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn map_triple_row(
    id: String,
    bank_id: String,
    item_id: String,
    subject: String,
    predicate: String,
    object: String,
    epistemic_kind: String,
    source_type: Option<String>,
    confidence: Option<f64>,
    valid_from: String,
    valid_until: Option<String>,
    tx_from: String,
    tx_until: Option<String>,
    snapshot_ref: Option<String>,
    gist_ref: Option<String>,
    created_at: String,
    updated_at: Option<String>,
) -> Result<TripleRecord, AmError> {
    let source_type = match source_type.as_deref() {
        None => None,
        Some(s) => Some(SourceType::parse(s)?),
    };
    Ok(TripleRecord {
        id,
        bank_id,
        item_id,
        subject,
        predicate,
        object,
        epistemic_kind: EpistemicKind::parse(&epistemic_kind)?,
        source_type,
        confidence,
        valid_from,
        valid_until,
        tx_from,
        tx_until,
        snapshot_ref,
        gist_ref,
        context: None,
        created_at,
        updated_at,
    })
}

/// Apply pattern + as_of filter; sort by id for stable tests.
///
/// Omitting `as_of` means “current”: the chosen axis is probed at wall-clock
/// now (`now_iso8601`), so a future-dated fact (`valid_from > now`) is not
/// returned as current.
pub(crate) fn filter_triples(rows: Vec<TripleRecord>, query: &TripleQuery) -> Vec<TripleRecord> {
    let as_of = query.as_of.clone().unwrap_or_else(now_iso8601);
    let mut out: Vec<TripleRecord> = rows
        .into_iter()
        .filter(|t| query.subject.as_ref().is_none_or(|s| &t.subject == s))
        .filter(|t| query.predicate.as_ref().is_none_or(|p| &t.predicate == p))
        .filter(|t| query.object.as_ref().is_none_or(|o| &t.object == o))
        .filter(|t| t.visible_at(query.time_axis, &as_of))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Ordered optional `subject`/`predicate` equality constraints for SQL pushdown.
///
/// Returning fixed `t.`-qualified column names + caller-owned values lets each
/// backend build a parameterized `WHERE` that uses the triple pattern indexes
/// instead of loading every bank row and filtering in memory. Callers must
/// alias `triples` as `t` so the names bind (the context read LEFT JOINs
/// `items` as `i`, which shares several column names).
///
/// `object` is deliberately absent: the asserted value is sealed under the
/// subject DEK, so SQL cannot compare it. An `object` filter is applied by
/// [`filter_triples`] after each row's value is opened, which is the same
/// tolerant path `context` already takes.
pub(crate) fn pattern_constraints(query: &TripleQuery) -> Vec<(&'static str, &str)> {
    let mut out = Vec::new();
    if let Some(subject) = query.subject.as_deref() {
        out.push(("t.subject", subject));
    }
    if let Some(predicate) = query.predicate.as_deref() {
        out.push(("t.predicate", predicate));
    }
    out
}

/// Compose the parameterized `WHERE` clause for `triple_query` pushdown.
///
/// Owns the full predicate: bank scope, optional SPO equality, and the
/// half-open axis-interval visibility (`from <= as_of AND (until IS NULL OR
/// as_of < until)`) with `as_of` defaulting to wall-clock now. `ph(i)` renders
/// the 1-based placeholder for the backend dialect (`?i` for SQLite, `$i` for
/// Postgres), so the interval algebra is written once and both backends use
/// their interval indexes instead of scanning every bank row.
///
/// Column names are `t.`-qualified: callers must alias `triples` as `t` (the
/// context read LEFT JOINs `items` as `i`, which shares several column names).
pub(crate) fn query_where_clause(
    query: &TripleQuery,
    ph: impl Fn(usize) -> String,
) -> (String, Vec<String>) {
    let mut values = vec![query.bank_id.clone()];
    let mut clause = format!("t.bank_id = {}", ph(1));
    for (column, value) in pattern_constraints(query) {
        values.push(value.to_owned());
        let _ = write!(clause, " AND {column} = {}", ph(values.len()));
    }
    let (from_col, until_col) = match query.time_axis {
        TimeAxis::Valid => ("t.valid_from", "t.valid_until"),
        TimeAxis::Transaction => ("t.tx_from", "t.tx_until"),
    };
    let as_of = query.as_of.clone().unwrap_or_else(now_iso8601);
    values.push(as_of.clone());
    let from_ph = ph(values.len());
    values.push(as_of);
    let until_ph = ph(values.len());
    let _ = write!(
        clause,
        " AND {from_col} <= {from_ph} AND ({until_col} IS NULL OR {until_ph} < {until_col})"
    );
    (clause, values)
}

/// Reject continuous attribute class lookups used by the discrete path.
pub(crate) fn wrong_update_rule_attr(attr: &str) -> AmError {
    AmError::new(
        ErrorCode::WrongUpdateRule,
        format!(
            "attribute `{attr}` is continuous; discrete triple/invalidation path cannot update it"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clio_types::{EpistemicKind, TimeAxis, TripleRecord};

    fn sample(id: &str, object: &str) -> TripleRecord {
        TripleRecord {
            id: id.into(),
            bank_id: "b".into(),
            item_id: "i".into(),
            subject: "Alice".into(),
            predicate: "lives_in".into(),
            object: object.into(),
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            valid_from: "2024-01-01".into(),
            valid_until: None,
            tx_from: "2024-01-02".into(),
            tx_until: None,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            created_at: "2024-01-02".into(),
            updated_at: None,
        }
    }

    #[test]
    fn map_filter_and_wrong_rule_helpers() {
        let err = wrong_update_rule_attr("affinity");
        assert_eq!(err.code, ErrorCode::WrongUpdateRule);
        assert!(
            map_triple_row(
                "e".into(),
                "b".into(),
                "i".into(),
                "s".into(),
                "p".into(),
                "o".into(),
                "nope".into(),
                None,
                None,
                "v".into(),
                None,
                "t".into(),
                None,
                None,
                None,
                "c".into(),
                None,
            )
            .is_err()
        );
        assert!(
            map_triple_row(
                "e".into(),
                "b".into(),
                "i".into(),
                "s".into(),
                "p".into(),
                "o".into(),
                "belief".into(),
                Some("bad_src".into()),
                None,
                "v".into(),
                None,
                "t".into(),
                None,
                None,
                None,
                "c".into(),
                None,
            )
            .is_err()
        );
        let ok = map_triple_row(
            "e".into(),
            "b".into(),
            "i".into(),
            "s".into(),
            "p".into(),
            "o".into(),
            "belief".into(),
            Some("user_stated".into()),
            Some(0.5),
            "v".into(),
            None,
            "t".into(),
            None,
            None,
            None,
            "c".into(),
            Some("u".into()),
        )
        .unwrap();
        assert_eq!(ok.epistemic_kind, EpistemicKind::Belief);
        let rows = vec![sample("e2", "NYC"), sample("e1", "Boston")];
        let filtered = filter_triples(
            rows,
            &TripleQuery {
                bank_id: "b".into(),
                subject: Some("Alice".into()),
                predicate: Some("lives_in".into()),
                object: Some("Boston".into()),
                as_of: None,
                time_axis: TimeAxis::Transaction,
            },
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "e1");
    }

    #[test]
    fn query_where_clause_pushes_pattern_and_interval_predicates() {
        let q = TripleQuery {
            bank_id: "b".into(),
            subject: Some("Alice".into()),
            predicate: None,
            object: Some("Boston".into()),
            as_of: Some("2026-01-01".into()),
            time_axis: TimeAxis::Valid,
        };
        let (sql, values) = query_where_clause(&q, |i| format!("?{i}"));
        // `object` is sealed, so it is not a SQL constraint: it is filtered in
        // memory by `filter_triples` after each value is opened.
        assert_eq!(values, vec!["b", "Alice", "2026-01-01", "2026-01-01"]);
        assert!(sql.contains("t.bank_id = ?1"));
        assert!(sql.contains("t.subject = ?2"));
        assert!(!sql.contains("t.object ="));
        assert!(sql.contains("t.valid_from <= ?3"));
        assert!(sql.contains("t.valid_until IS NULL OR ?4 < t.valid_until"));
        assert!(!sql.contains("tx_from"));
        let (tx_sql, tx_values) = query_where_clause(
            &TripleQuery {
                time_axis: TimeAxis::Transaction,
                as_of: None,
                ..q.clone()
            },
            |i| format!("${i}"),
        );
        assert!(tx_sql.contains("t.tx_from <= $3"));
        assert!(tx_sql.contains("t.tx_until IS NULL OR $4 < t.tx_until"));
        assert!(!tx_values[2].is_empty(), "{:?}", tx_values[2]);
        assert_eq!(tx_values[2], tx_values[3]);
        assert!(!tx_sql.contains("t.valid_until IS NULL OR"));
    }
}
