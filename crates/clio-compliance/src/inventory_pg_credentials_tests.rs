// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the credential half of the scope-ledger
//! Postgres tests: reading a connection URL's userinfo in memory for a leak
//! check, and the fail-closed gate that turns an unreachable store into a panic
//! unless the operator opted out.
//!
//! ## Owns
//! - `url_userinfo` and `url_password`: the in-memory credential readers
//! - `pg_gate_or_panic`: the pair gate over the store's fail-closed helper
//!
//! ## Does not own
//! - The URL resolution and the skip wording (`inventory_pg_support_tests`)
//! - The opt-out read and the strict default (`clio_store::pg_skip`)
//! - The ledger and its rows (`inventory.rs`, `inventory_rows.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the credential readers and their gate.

use clio_types::AmError;

/// The userinfo (`user:password`) a connection URL carries, read in memory and
/// never printed.
#[must_use]
pub(crate) fn url_userinfo(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (userinfo, _host) = after_scheme.rsplit_once('@')?;
    (!userinfo.is_empty()).then(|| userinfo.to_owned())
}

/// The password half of a connection URL's userinfo, read in memory and never printed.
#[must_use]
pub(crate) fn url_password(url: &str) -> Option<String> {
    let userinfo = url_userinfo(url)?;
    userinfo
        .split_once(':')
        .map(|(_, password)| password.to_owned())
}

/// The fail-closed gate every fixture helper routes through: the pair on
/// success, or a panic unless `CLIO_SKIP_PG_TESTS` opted out, in which case the
/// store's credential-safe skip note prints and this yields `None`.
pub(crate) fn pg_gate_or_panic<T>(suite: &str, pair: Result<T, AmError>) -> Option<T> {
    match pair {
        Ok(value) => Some(value),
        Err(err) => {
            clio_store::pg_skip::unreachable_pg_or_skip(suite, &err);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{url_password, url_userinfo};

    /// The credential readers split the userinfo half only, and an address
    /// without one yields nothing to check against.
    #[test]
    fn inventory_pg_url_credential_readers_split_only_the_userinfo() {
        let url = "postgres://user:secret@127.0.0.1:34310/clio?options=x";
        assert_eq!(url_userinfo(url).as_deref(), Some("user:secret"));
        assert_eq!(url_password(url).as_deref(), Some("secret"));
        assert_eq!(url_userinfo("postgres://127.0.0.1:34310/clio"), None);
        assert_eq!(url_password("postgres://127.0.0.1:34310/clio"), None);
    }

    /// A separator inside the password must not truncate the userinfo, so the
    /// leak check still sees the whole secret.
    #[test]
    fn inventory_pg_url_credential_readers_keep_a_separator_in_the_password() {
        let url = "postgres://user:pa/ss?w@127.0.0.1:34310/clio";
        assert_eq!(url_userinfo(url).as_deref(), Some("user:pa/ss?w"));
        assert_eq!(url_password(url).as_deref(), Some("pa/ss?w"));
    }
}
