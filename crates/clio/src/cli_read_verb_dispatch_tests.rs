// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test that pins a CLI verb to the CLI's
//! own dispatch path: the default tool profile narrows what an MCP client is
//! advertised and may call, and it must not narrow what the CLI runs.
//!
//! ## Owns
//! - One verb outside the `minimal` preset succeeding while the resolved
//!   profile is the default
//!
//! ## Does not own
//! - The `tools/call` gate over an MCP client (see the `protocol` gate tests)
//! - What `tools/list` advertises (see the tool-profile tests)
//!
//! ## Boundary
//!
//! A test here breaks when the CLI's dispatch starts reading the advertised
//! view, or when the default preset starts carrying the verb it runs.
//!
//! Keep this module focused on the CLI dispatch exemption.

use crate::cli_read_test_support::{run_state, state};
use clio_types::ToolPreset;
use std::process::ExitCode;

#[test]
fn a_verb_outside_the_default_profile_still_runs() {
    // The premise: `stats` is not in the preset the default configuration
    // resolves, so this case only passes when the CLI skips the view.
    assert!(
        !ToolPreset::Minimal.tools().contains(&"stats"),
        "`stats` belongs to the minimal preset, so this case proves nothing"
    );
    let (code, out, err) = run_state("stats", &["--output", "json"], &state(), false);
    assert_eq!(code, ExitCode::from(0), "stderr: {err}");
    assert!(out.contains("\"ok\":true"), "{out}");
}
