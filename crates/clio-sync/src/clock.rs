// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for wall-clock stamping for the sync crate:
//! one UTC `now_utc()` used for journal/created timestamps and DLQ records.
//!
//! ## Owns
//! - `now_utc()` epoch → ISO-8601 UTC text conversion (same format as the
//!   store's durable stamps)
//!
//! ## Does not own
//! - Transaction-time semantics (callers pass writer stamps on envelopes)
//! - Cursor or LWW ordering logic (see `protocol`)
//!
//! ## Boundary
//!
//! Changes to the sync stamp format should normally require changes here and
//! in `clio-store` together. Keep this module focused on clock stamping.

use clio_types::civil_from_days;

/// Current UTC time as ISO-8601 text (comparable with the store's stamps).
#[must_use]
pub fn now_utc() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let (y, m, d) = civil_from_days(days);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}
