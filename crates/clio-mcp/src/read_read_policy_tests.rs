// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the resolution of the two bank-defaulted read controls from
//! a call's raw wire arguments: the pack fidelity channel and the provenance
//! floor's absence handling.
//!
//! ## Owns
//! - `an_absent_or_null_fidelity_argument_takes_the_bank_default`
//! - `an_explicit_fidelity_argument_beats_the_bank_default`
//! - `a_non_string_fidelity_argument_is_refused_by_name`
//! - `an_absent_floor_argument_resolves_to_no_floor`
//!
//! ## Does not own
//! - The published schema text of either argument (`schema_read_props_tests`)
//! - The refusal seen over MCP, which the binding asserts end to end
//!   (`read_retrieve_exact_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on how a read control resolves from its argument.

use crate::read_read_policy::{FIDELITY_ARG, read_fidelity, read_provenance_floor};
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_config::retention_levels::PackFidelityDefault;
use clio_types::ErrorCode;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Fixture bank. Synthetic ids only: no real content.
const BANK: &str = "read-policy";

/// A retention store path under the system temp dir, unique per call.
///
/// The process id separates processes and the atomic counter separates calls
/// inside one, following the workspace's scratch-path pattern. Each fixture owns
/// its own path and removes it, so a profile write in one test cannot be read by
/// another.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-read-policy-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// A hermetic state whose retention store is a file this fixture owns.
///
/// The effective config is empty, so no embedder resolves and the profile is the
/// builtin default until this fixture writes one. The store is redirected
/// before any write, so the fixture never reads or writes the store the host
/// environment names and one test cannot see another's profile.
struct Fixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl Fixture {
    /// Open the state with no stored channel default.
    fn new() -> Self {
        Self::with_bank_default(None)
    }

    /// Open the state and store the given channel default in its own store file.
    fn with_bank_default(channel: Option<&str>) -> Self {
        let mut state = McpState::open_with_effective(
            &McpOpenOptions {
                default_bank: BANK.into(),
                ..McpOpenOptions::default()
            },
            &json!({}),
        )
        .expect("state");
        let store_path = temp_store_path();
        // A stale file from a crashed run must not seed this fixture's store.
        let _ = std::fs::remove_file(&store_path);
        Arc::get_mut(&mut state)
            .expect("the fixture state is uniquely owned")
            .set_retention_path(&store_path);
        let fixture = Self { state, store_path };
        if let Some(channel) = channel {
            let held = dispatch(
                &fixture.state,
                "retention_profile_set",
                &json!({
                    "bank": BANK,
                    "profile": {"pack_fidelity_default": channel},
                    "confirm": true
                }),
            );
            assert_eq!(held["ok"], json!(true), "{held}");
        }
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.store_path);
    }
}

impl std::ops::Deref for Fixture {
    type Target = McpState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// An absent key and an explicit `null` are the same request, so both take the
/// bank's stored default rather than silently packing `flow`.
#[test]
fn an_absent_or_null_fidelity_argument_takes_the_bank_default() {
    let exact = Fixture::with_bank_default(Some("exact"));
    assert_eq!(
        read_fidelity(&exact, BANK, None).expect("absent argument"),
        PackFidelityDefault::Exact,
        "an absent key takes the bank default"
    );
    assert_eq!(
        read_fidelity(&exact, BANK, Some(&Value::Null)).expect("explicit null"),
        PackFidelityDefault::Exact,
        "an explicit null is the same request as an absent key"
    );
    // A bank that states no other channel resolves to the enum's own default,
    // so a missing default can never leave the request without a channel.
    let flow = Fixture::with_bank_default(Some("flow"));
    assert_eq!(
        read_fidelity(&flow, BANK, None).expect("absent argument"),
        PackFidelityDefault::Flow,
        "a bank that names no other channel packs flow"
    );
}

/// An explicit argument wins over the stored default in both directions, so a
/// caller can override a bank policy per call and can restore it per call too.
#[test]
fn an_explicit_fidelity_argument_beats_the_bank_default() {
    let over_exact = Fixture::with_bank_default(Some("exact"));
    assert_eq!(
        read_fidelity(&over_exact, BANK, Some(&json!("flow"))).expect("explicit flow"),
        PackFidelityDefault::Flow,
        "an explicit flow overrides a bank default of exact"
    );
    let over_flow = Fixture::with_bank_default(Some("flow"));
    assert_eq!(
        read_fidelity(&over_flow, BANK, Some(&json!("exact"))).expect("explicit exact"),
        PackFidelityDefault::Exact,
        "an explicit exact overrides a bank default of flow"
    );
}

/// A value that is not a string names no channel, so it is refused by name
/// rather than reaching the parser, which would echo a number as text and blame
/// the stored profile field for a wire-type mistake.
#[test]
fn a_non_string_fidelity_argument_is_refused_by_name() {
    let state = Fixture::new();
    let refused = read_fidelity(&state, BANK, Some(&json!(3))).expect_err("a number is refused");
    assert_eq!(
        refused.code,
        ErrorCode::InvalidArgument,
        "a non-string value is the documented code"
    );
    assert!(
        refused.message.contains(FIDELITY_ARG),
        "the refusal names the wire argument: {}",
        refused.message
    );
}

/// The floor's absence handling is the same shape, so a bank default cannot be
/// bypassed by omitting the argument and cannot be widened by a null.
#[test]
fn an_absent_floor_argument_resolves_to_no_floor() {
    let state = Fixture::new();
    // The builtin profile states no floor, so absence resolves to no floor at
    // all rather than to the strictest rung.
    assert_eq!(
        read_provenance_floor(&state, BANK, None).expect("absent argument"),
        None,
        "a bank that states no floor resolves to no floor"
    );
    assert_eq!(
        read_provenance_floor(&state, BANK, Some(&Value::Null)).expect("explicit null"),
        None,
        "an explicit null is the same request as an absent key"
    );
}
