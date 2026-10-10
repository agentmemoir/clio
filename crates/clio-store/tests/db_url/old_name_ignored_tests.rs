// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the case the whole rename turns on: the old
//! unprefixed name is not a second spelling of the variable.
//!
//! ## Owns
//! - The unprefixed name alone reaching no path at all
//! - The prefixed name winning when both spellings are present
//!
//! ## Does not own
//! - The cases in which the prefixed name is the answer (see the prefixed-name
//!   module)
//! - The cases in which the name is absent (see the absent-value module)
//! - The readers themselves (see the support module beside this one)
//!
//! ## Boundary
//!
//! Changes to whether the old spelling is honoured should normally require
//! changes here. Changes to the precedence among the prefixed tiers should not.
//!
//! Keep this module focused on the old spelling being ignored.

use super::db_url_env_test_support::{
    FALLBACK, FIRST, InjectedEnv, OLD, PREFIXED, UNPREFIXED, read_all,
};
use clio_store::StoreBackend;

/// The old spelling alone points Clio at nothing.
///
/// This is the case an alias or a leftover fallback would break silently. Every
/// path must answer the same absent-value default, and the backend must stay
/// SQLite, because a path that read the old name would have inferred Postgres
/// from the address it found there.
#[test]
fn the_unprefixed_name_alone_reaches_no_path() {
    let answers = read_all(&InjectedEnv::new(&[(UNPREFIXED, OLD)]));

    assert_eq!(answers.resolver_url, FALLBACK, "the database path resolver");
    assert_eq!(answers.overlay_url, FALLBACK, "the config overlay");
    assert_eq!(
        answers.bootstrap_effective_url, FALLBACK,
        "the store bootstrap given the effective document"
    );
    assert_eq!(
        answers.bootstrap_effective_backend,
        StoreBackend::Sqlite,
        "the effective document's own default backend"
    );

    assert!(
        answers.bootstrap_env_tier_url.is_empty(),
        "the store bootstrap's own environment tier answered {} from the unprefixed name",
        answers.bootstrap_env_tier_url
    );

    assert_eq!(
        answers.resolver_backend, "sqlite",
        "the resolver inferred a backend, so it read an address"
    );
    assert_eq!(
        answers.overlay_backend, "sqlite",
        "the overlay inferred a backend, so it read an address"
    );
    assert_eq!(
        answers.bootstrap_backend,
        StoreBackend::Sqlite,
        "the bootstrap inferred a backend, so it read an address"
    );
}

/// Both spellings present: the prefixed one is the answer, and the old one
/// changes nothing.
///
/// The second half matters as much as the first. A fallback that preferred the
/// old name would pass a test that only checked the old name was not the only
/// thing read, and would leave an operator's stale export silently outranking
/// the name they just set.
#[test]
fn the_prefixed_name_wins_when_both_spellings_are_present() {
    let answers = read_all(&InjectedEnv::new(&[(UNPREFIXED, OLD), (PREFIXED, FIRST)]));

    assert_eq!(answers.resolver_url, FIRST, "the database path resolver");
    assert_eq!(answers.overlay_url, FIRST, "the config overlay");
    assert_eq!(
        answers.bootstrap_env_tier_url, FIRST,
        "the store bootstrap's own environment tier"
    );
    assert_eq!(
        answers.bootstrap_effective_url, FIRST,
        "the store bootstrap given the effective document"
    );

    assert_ne!(
        answers.resolver_url, OLD,
        "the old spelling outranked the prefixed one"
    );
    assert_eq!(answers.resolver_backend, "postgres", "the resolver backend");
    assert_eq!(answers.overlay_backend, "postgres", "the overlay backend");
    assert_eq!(
        answers.bootstrap_backend,
        StoreBackend::Postgres,
        "the bootstrap backend"
    );
}
