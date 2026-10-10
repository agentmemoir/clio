// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cases in which the prefixed name is set
//! but must count as absent: nothing set at all, a value with no content, and a
//! value carrying an interior NUL.
//!
//! ## Owns
//! - An environment with no database name answering one shared fallback
//! - Blank and whitespace-only values counting as absent
//! - A value carrying an interior NUL counting as absent
//!
//! ## Does not own
//! - The cases in which the prefixed name is the answer (see the prefixed-name
//!   module)
//! - The case in which the old unprefixed spelling is set (see the old-name
//!   module)
//! - The readers themselves (see the support module beside this one)
//!
//! ## Boundary
//!
//! Changes to what counts as an absent environment value should normally require
//! changes here. Changes to which name a database is read from should not.
//!
//! Keep this module focused on a set name that must count as absent.

use super::db_url_env_test_support::{InjectedEnv, PREFIXED, assert_fallback, read_all};

/// No name set at all: every path answers the one default, and the paths that
/// carry a default carry the same one.
///
/// Asserting the concrete default rather than skipping it is deliberate. The
/// lookup answers nothing for the directory names either, so there is nothing on
/// this machine for a path to prefer, and the answer is the same everywhere.
#[test]
fn no_database_name_agrees_on_one_fallback() {
    assert_fallback(&read_all(&InjectedEnv::none()), "no name set");
}

/// A value with no content names no database, so it takes the next tier rather
/// than handing an empty address to an opener.
///
/// Every blank spelling is in one test because they are one claim: a value with
/// no non-whitespace character is absent, whatever the character is.
#[test]
fn a_blank_or_whitespace_only_value_counts_as_absent() {
    for value in ["", " ", "\t", "\n", "  \t\n "] {
        let answers = read_all(&InjectedEnv::new(&[(PREFIXED, value)]));
        assert_fallback(&answers, &format!("value {value:?}"));
    }
}

/// A value carrying an interior NUL cannot reach an opener: the operating system
/// truncates a process value at the first NUL, so a name that kept one is not a
/// name anything can open. It counts as absent rather than as an address.
///
/// The injected lookup is what makes this case testable at all. A real process
/// value cannot carry an interior NUL, so the only way to assert the rule is to
/// hand the rule a value the operating system would never produce.
#[test]
fn a_value_carrying_an_interior_nul_counts_as_absent() {
    let answers = read_all(&InjectedEnv::new(&[(PREFIXED, "postgres://a\0b/db")]));
    assert_fallback(&answers, "interior NUL");
}
