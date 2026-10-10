// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the shared token estimator.
//!
//! ## Owns
//! - `estimate_tokens` rounding and Unicode behavior
//!
//! ## Boundary
//!
//! Estimator changes should normally update tests here.

use crate::tokens::estimate_tokens;

#[test]
fn rounds_up_and_handles_empty() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("a"), 1);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("abcde fgh"), 3);
}

#[test]
fn counts_unicode_scalar_values() {
    assert_eq!(estimate_tokens("héllo"), 2);
}
