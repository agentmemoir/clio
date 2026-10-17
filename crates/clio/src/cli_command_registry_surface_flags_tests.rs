// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the accepted-name derivation: the hidden flags the
//! transports keep, the per-surface name lists the parsers gate on, and the
//! suggestion tie-break the alphabetical order preserves.
//!
//! ## Owns
//! - `--shared-bank`'s accepted-but-unprinted shape on both transports
//! - The derived name lists against the accepted sets they replaced
//! - One near-miss suggestion whose answer depends on the list order
//!
//! ## Does not own
//! - The matcher itself (see `cli_args_suggest_tests`)
//! - The records the names derive from (see the record modules' own tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the derived names.

/// The transports keep `--shared-bank` accepted and unprinted, the hidden
/// name is neither documented nor a declared global, and no other reserved
/// surface hides a flag.
#[test]
fn shared_bank_is_accepted_by_the_transports_and_printed_nowhere() {
    for name in ["mcp stdio", "mcp http"] {
        let spec = crate::cli_command_registry::find(name)
            .unwrap_or_else(|| panic!("`{name}` is registered"));
        let names = super::parse_flag_names(name);
        assert!(
            names.contains(&super::SHARED_BANK),
            "{name} accepts --shared-bank"
        );
        assert!(
            !spec.usage_line().contains("shared-bank"),
            "{name} does not print --shared-bank"
        );
        assert!(
            crate::cli_command_registry::global_flag(super::SHARED_BANK).is_none(),
            "--shared-bank is not a declared global"
        );
        assert!(
            !spec
                .documented
                .iter()
                .any(|flag| flag.long == super::SHARED_BANK),
            "{name} does not document --shared-bank"
        );
    }
    for name in [
        "version",
        "health",
        "help",
        "status",
        "setup",
        "install",
        "uninstall",
        "mcp schema-export",
        "ops diagnose",
        "ops verify",
        "ops doctor",
        "ops repair",
        "ops reindex",
    ] {
        assert!(
            super::hidden_accepted_flags(name).is_empty(),
            "{name} hides no flag"
        );
    }
}

/// The derived name lists keep the per-surface accepted sets the retired hand
/// tables gated. The lists are alphabetical: the suggestion matcher keeps the
/// earlier candidate on a tie and only same-first-letter candidates compete,
/// so this order preserves every near-miss answer those tables gave.
#[test]
fn the_derived_names_keep_each_surfaces_accepted_set() {
    assert_eq!(
        super::parse_flag_names("mcp stdio"),
        ["backend", "bank", "db", "shared-bank"]
    );
    assert_eq!(
        super::parse_flag_names("mcp http"),
        [
            "backend",
            "bank",
            "bind",
            "db",
            "insecure-no-auth",
            "sessions",
            "shared-bank",
            "token-file"
        ]
    );
    assert_eq!(
        super::parse_flag_names("mcp schema-export"),
        ["out", "path"]
    );
    assert_eq!(
        super::parse_flag_names("status"),
        ["backend", "db", "dir", "output"]
    );
    assert_eq!(
        super::parse_flag_names("ops diagnose"),
        ["backend", "bank", "db"]
    );
    assert_eq!(
        super::parse_flag_names("ops verify"),
        ["backend", "bank", "db"]
    );
    assert_eq!(
        super::parse_flag_names("ops doctor"),
        ["backend", "bank", "db", "dry-run"]
    );
    assert_eq!(
        super::parse_flag_names("ops repair"),
        ["actions", "backend", "bank", "confirm", "db", "plan"]
    );
    assert_eq!(
        super::parse_flag_names("ops reindex"),
        ["backend", "bank", "confirm", "db", "dry-run", "target"]
    );
    assert_eq!(super::parse_flag_names("compose up"), ["dir", "services"]);
    assert_eq!(super::parse_flag_names("compose down"), ["dir", "services"]);
}

/// A near-miss whose answer depends on the candidate order: `--bink` sits
/// equidistant between `--bank` and `--bind`, and both outrank `--backend`, so
/// the alphabetical list keeps `--bank` first and the suggestion names the
/// value flag, not the bind.
#[test]
fn a_near_miss_on_the_http_transport_keeps_the_value_flag_suggestion() {
    let error = crate::cli_top_level::parse_flags(
        ["--bink".to_owned()],
        &super::parse_flag_names("mcp http"),
    )
    .expect_err("--bink is not a flag");
    assert_eq!(
        error.hint(),
        Some("did you mean `--bank`?"),
        "the near-miss suggestion follows the alphabetical order"
    );
}
