// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI page-limit contract end to end for `inspect`: a
//! limit above the cap is served at the cap with the requested value
//! disclosed, a limit at the cap carries no disclosure, and a non-positive
//! limit is refused at the CLI layer before the store.
//!
//! ## Owns
//! - The `inspect` clamp-and-disclose behavior over `run_with_state`
//! - `add_requested_limit`'s disclosure condition
//!
//! ## Does not own
//! - The shared `page_limit` clamp (`cli_read_args_page_limit_tests`)
//! - The hygiene audit's clamp disclosure (`cli_read_hygiene_tests_limit_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the CLI page-limit contract for `inspect`.

use super::*;

/// A parsed `--limit` line (or, with `None`, a line without the flag).
fn parsed_limit(value: Option<&str>) -> ParsedArgs {
    let tokens: Vec<String> = match value {
        Some(value) => vec!["--limit".to_owned(), value.to_owned()],
        None => Vec::new(),
    };
    crate::cli_args::parse(&crate::cli_args::FlagSpec::new(&["limit"], &[]), tokens).expect("parse")
}

/// `--limit 201` is served as the 200 cap, and the JSON page names the value
/// the caller asked for next to the served one.
#[test]
fn inspect_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let state = state_with_items();
    let (_, out, _) = run_state(
        "inspect",
        &["--bank", "bank-a", "--limit", "201"],
        &state,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["limit"], json!(200), "{page}");
    assert_eq!(page["requested_limit"], json!(201), "{page}");

    // The text header shows the lowering on the established `items:` line.
    let (_, text, _) = run_state(
        "inspect",
        &["--bank", "bank-a", "--limit", "201"],
        &state,
        true,
    );
    assert!(
        text.contains("items: 2 of 2 (limit 200, lowered from 201)"),
        "{text}"
    );
}

/// A limit at or below the cap is served as typed and carries no disclosure.
#[test]
fn inspect_serves_an_in_cap_limit_undisclosed() {
    let state = state_with_items();
    let (_, out, _) = run_state(
        "inspect",
        &["--bank", "bank-a", "--limit", "5"],
        &state,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["limit"], json!(5), "{page}");
    assert_eq!(page.get("requested_limit"), None, "{page}");
}

/// `--limit 0` is refused at the CLI layer as a usage error, before the store
/// is asked for a page of nothing.
#[test]
fn inspect_refuses_a_zero_limit_at_the_cli_layer() {
    let state = state_with_items();
    let (code, out, err) = run_state(
        "inspect",
        &["--bank", "bank-a", "--limit", "0"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("positive page size"), "{err}");
}

/// The decorator fires exactly when the typed value exceeds the cap, and an
/// absent `--limit` has nothing to disclose.
#[test]
fn add_requested_limit_discloses_only_a_lowered_request() {
    let mut lowered = json!({"limit": 200});
    add_requested_limit(&mut lowered, &parsed_limit(Some("500")));
    assert_eq!(lowered["requested_limit"], json!(500));

    let mut in_cap = json!({"limit": 200});
    add_requested_limit(&mut in_cap, &parsed_limit(Some("200")));
    add_requested_limit(&mut in_cap, &parsed_limit(Some("50")));
    assert_eq!(in_cap.get("requested_limit"), None, "{in_cap}");

    let mut omitted = json!({"limit": 50});
    add_requested_limit(&mut omitted, &parsed_limit(None));
    assert_eq!(omitted.get("requested_limit"), None, "{omitted}");
}
