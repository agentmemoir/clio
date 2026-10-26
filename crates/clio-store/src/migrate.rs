// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module supplies schema assets and shared database error conversion.
//!
//! ## Owns
//! - Embedded schema SQL and portable timestamp formatting
//! - Driver error classification and context
//!
//! ## Does not own
//! - Store startup order or schema convergence
//! - CLI output or exit codes
//!
//! ## Boundary
//!
//! Changes to database error conversion should normally require changes here.
//! Changes to store commands should not require changes here.
//!
//! Keep this module focused on shared schema and driver primitives.

use crate::distilled_freshness::stamp_millis;
use clio_types::{AmError, ErrorCode, civil_from_days};

/// Schema version this build expects (kept in lockstep with `SQL_CORE`).
pub const SCHEMA_VERSION: &str = "1";
/// Core portable DDL.
pub const SQL_CORE: &str = include_str!("../../../sql/001_core.sql");
/// Postgres + pgvector dialect.
pub const SQL_VECTORS_POSTGRES: &str = include_str!("../../../sql/002_vectors_postgres.sql");
/// SQLite + sqlite-vec dialect.
pub const SQL_VECTORS_SQLITE: &str = include_str!("../../../sql/002_vectors_sqlite.sql");
/// Local-compliance ownership slots (portable).
pub const SQL_COMPLIANCE_SLOTS: &str = include_str!("../../../sql/003_local_compliance_slots.sql");

/// Map a driver error into `AmError`.
///
/// The Postgres driver's own `Display` for a server-side refusal is the bare
/// string `db error`. The reason an operator can act on — the constraint name,
/// the `DETAIL`, the `HINT` — sits behind that in the error's source chain, so
/// using `Display` alone left every statement-level failure reporting as
/// `items insert: db error` and nothing else. This function is the single
/// mapping every statement goes through, so the walk belongs here; the connect
/// and probe paths already did it locally, which is why the same failure said
/// three different things depending on how far it got.
///
/// Errors from the SQLite backend keep their existing rendering: `rusqlite`
/// puts its message in `Display` already, so there is nothing behind it to find
/// and no test would notice a change.
#[must_use]
pub fn map_db_err<E>(context: &str, err: E) -> AmError
where
    E: std::fmt::Display + 'static,
{
    let postgres_error = (&err as &dyn std::any::Any).downcast_ref::<postgres::Error>();
    if postgres_error.is_some_and(|error| {
        matches!(error.code(), Some(code) if *code == postgres::error::SqlState::T_R_DEADLOCK_DETECTED
            || *code == postgres::error::SqlState::LOCK_NOT_AVAILABLE)
    }) {
        return AmError::new(ErrorCode::StorageBusy, "Postgres lock conflict; retry the command");
    }
    let reason = postgres_error.map_or_else(|| err.to_string(), crate::pg_tls::describe);
    AmError::new(ErrorCode::Internal, format!("{context}: {reason}"))
}

/// Convert driver `Result`s into `AmError` (single coverage sink for all call sites).
pub(crate) trait IntoAmError<T> {
    fn am(self, context: &'static str) -> Result<T, AmError>;
}

impl<T, E: std::fmt::Display + 'static> IntoAmError<T> for Result<T, E> {
    fn am(self, context: &'static str) -> Result<T, AmError> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(map_db_err(context, e)),
        }
    }
}

/// Fail closed when the SQLite vector extension is unavailable.
#[must_use]
pub fn sqlite_vec_missing(detail: impl std::fmt::Display) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "sqlite-vec extension is required but failed to load ({detail}); \
             install/link sqlite-vec — silent float-blob mode is not allowed"
        ),
    )
}

/// ISO-8601-ish UTC timestamp suitable for portable text columns.
#[must_use]
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    // ponytail: no chrono dependency; millisecond epoch stamp as sortable UTC text.
    format!("{millis}")
}

/// UTC `YYYY-MM-DDThh:mm:ssZ` for the current instant.
///
/// Distinct from [`now_iso`] (epoch millis): this form sorts correctly against
/// ISO date strings such as `2024-06-01`, which is what defaults an omitted
/// triple `as_of` to “now”.
#[must_use]
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The canonical item timestamp: UTC RFC 3339, second precision, `Z` suffix.
///
/// Every item time column is compared as text, so the whole column has to be
/// one form at one precision: `2026-10-12T08:30:01.500Z` sorts before
/// `2026-10-12T08:30:01Z` even though it is later, and an epoch-millis stamp
/// sorts after every date string.
#[must_use]
pub fn millis_to_rfc3339(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Whether a timestamp is an all-digit epoch stamp rather than a date string.
///
/// This is the one test the backfill and the inspector share, so "needs
/// rewriting" and "cannot be compared as text" can never disagree.
#[must_use]
pub fn is_epoch_stamp(raw: &str) -> bool {
    !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit())
}

/// Rewrite a timestamp this store writes into [`millis_to_rfc3339`]'s form.
///
/// Two inputs are placed on the canonical clock:
///
/// - an all-digit value, read as epoch millis (the form [`now_iso`] produces);
/// - an RFC 3339 date-time `YYYY-MM-DDThh:mm:ss[.fraction](Z|±hh:mm)`,
///   converted to UTC by applying its offset and cut to whole seconds.
///
/// The calendar is validated by the store's own portable stamp reader, so
/// "placeable" means the same thing here as it does on a read: a month the year
/// does not have, a day the month does not have, an hour of 24, or an offset of
/// `+24:00` is not a stamp and is not rewritten. Anything this build cannot
/// place is returned unchanged, and nothing it does place lands on a different
/// instant — an earlier build cut a value at its 19th byte, which dropped
/// `+02:00` instead of applying it and moved
/// `2026-10-12T08:30:01.500+02:00` two hours later on every open.
#[must_use]
pub fn canonical_ts(raw: &str) -> String {
    canonical_stamp(raw).unwrap_or_else(|| raw.to_owned())
}

/// The canonical form of a stamp this build can place, or `None` when it cannot.
///
/// This is the placeability test [`canonical_ts`] is built on, and the one a
/// write path needs: a caller-supplied stamp must be refused rather than stored
/// verbatim, and "refused" has to mean the same thing as "the backfill cannot
/// place it", or the two disagree about which rows are on the shared clock.
#[must_use]
pub fn canonical_stamp(raw: &str) -> Option<String> {
    stamp_instant(raw).map(millis_to_rfc3339)
}

/// The instant a stamp this build can place names, in UTC epoch millis.
fn stamp_instant(raw: &str) -> Option<i64> {
    if is_epoch_stamp(raw) {
        return raw.parse::<i64>().ok();
    }
    let (millis, offset_secs) = stamp_offset(raw)?;
    Some(millis - offset_secs * 1_000)
}

/// The canonical form of a caller-supplied stamp, or the refusal for one this
/// build cannot place.
///
/// Every write path that accepts a timestamp from a caller goes through here, so
/// "placeable" is decided once: a value [`canonical_stamp`] rewrites is stored in
/// its canonical form, and one it cannot place is refused rather than written and
/// compared as a literal string.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] naming the value that cannot be placed.
pub fn canonical_write_stamp(raw: &str) -> Result<String, AmError> {
    canonical_stamp(raw).ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            format!("timestamp `{raw}` is not a timestamp this build can place"),
        )
    })
}

/// Canonical form of an optional stamp: absent stays absent, present is
/// canonicalised under [`canonical_write_stamp`]'s rule.
///
/// An empty value is a caller convention for "absent", so it stays absent
/// rather than being stored as an empty string in a nullable time column.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] when a present, non-empty value cannot be
/// placed.
pub fn canonical_optional_stamp(raw: Option<&str>) -> Result<Option<String>, AmError> {
    match raw.filter(|value| !value.is_empty()) {
        Some(value) => canonical_write_stamp(value).map(Some),
        None => Ok(None),
    }
}

/// The `tx_until` to store when closing a row whose transaction interval starts
/// at `tx_from`.
///
/// Reads compare the column as text with a half-open predicate, so an interval
/// whose ends name the same instant contains no instant: the closed row leaves
/// transaction-time history, and `TripleRecord::validate` refuses a negative
/// length. Both sides are therefore resolved to instants rather than compared as
/// bytes, and compared at the whole second the columns are written and read at,
/// so the row's own start spelled on another offset or with a fraction is still
/// recognised as its start. A close naming an instant at or before that start is
/// placed one second after it, in the canonical form; a close naming a later
/// second is returned unchanged, byte for byte, and so is one either side cannot
/// be placed on the clock.
#[must_use]
pub(crate) fn close_stamp(tx_from: &str, tx_until: &str) -> String {
    let (Some(from), Some(until)) = (stamp_instant(tx_from), stamp_instant(tx_until)) else {
        return tx_until.to_owned();
    };
    if until.div_euclid(1_000) > from.div_euclid(1_000) {
        return tx_until.to_owned();
    }
    millis_to_rfc3339(from.saturating_add(1_000))
}

/// `YYYY-MM-DDThh:mm:ss[.fraction](Z|±hh:mm)` → (instant, offset in seconds east
/// of UTC).
///
/// The shape is checked here and the calendar is not: the date-time and its
/// fraction are handed to the store's portable stamp reader with a `Z` suffix,
/// so the month length, the leap rule and the clock ranges are read by the same
/// code that reads a stored stamp on the read path. Only the offset arithmetic
/// is new, and [`canonical_ts`] applies it.
fn stamp_offset(raw: &str) -> Option<(i64, i64)> {
    let bytes = raw.as_bytes();
    let shaped = bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..19]
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit());
    if !shaped {
        return None;
    }
    let rest = &raw[19..];
    let (fraction, zone) = match rest.strip_prefix('.') {
        Some(after) => {
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            (&rest[..=digits], &after[digits..])
        }
        None => ("", rest),
    };
    let offset_secs = match zone {
        "" | "Z" => 0,
        _ => zone_offset(zone)?,
    };
    let stamp = format!("{}{fraction}Z", &raw[..19]);
    Some((stamp_millis(&stamp)?, offset_secs))
}

/// `±hh:mm` → seconds east of UTC; `None` for anything else, including an hour
/// or minute the zone range does not have.
fn zone_offset(zone: &str) -> Option<i64> {
    let bytes = zone.as_bytes();
    if bytes.len() != 6 || bytes[3] != b':' {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i64 = zone.get(1..3)?.parse().ok()?;
    let minutes: i64 = zone.get(4..6)?.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

#[path = "migrate_ts_backfill.rs"]
pub(crate) mod ts_backfill;

#[cfg(test)]
#[path = "migrate_canonical_tests.rs"]
mod canonical_tests;

#[cfg(test)]
#[path = "migrate_reason_tests.rs"]
mod reason_tests;

#[cfg(test)]
#[path = "migrate_compliance_slots_tests.rs"]
mod migrate_compliance_slots_tests;

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
