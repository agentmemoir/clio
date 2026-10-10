// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cases in which the prefixed name is set
//! and is the answer: alone, and to a value unlike any default.
//!
//! ## Owns
//! - The prefixed name reaching all three paths on its own
//! - A prefixed value that no default could be mistaken for
//! - The link between the literal the cases use and the constant the crate owns
//!
//! ## Does not own
//! - The cases in which the name is absent (see the absent-value module)
//! - The case in which the old unprefixed spelling is set (see the old-name
//!   module)
//! - The readers themselves (see the support module beside this one)
//!
//! ## Boundary
//!
//! Changes to the prefixed variable's name should normally require changes here.
//! Changes to what any reader does with a value should not.
//!
//! Keep this module focused on the prefixed name being the answer.

use super::db_url_env_test_support::{FIRST, InjectedEnv, PREFIXED, SECOND, read_all};
use clio_store::StoreBackend;

/// Every path returns the prefixed value when the prefixed name is the only name
/// set. Three paths, one answer: a path that disagreed here is the defect the
/// rename fixed, and this is the case that catches its return.
#[test]
fn the_prefixed_name_alone_reaches_every_path() {
    let answers = read_all(&InjectedEnv::new(&[(PREFIXED, FIRST)]));

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

    assert_eq!(answers.resolver_backend, "postgres", "the resolver backend");
    assert_eq!(answers.overlay_backend, "postgres", "the overlay backend");
    assert_eq!(
        answers.bootstrap_backend,
        StoreBackend::Postgres,
        "the bootstrap backend"
    );
    assert_eq!(
        answers.bootstrap_effective_backend,
        StoreBackend::Postgres,
        "the bootstrap backend given the effective document"
    );
}

/// A value unlike any default, so a path that fell through a precedence tier
/// instead of reading the name would answer something else and be caught.
#[test]
fn a_prefixed_value_unlike_any_default_wins_on_every_path() {
    let answers = read_all(&InjectedEnv::new(&[(PREFIXED, SECOND)]));

    assert_eq!(answers.resolver_url, SECOND, "the database path resolver");
    assert_eq!(answers.overlay_url, SECOND, "the config overlay");
    assert_eq!(
        answers.bootstrap_env_tier_url, SECOND,
        "the store bootstrap's own environment tier"
    );
    assert_eq!(
        answers.bootstrap_effective_url, SECOND,
        "the store bootstrap given the effective document"
    );
}

/// The literal these cases set is the name the config crate owns.
///
/// The cases spell the name out so a rename cannot make them agree with
/// themselves. This test is the other half of that: it stops the spelling and
/// the constant drifting apart, which would leave every case below testing a
/// name the product no longer reads.
#[test]
fn the_name_these_cases_use_is_the_name_the_config_crate_owns() {
    assert_eq!(clio_config::db_url_env::DATABASE_URL_ENV, PREFIXED);
}
