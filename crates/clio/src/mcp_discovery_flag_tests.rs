// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the `--discovery` flag: the
//! values it accepts, the values it refuses, and the absent flag leaving the
//! configured setting to decide.
//!
//! ## Owns
//! - The accepted `on` and `off` values
//! - The usage refusal for every other value
//!
//! ## Does not own
//! - Which tools discovery adds (see `clio-mcp`)
//! - Rendering the refusal, or opening a store (see `mcp_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the flag's values.

use super::selection;
use crate::cli_flags_test_support::flags_of;
use std::process::ExitCode;

/// The two values the flag accepts, and the equals spelling of one of them,
/// decode to the switch they name.
#[test]
fn on_and_off_are_the_only_values() {
    assert_eq!(
        selection(&flags_of(&["--discovery", "on"])).expect("`on` is accepted"),
        Some(true)
    );
    assert_eq!(
        selection(&flags_of(&["--discovery", "off"])).expect("`off` is accepted"),
        Some(false)
    );
    assert_eq!(
        selection(&flags_of(&["--discovery=on"])).expect("the equals spelling is accepted"),
        Some(true)
    );
}

/// With no flag, the selection is absent so the configuration document decides;
/// the document's own default is off.
#[test]
fn the_absent_flag_leaves_the_document_to_decide() {
    assert_eq!(
        selection(&flags_of(&[])).expect("no flag is not an error"),
        None
    );
    let mut runtime = clio_config::Runtime::with_env(&|_: &str| None);
    assert!(!runtime.mcp_discovery(), "discovery is off by default");
    runtime
        .load_file_json(r#"{"mcp": {"discovery": true}}"#)
        .expect("a valid config layer");
    assert!(
        runtime.mcp_discovery(),
        "the document decides when no flag is installed"
    );
}

/// Every value other than `on` and `off` is a usage refusal, including the
/// `true` a bare flag produces, so no unrecognised word silently means on or
/// off.
#[test]
fn an_unrecognised_value_is_refused_not_defaulted() {
    for value in ["true", "yes", "ON", "1"] {
        let error = selection(&flags_of(&["--discovery", value]))
            .expect_err("only `on` and `off` are accepted");
        assert_eq!(
            error.exit_code(),
            ExitCode::from(2),
            "`{value}` must be a usage error"
        );
        assert!(error.message().contains(value), "{}", error.message());
    }
    let bare = selection(&flags_of(&["--discovery"]))
        .expect_err("a bare `--discovery` names no accepted value");
    assert_eq!(
        bare.exit_code(),
        ExitCode::from(2),
        "a bare flag must be a usage error"
    );
    assert!(bare.message().contains("true"), "{}", bare.message());
}
