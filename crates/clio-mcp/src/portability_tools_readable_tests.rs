// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the readable-export MCP gate test: a
//! `dsar_plaintext` export without the explicit opt-in is refused and writes no
//! file.
//!
//! ## Owns
//! - The readable opt-in refusal over the shared portability test state
//!
//! ## Does not own
//! - The shared state, call, and probe helpers (see `portability_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::portability_tools_tests::{call, state, store_probe, temp_path};
use serde_json::{Value, json};

/// The X25519 public key matching a fixed test secret, as base64 recipient text.
const REAL_RECIPIENT: &str = "UT9mObPoTo21MOhLumiTc9o7bz7PRd8yQUc41Y15LWY=";

/// An all-zero curve point: every shared secret it agrees is all zero.
const LOW_ORDER_RECIPIENT: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

#[test]
fn export_readable_requires_the_opt_in() {
    let s = state();
    assert_eq!(store_probe(&s, "pexp-ro")["ok"], Value::Bool(true));
    let dest = temp_path("readable-no-optin");
    let out = call(
        &s,
        "export",
        json!({
            "destination": dest,
            "bank": "bank-a",
            "actor": "harness",
            "content_mode": "dsar_plaintext",
            "recipient": REAL_RECIPIENT
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
    assert!(!std::path::Path::new(&dest).exists());
}

#[test]
fn export_readable_rejects_a_low_order_recipient() {
    let s = state();
    let dest = temp_path("readable-low-order");
    let out = call(
        &s,
        "export",
        json!({
            "destination": dest,
            "bank": "bank-a",
            "actor": "harness",
            "content_mode": "dsar_plaintext",
            "readable_export_opt_in": true,
            "recipient": LOW_ORDER_RECIPIENT
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "invalid_argument", "{out}");
    assert!(!std::path::Path::new(&dest).exists());
}
