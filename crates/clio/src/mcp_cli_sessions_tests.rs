// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `--sessions` switch on `clio mcp http`: bare it turns
//! sessions on, and any value on it is refused rather than read as a choice.
//!
//! ## Owns
//! - Assertions over `refuse_sessions_value` for absent, bare, and valued forms
//!
//! ## Does not own
//! - The HTTP transport or its session handling (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the switch's accepted forms should normally require changes here.
//! Changes to the transport should not.
//!
//! Keep this module focused on the `--sessions` value rule.

use super::refuse_sessions_value;
use std::collections::HashMap;

fn flags(sessions: Option<&str>) -> HashMap<String, String> {
    let mut flags = HashMap::new();
    if let Some(value) = sessions {
        flags.insert("sessions".to_owned(), value.to_owned());
    }
    flags
}

#[test]
fn an_absent_or_bare_sessions_switch_is_accepted() {
    assert!(refuse_sessions_value(&flags(None)).is_ok());
    assert!(refuse_sessions_value(&flags(Some("true"))).is_ok());
}

#[test]
fn a_value_on_the_sessions_switch_is_refused_by_name() {
    for value in ["false", "extra"] {
        let err = refuse_sessions_value(&flags(Some(value))).expect_err("a value is refused");
        assert_eq!(err.message(), "--sessions takes no value", "value {value}");
    }
}
