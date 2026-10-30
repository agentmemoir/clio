// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins the bank scope of the MCP hygiene read tools: a call from
//! a credential restricted to one bank is refused with `forbidden` when its
//! arguments name another bank, instead of running against it.
//!
//! ## Owns
//!
//! - The `hygiene_audit` and `hygiene_log_list` refusal on a foreign bank
//!   (`forbidden` error code) and the positive control on the named bank
//!
//! ## Does not own
//!
//! - The authorization rule itself (`runtime_context::resolve_ctx`)
//! - Argument decoding and response shapes (`hygiene_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the hygiene tools stop going through
//! `resolve_ctx`; they never change the rule.
//!
//! Keep this module focused on the hygiene tools' bank scope.

use super::hygiene_tools_tests::state;
use crate::hygiene_tools::dispatch_inner;
use crate::principal::Principal;
use crate::runtime::McpState;
use clio_types::{Actor, ErrorCode};
use serde_json::json;
use std::sync::Arc;

/// The same state, scoped to one credential that may reach `bank-mcp` only.
fn bank_scoped() -> Arc<McpState> {
    Arc::new(state().with_principal(Principal::new(
        "bank-key".to_owned(),
        Actor::Agent,
        vec!["bank-mcp".to_owned()],
    )))
}

#[test]
fn hygiene_audit_refuses_a_foreign_bank_as_forbidden() {
    let st = bank_scoped();
    let err = dispatch_inner(&st, "hygiene_audit", &json!({ "bank": "bank-other" }))
        .expect_err("a foreign bank must be refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err}");
}

#[test]
fn hygiene_log_list_refuses_a_foreign_bank_as_forbidden() {
    let st = bank_scoped();
    let err = dispatch_inner(&st, "hygiene_log_list", &json!({ "bank": "bank-other" }))
        .expect_err("a foreign bank must be refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err}");
}

#[test]
fn a_bank_scoped_credential_still_reaches_its_named_bank() {
    let st = bank_scoped();
    let audit = dispatch_inner(&st, "hygiene_audit", &json!({ "bank": "bank-mcp" }));
    assert!(audit.is_ok(), "{audit:?}");
    let listed = dispatch_inner(&st, "hygiene_log_list", &json!({ "bank": "bank-mcp" }));
    assert!(listed.is_ok(), "{listed:?}");
}
