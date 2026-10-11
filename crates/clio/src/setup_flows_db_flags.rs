// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the database flag applicability table: which
//! database-group flags each database route reads, and the refusal for a flag
//! the chosen route does not read.
//!
//! ## Owns
//! - `DbRoute`, the four routes a completed database answer can take
//! - `refuse_foreign`, the refusal for a database flag the route does not read
//! - The table of which database flags each route reads
//!
//! ## Does not own
//! - The questions themselves (see `setup_flows_db`)
//! - Whether a flag's value is valid (see `setup_validate`)
//! - The value-specific checks, such as a root certificate with a mode that
//!   does not verify (see `setup_flows_db_typed`)
//!
//! ## Boundary
//!
//! Changes to which flags a route reads should normally require changes here.
//! Changes to what a flag means should not require changes here.
//!
//! Keep this module focused on the one mapping from a route to the flags it reads.
//!
//! # Why the table is here and not next to each question
//!
//! A flag that reaches a route that never reads it is a silent no-op: the run
//! reports success and the setting is gone. The routes were each refusing the
//! flags they knew about, so the set of refusals grew one flag at a time and
//! fell behind the set of flags. One table, read by every route, makes the gap
//! visible: a flag is either in a route's `reads` list or it is refused there.

use crate::setup_cli_flows::Ctx;
use crate::setup_render::Signal;

/// Which route a completed database answer took.
///
/// The two existing-Postgres sub-routes are separate variants because they read
/// different flags: a paste keeps a whole address, while the typed route builds
/// one from fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbRoute {
    Managed,
    Sqlite,
    Pasted,
    Typed,
}

/// Every route, in the order a refusal message lists them.
const ROUTES: [DbRoute; 4] = [
    DbRoute::Managed,
    DbRoute::Sqlite,
    DbRoute::Pasted,
    DbRoute::Typed,
];

impl DbRoute {
    /// How the route is named on the command line, for a refusal message.
    const fn label(self) -> &'static str {
        match self {
            Self::Managed => "`--database managed`",
            Self::Sqlite => "`--database sqlite`",
            Self::Pasted => "`--database existing --database-connect pasted`",
            Self::Typed => "`--database existing --database-connect typed`",
        }
    }

    /// The database-group flags this route reads.
    ///
    /// A flag outside this list would be a silent no-op on this route, so
    /// [`refuse_foreign`] refuses it instead. `--database` is the route selector
    /// itself and is not here.
    const fn reads(self) -> &'static [&'static str] {
        match self {
            Self::Managed => &[
                "database-host",
                "database-port",
                "database-name",
                "database-user",
                "database-ssl",
            ],
            Self::Sqlite => &["db-path"],
            Self::Pasted => &["database-url", "database-connect"],
            Self::Typed => &[
                "database-connect",
                "database-host",
                "database-port",
                "database-name",
                "database-user",
                "database-password",
                "database-ssl",
                "database-ssl-rootcert",
                "database-option",
            ],
        }
    }
}

/// Every database-group value flag the applicability check watches.
///
/// The set is closed on purpose: a new database flag is added here and to the
/// route that reads it in the same change, or the matrix test fails. `--database`
/// is the selector, not a setting.
pub(crate) const DATABASE_FLAGS: &[&str] = &[
    "db-path",
    "database-url",
    "database-connect",
    "database-host",
    "database-port",
    "database-name",
    "database-user",
    "database-password",
    "database-ssl",
    "database-ssl-rootcert",
    "database-option",
];

/// Refuse the first database flag the chosen route does not read.
///
/// A route reads a flag only if a question or an assembly step on that route
/// consumes it. Accepting a flag the route ignores would be a silent no-op: the
/// run would report success and the setting would be gone, which is worse than
/// refusing it by name.
///
/// # Errors
/// A database flag was given that this route does not read.
pub(crate) fn refuse_foreign(ctx: &Ctx<'_>, route: DbRoute) -> Result<(), Signal> {
    let reads = route.reads();
    for flag in DATABASE_FLAGS {
        if !reads.contains(flag) && ctx.has_flag(flag) {
            return Err(Signal::usage(format!(
                "--{flag} applies to {}, not to {}",
                allowed(flag),
                route.label()
            )));
        }
    }
    Ok(())
}

/// The routes that read one flag, named for the operator.
///
/// Derived from the same table the check reads, so a route that gains a flag
/// cannot leave this message behind.
fn allowed(flag: &str) -> String {
    let labels: Vec<&str> = ROUTES
        .into_iter()
        .filter(|route| route.reads().contains(&flag))
        .map(DbRoute::label)
        .collect();
    labels.join(" or ")
}

#[cfg(test)]
#[path = "setup_flows_db_flags_tests.rs"]
mod tests;
