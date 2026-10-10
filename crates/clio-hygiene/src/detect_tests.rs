// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the noise signal detectors and preview masking
//! (`detect.rs`).
//!
//! ## Owns
//! - Test coverage for secret-class detection, pattern-kind detection, and
//!   preview masking
//!
//! ## Does not own
//! - Scoring composition (`scoring_tests.rs`)
//! - The detector behavior itself (`detect.rs`)
//!
//! ## Boundary
//!
//! Tests here break when detection changes; they never change it.
//!
//! Keep this module focused on `detect.rs` tests.

use crate::detect::{MAX_PREVIEW_CHARS, detect_noise, masked_preview};

#[test]
fn secret_classes_are_detected() {
    let labeled = "db connect failed for api_key: sk-live-abcdefgh1234";
    assert!(detect_noise(labeled).secret_hit());
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpA...";
    assert!(detect_noise(pem).secret_hit());
    let bearer = "call with bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefgh";
    assert!(detect_noise(bearer).secret_hit());
    let token = "deploy key Zx3_9kQ2-wLp7RtYvBnM4cDf8GhJ5KmNpQrStUvWx12";
    assert!(detect_noise(token).secret_hit());
}

#[test]
fn plain_text_is_not_secret() {
    assert!(!detect_noise("service listens on port 8080").secret_hit());
    assert!(!detect_noise("user prefers tabs over spaces").secret_hit());
}

#[test]
fn pattern_classes_are_detected() {
    let spam = "$ ls\n$ cd /tmp\n$ rm -rf build";
    assert!(detect_noise(spam).pattern_kinds.contains(&"terminal_spam"));
    let stack = "thread 'main' panicked at src/lib.rs:42:\nat src/store.rs:100\nat src/db.rs:33";
    assert!(detect_noise(stack).pattern_kinds.contains(&"stack_trace"));
    let hb = "heartbeat ok\nheartbeat ok\nheartbeat ok";
    assert!(detect_noise(hb).pattern_kinds.contains(&"heartbeat"));
    let dump = "$ npm run build\n$ npm test\n$ cargo test\n$ git status\n$ make check";
    assert!(detect_noise(dump).pattern_kinds.contains(&"command_dump"));
    let clean = detect_noise("clean lesson: cache before query");
    assert!(clean.pattern_kinds.is_empty(), "{:?}", clean.pattern_kinds);
}

#[test]
fn ansi_escape_is_terminal_spam() {
    let text = "\u{1b}[31merror\u{1b}[0m boot failed";
    assert!(detect_noise(text).pattern_kinds.contains(&"terminal_spam"));
}

#[test]
fn secret_preview_is_fully_redacted() {
    let secret = "api_key: sk-live-abcdefgh1234 and more context after";
    assert!(detect_noise(secret).secret_hit());
    assert_eq!(masked_preview(secret, true).as_deref(), Some("[REDACTED]"));
    // Fail closed: the scrubber itself marks labeled secrets secret-class, so
    // a "non-secret" preview never carries a raw labeled key either.
    let preview = masked_preview(secret, false).expect("preview present");
    assert!(!preview.contains("sk-live-abcdefgh1234"));
}

#[test]
fn non_secret_preview_is_truncated_and_scrubbed() {
    let long = "x".repeat(MAX_PREVIEW_CHARS + 50);
    let preview = masked_preview(&long, false).expect("preview");
    assert_eq!(preview.chars().count(), MAX_PREVIEW_CHARS);
    assert_eq!(
        masked_preview("   ", false),
        None,
        "empty text yields no preview"
    );
}
