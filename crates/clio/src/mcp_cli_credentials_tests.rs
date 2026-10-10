// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how `clio mcp http` loads the credential store its
//! `--token-file` flag names: which unusable states refuse the start, and which
//! usable one reaches the transport config as a credential source.
//!
//! ## Owns
//! - Credential-store load tests over the real file: absent flag, missing path,
//!   malformed file, no record, and one record
//! - The check that only a loaded store counts as a credential source
//!
//! ## Does not own
//! - The usage text (`mcp_cli_tests`)
//! - The store's record shape and its validation (`clio-config`)
//! - The credential edge over a live request (`clio-mcp`)
//!
//! ## Boundary
//!
//! Tests here break when the credential-store load or its refusals change; they
//! never change it. A change to the store's own format belongs in `clio-config`,
//! where the loader lives.
//!
//! Keep this module focused on the credential-store load.

use super::tests::{Scratch, absent_path, config_with, flags_of, lookup, record, token_file};
use super::*;

#[test]
fn an_absent_token_file_flag_keeps_the_credential_free_default() {
    let store =
        load_http_credentials(&flags_of(&["--sessions"]), &lookup(&[])).expect("no flag is fine");
    assert!(store.is_none(), "no flag must not name a store");
}

#[test]
fn a_token_file_naming_a_missing_path_is_refused() {
    let missing = absent_path("gone");
    let named = missing.display().to_string();
    let reason = load_http_credentials(&token_file(&named), &lookup(&[]))
        .expect_err("a named path that is absent refuses the start");
    assert!(reason.contains(&named), "the reason must name it: {reason}");
}

#[test]
fn a_malformed_token_file_is_refused_rather_than_read_as_empty() {
    let scratch = Scratch::new("malformed");
    std::fs::write(scratch.path(), "not json at all").expect("write raw");
    assert!(
        load_http_credentials(&token_file(&scratch.display()), &lookup(&[])).is_err(),
        "a malformed store must refuse the start"
    );
}

#[test]
fn a_token_file_holding_no_record_is_refused() {
    let scratch = Scratch::new("empty-store");
    scratch.write_store(&[]);
    let reason = load_http_credentials(&token_file(&scratch.display()), &lookup(&[]))
        .expect_err("a store that authorizes nobody refuses the start");
    assert!(reason.contains("no record"), "{reason}");
}

#[test]
fn a_loaded_store_reaches_the_transport_config_as_its_credential_source() {
    let scratch = Scratch::new("one-record");
    scratch.write_store(&[record("agent-default", "one-record-secret")]);
    // The flag alone builds no credential: only the loaded store is one.
    let mut cfg = config_with(&["--token-file", &scratch.display()]);
    assert!(!clio_mcp::http::has_credential_source(&cfg));
    cfg.credentials = Some(
        load_http_credentials(&token_file(&scratch.display()), &lookup(&[]))
            .expect("a usable store loads")
            .expect("the flag named a store"),
    );
    assert!(clio_mcp::http::has_credential_source(&cfg));
}
