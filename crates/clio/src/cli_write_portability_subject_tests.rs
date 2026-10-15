// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio export --subject` binding: the optional subject
//! selector is decoded onto the `export` tool's `subject_id` argument and left
//! absent when the flag is not given.
//!
//! ## Owns
//! - Build-argument assertions for the `--subject` flag of `cli_write_portability`
//!
//! ## Does not own
//! - The command under test (`cli_write_portability`)
//! - Bundle and subject-selection semantics (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_write_group::{WriteCall, WriteGroup};

/// The `export` build decodes `--subject` onto the tool's `subject_id`.
#[test]
fn export_binds_subject_as_subject_id() {
    let group = super::PortabilityWriteGroup;
    let parsed = crate::cli_args::parse(
        &group.spec("export"),
        crate::cli_read_test_support::tokens(&[
            "--out",
            "bundle.json",
            "--subject",
            "subj-1",
            "--actor",
            "harness",
        ]),
    )
    .expect("parse");
    let WriteCall::Tool { tool, args } = group.build("export", &parsed).expect("build") else {
        panic!("export builds a tool call");
    };
    assert_eq!(tool, "export");
    assert_eq!(args["destination"], serde_json::json!("bundle.json"));
    assert_eq!(args["subject_id"], serde_json::json!("subj-1"));
}

/// Without `--subject`, no `subject_id` argument is sent, so the bank bundle
/// path is unchanged.
#[test]
fn export_omits_subject_id_when_the_flag_is_absent() {
    let group = super::PortabilityWriteGroup;
    let parsed = crate::cli_args::parse(
        &group.spec("export"),
        crate::cli_read_test_support::tokens(&["--out", "bundle.json", "--actor", "harness"]),
    )
    .expect("parse");
    let WriteCall::Tool { args, .. } = group.build("export", &parsed).expect("build") else {
        panic!("export builds a tool call");
    };
    assert!(args.get("subject_id").is_none(), "{args}");
}
