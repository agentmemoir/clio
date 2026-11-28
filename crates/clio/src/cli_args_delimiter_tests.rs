// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `--` delimiter scan: the arguments before the first
//! `--`, or all of them when the list holds none.
//!
//! ## Owns
//! - Assertions over `before_delimiter`
//!
//! ## Does not own
//! - The scan under test (`cli_args_delimiter`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::before_delimiter;

fn tokens(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// The helper stops at the first `--`, whatever follows it.
#[test]
fn before_delimiter_stops_at_the_first_delimiter() {
    let plain = tokens(&["health", "--json"]);
    assert_eq!(before_delimiter(&plain), plain.as_slice());
    let first = tokens(&["--", "help"]);
    assert_eq!(before_delimiter(&first), tokens(&[]).as_slice());
    assert_eq!(
        before_delimiter(&tokens(&["help", "--", "--json"])),
        ["help".to_owned()]
    );
    assert_eq!(
        before_delimiter(&tokens(&["help", "--"])),
        ["help".to_owned()]
    );
}
