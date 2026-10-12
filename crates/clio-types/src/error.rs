// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for stable error codes and tool `{ok, code, …}` envelopes.
//!
//! ## Owns
//! - `ErrorCode` and `AmError` shapes
//! - `ToolResult` success/failure serialization
//!
//! ## Does not own
//! - Domain validation rules for config or ranking values
//! - Logging sinks or secret redaction
//!
//! ## Boundary
//!
//! Changes to tool error envelope shape should normally require changes here.
//! Changes to which operations fail should not require changes here.
//!
//! Keep this module focused on stable error codes and tool response envelopes.

use serde::Serialize;
use std::fmt;

/// Tool failure codes used by in-process bindings and serialized into the
/// `{ok, code, message}` envelope as `snake_case` strings.
///
/// # The set is open
///
/// Codes are added over time and are never removed or renamed. A consumer must
/// therefore treat `code` as an open enumeration: match the codes it cares about
/// and keep a default arm, rather than switching exhaustively over a list it
/// copied from this file. Adding a variant is a compatible change; a consumer
/// that breaks on an unknown code was already relying on a closed set that the
/// type never promised.
///
/// The *meaning* of a code is also open in one direction only: a condition may
/// be reclassified to a more specific code, so a consumer must not read a code
/// as "this is the only way this condition can surface". What is guaranteed is
/// the wire string in [`ErrorCode::as_str`], which is covered by
/// `all_error_codes_have_stable_strings`.
///
/// # Which code a missing target gets
///
/// The code follows the resource the caller named, so one condition can surface
/// under different codes for different resources:
///
/// - an item row, including a reference to a missing item inside a payload such
///   as a triple's `snapshot_ref` or a distilled unit's hub, is
///   [`ErrorCode::ItemNotFound`];
/// - a distilled-unit record is [`ErrorCode::NotFound`], although the caller
///   names it by item id;
/// - a belief id is [`ErrorCode::InvalidArgument`] on the append and correct
///   paths, and [`ErrorCode::NotFound`] on the belief history read;
/// - a task id, and a triple addressed by subject and predicate, are
///   [`ErrorCode::NotFound`];
/// - a memtree node is [`ErrorCode::UnknownPath`];
/// - a `graph_link` endpoint is [`ErrorCode::ItemNotFound`], on both the
///   retrieve-side route and the batch pre-flight.
///
/// One of those departs from the item-row rule and is recorded here rather than
/// silently relied on: the distilled-unit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotImplemented,
    InvalidArgument,
    InvalidType,
    OutOfRange,
    UnknownProfile,
    UnknownPath,
    MissingBank,
    /// A read target does not exist (or is not visible in the caller's scope).
    NotFound,
    /// An item id target does not exist or is not visible in the caller's bank.
    ///
    /// A discarded item is not visible: discard is removal from active use, and
    /// the row stays stored. Naming one therefore reports this code exactly as a
    /// missing one does, which is why a `graph_link` endpoint is refused on
    /// every route.
    ItemNotFound,
    /// Co-activation reinforcement could not be durably handed off.
    CoactivationHandoffFailed,
    Forbidden,
    ConfigCorrupt,
    FactorUnavailable,
    /// Discrete path rejected a continuous/EMA-style update.
    WrongUpdateRule,
    /// Read hit a crypto-shredded subject: the DEK was destroyed.
    ///
    /// Reads fail closed with this code instead of surfacing a raw decryption
    /// failure; payloads for the subject are permanently unrecoverable.
    ErasedSubject,
    /// One row's stored content could not be opened: a malformed envelope, an
    /// undecodable nonce or ciphertext, a missing content column, or sealed
    /// bytes that are not this build's content format.
    ///
    /// This is deliberately distinct from [`ErrorCode::Internal`], which also
    /// covers a backend that is down (a driver, connection, or key-store
    /// failure). A caller that only needs an optional row — an admission
    /// neighbor scan, a tolerant listing — skips this code and keeps going; the
    /// same caller must still fail closed on `Internal`.
    ///
    /// `clio_store::is_unreadable_content` is the one predicate that groups this
    /// code with `Forbidden` and `ErasedSubject` for that purpose.
    UnreadableContent,
    /// Ephemeral workspace quota exceeded (scratchpad byte/key caps and a
    /// filesystem that is full).
    ///
    /// This is the machine's condition and only that. A limit this build
    /// imposes is not a quota: the archive reader's own caps are
    /// `ArchiveCorrupt` because a cap is a property of the file, and a snapshot
    /// member too large for the ustar size field is `OutOfRange` because it is
    /// a property of the member (#86). No amount of free space lifts either.
    QuotaExceeded,
    /// An on-disk archive this build refuses to read: a broken or truncated
    /// container, a bad header checksum, a missing end-of-archive marker, a
    /// manifest of the wrong kind or format version, a digest mismatch, or a
    /// member count, member size, or total inflated size over the reader's caps.
    ///
    /// The message names the specific defect; the code only classifies the
    /// failure as "this file is not readable by this build". The reader's caps
    /// are here rather than in `QuotaExceeded` because a cap is a property of
    /// the file: no amount of free space lets this build read it.
    ArchiveCorrupt,
    Internal,
}

impl ErrorCode {
    /// Every code, in declaration order.
    ///
    /// Callers that need to reason about a whole code (a coverage-style table, a
    /// classification predicate, a metrics label set) should iterate this instead
    /// of keeping their own copy of the list, which is how a hand-maintained list
    /// goes stale. `all_error_codes_have_stable_strings` fails if a variant is
    /// added here without also being added to that test's string table.
    pub const ALL: &'static [ErrorCode] = &[
        Self::NotImplemented,
        Self::InvalidArgument,
        Self::InvalidType,
        Self::OutOfRange,
        Self::UnknownProfile,
        Self::UnknownPath,
        Self::MissingBank,
        Self::NotFound,
        Self::ItemNotFound,
        Self::CoactivationHandoffFailed,
        Self::Forbidden,
        Self::ConfigCorrupt,
        Self::FactorUnavailable,
        Self::WrongUpdateRule,
        Self::ErasedSubject,
        Self::UnreadableContent,
        Self::QuotaExceeded,
        Self::ArchiveCorrupt,
        Self::Internal,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotImplemented => "not_implemented",
            Self::InvalidArgument => "invalid_argument",
            Self::InvalidType => "invalid_type",
            Self::OutOfRange => "out_of_range",
            Self::UnknownProfile => "unknown_profile",
            Self::UnknownPath => "unknown_path",
            Self::MissingBank => "missing_bank",
            Self::NotFound => "not_found",
            Self::ItemNotFound => "item_not_found",
            Self::CoactivationHandoffFailed => "coactivation_handoff_failed",
            Self::Forbidden => "forbidden",
            Self::ConfigCorrupt => "config_corrupt",
            Self::FactorUnavailable => "factor_unavailable",
            Self::WrongUpdateRule => "wrong_update_rule",
            Self::ErasedSubject => "erased_subject",
            Self::UnreadableContent => "unreadable_content",
            Self::QuotaExceeded => "quota_exceeded",
            Self::ArchiveCorrupt => "archive_corrupt",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Library-level error carrying a stable code and human message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmError {
    pub code: ErrorCode,
    pub message: String,
}

impl AmError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn not_implemented(tool: &str) -> Self {
        Self::new(
            ErrorCode::NotImplemented,
            format!("tool `{tool}` is declared but not implemented in this build"),
        )
    }
}

impl fmt::Display for AmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AmError {}

/// Tool envelope: success payload or structured failure (`{ok, code, …}`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ToolResult<T: Serialize> {
    Ok {
        ok: bool,
        #[serde(flatten)]
        data: T,
    },
    Err {
        ok: bool,
        code: ErrorCode,
        message: String,
    },
}

impl<T: Serialize> ToolResult<T> {
    #[must_use]
    pub fn success(data: T) -> Self {
        Self::Ok { ok: true, data }
    }

    #[must_use]
    pub fn failure(err: AmError) -> Self {
        Self::Err {
            ok: false,
            code: err.code,
            message: err.message,
        }
    }

    #[must_use]
    pub fn from_result(result: Result<T, AmError>) -> Self {
        match result {
            Ok(data) => Self::success(data),
            Err(err) => Self::failure(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_includes_code() {
        let err = AmError::new(ErrorCode::InvalidType, "expected number");
        assert!(err.to_string().contains("invalid_type"));
    }

    #[test]
    fn tool_result_failure_serializes_ok_false() {
        let result = ToolResult::<()>::failure(AmError::not_implemented("store"));
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("not_implemented"));
    }

    /// The wire-string contract, frozen.
    ///
    /// This table is the compatibility promise. Adding a row is the only
    /// supported way to add a code; changing a string is a breaking change to
    /// every consumer that matched it. Because the table and
    /// [`ErrorCode::ALL`] must agree on length, a new variant cannot be added to
    /// one without the other — which is what stops the list going stale.
    const CODE_STRINGS: &[(ErrorCode, &str)] = &[
        (ErrorCode::NotImplemented, "not_implemented"),
        (ErrorCode::InvalidArgument, "invalid_argument"),
        (ErrorCode::InvalidType, "invalid_type"),
        (ErrorCode::OutOfRange, "out_of_range"),
        (ErrorCode::UnknownProfile, "unknown_profile"),
        (ErrorCode::UnknownPath, "unknown_path"),
        (ErrorCode::MissingBank, "missing_bank"),
        (ErrorCode::NotFound, "not_found"),
        (ErrorCode::ItemNotFound, "item_not_found"),
        (
            ErrorCode::CoactivationHandoffFailed,
            "coactivation_handoff_failed",
        ),
        (ErrorCode::Forbidden, "forbidden"),
        (ErrorCode::ConfigCorrupt, "config_corrupt"),
        (ErrorCode::FactorUnavailable, "factor_unavailable"),
        (ErrorCode::WrongUpdateRule, "wrong_update_rule"),
        (ErrorCode::ErasedSubject, "erased_subject"),
        (ErrorCode::UnreadableContent, "unreadable_content"),
        (ErrorCode::QuotaExceeded, "quota_exceeded"),
        (ErrorCode::ArchiveCorrupt, "archive_corrupt"),
        (ErrorCode::Internal, "internal"),
    ];

    #[test]
    fn all_error_codes_have_stable_strings() {
        for (code, wire) in CODE_STRINGS {
            assert_eq!(code.as_str(), *wire, "{code:?} wire string changed");
            assert_eq!(code.to_string(), *wire, "{code:?} Display drifted");
        }
    }

    /// The frozen table and [`ErrorCode::ALL`] describe the same set, with no
    /// duplicates on either side.
    #[test]
    fn all_error_codes_covers_exactly_the_frozen_table() {
        assert_eq!(
            ErrorCode::ALL.len(),
            CODE_STRINGS.len(),
            "a code was added without a frozen wire string"
        );
        for code in ErrorCode::ALL {
            let occurrences = CODE_STRINGS
                .iter()
                .filter(|(listed, _)| listed == code)
                .count();
            assert_eq!(occurrences, 1, "{code:?} is not listed exactly once");
        }
    }

    /// `AmError` is a `std::error::Error`, so callers can use `?` into any
    /// boxed-error boundary.
    #[test]
    fn am_error_is_a_std_error() {
        let err = AmError::new(ErrorCode::Internal, "x");
        let as_std: &dyn std::error::Error = &err;
        assert_eq!(as_std.to_string(), "internal: x");
    }

    #[test]
    fn tool_result_from_result() {
        let ok = serde_json::to_value(ToolResult::from_result(Ok(()))).expect("ser");
        assert_eq!(ok["ok"], true);
        let err = serde_json::to_value(ToolResult::<()>::from_result(Err(AmError::new(
            ErrorCode::Forbidden,
            "no",
        ))))
        .expect("ser");
        assert_eq!(err["ok"], false);
        assert_eq!(err["code"], "forbidden");
    }
}
