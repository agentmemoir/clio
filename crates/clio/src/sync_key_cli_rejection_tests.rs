// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the key verbs' argument surface: the flag specs each verb
//! carries, the unknown-flag refusals the parser raises for a flag another verb
//! owns, and the export usage line's statement of the deliberate exposure.
//!
//! ## Owns
//! - The flag-spec test for `sync keygen`, `sync key export`, and
//!   `sync key status`
//! - The export usage content test
//!
//! ## Does not own
//! - The verbs' runtime refusals and output (see `sync_key_cli_tests`)
//! - The parser itself (see `cli_args_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Parsing runs against
//! `spec()` directly, so no test reads the process environment or any store.
//!
//! Keep this module focused on the key verbs' argument surface tests.

use super::spec;
use crate::cli_args;
use crate::sync_key_cli_test_support::tokens;

/// Each key verb carries its own flag spec, and a flag it does not own is an
/// unknown-flag refusal rather than a silent accept.
///
/// `keygen` is the only key verb with a local value flag: `--store`. The two
/// read-only verbs carry no local flags at all, so `--store` and `--bind` fail
/// closed on them, while the global `--output` keeps parsing everywhere.
#[test]
fn the_key_verbs_carry_their_own_flag_specs() {
    let keygen = spec("sync keygen");
    assert!(
        keygen.value_flags().contains(&"store"),
        "keygen takes --store: {:?}",
        keygen.value_flags()
    );
    assert!(
        keygen.value_flags().contains(&"output"),
        "keygen keeps the global --output: {:?}",
        keygen.value_flags()
    );
    assert!(
        cli_args::parse(&keygen, tokens(&["--store", "file"])).is_ok(),
        "keygen parses --store"
    );
    assert!(
        cli_args::parse(&keygen, tokens(&["--bind", "127.0.0.1:0"])).is_err(),
        "keygen refuses a flag it does not own"
    );

    for verb in ["sync key export", "sync key status"] {
        let verb_spec = spec(verb);
        assert!(
            !verb_spec.value_flags().contains(&"store"),
            "{verb} takes no --store"
        );
        assert!(
            !verb_spec.value_flags().contains(&"bind"),
            "{verb} takes no --bind"
        );
        assert!(
            cli_args::parse(&verb_spec, tokens(&["--output", "json"])).is_ok(),
            "{verb} keeps the global --output"
        );
        for args in [&["--store", "file"][..], &["--bind", "x"][..]] {
            let error = cli_args::parse(&verb_spec, tokens(args))
                .expect_err("a flag another verb owns is refused");
            assert!(
                error.message().contains("unknown flag"),
                "{verb} refuses {args:?} as an unknown flag: {}",
                error.message()
            );
        }
    }
}

/// The export usage names the deliberate exposure and the file path that
/// avoids it, so an operator reading `--help` learns both.
#[test]
fn the_export_usage_names_the_exposure_and_the_file_path() {
    let usage =
        crate::cli_help_usage::usage_for("sync key export").expect("a usage line is registered");
    assert!(usage.contains("process listings"), "{usage}");
    assert!(usage.contains("CLIO_SYNC_KEY_FILE"), "{usage}");
}
