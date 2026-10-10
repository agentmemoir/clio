// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared destructive-confirmation gate: which flag
//! combinations consent to a destructive change, and the exit-2 refusal naming
//! the missing flag.
//!
//! ## Owns
//! - Unit assertions over `confirmed` / `require_confirm`
//!
//! ## Does not own
//! - The gate implementation under test (`cli_confirm`)
//! - The commands that call it (see the `cli_write_*` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_args::{FlagSpec, parse};

fn parsed(args: &[&str]) -> ParsedArgs {
    let tokens: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    parse(&FlagSpec::new(&["reason"], &["confirm", "dry-run"]), tokens).expect("parse")
}

#[test]
fn confirmation_requires_confirm_yes_or_dry_run() {
    assert!(confirmed(&parsed(&["--confirm"])));
    assert!(confirmed(&parsed(&["--yes"])));
    assert!(!confirmed(&parsed(&[])));
    // `--dry-run` is not consent to write; the gate allows it only as a preview.
    assert!(!confirmed(&parsed(&["--dry-run"])));
}

#[test]
fn require_confirm_refuses_and_names_the_flag() {
    let error = require_confirm("discard", &parsed(&["x", "--reason", "r"])).expect_err("refused");
    assert_eq!(error.exit_code(), std::process::ExitCode::from(2));
    assert!(error.message().contains("destructive"), "{error:?}");
    assert!(
        error.hint().unwrap_or_default().contains("pass --confirm"),
        "{error:?}"
    );
}

#[test]
fn require_confirm_accepts_each_consent_form() {
    for args in [
        vec!["--confirm"],
        vec!["--yes"],
        vec!["--dry-run"],
        vec!["--confirm", "--dry-run"],
    ] {
        assert!(
            require_confirm("discard", &parsed(&args)).is_ok(),
            "{args:?} must pass the gate"
        );
    }
}
