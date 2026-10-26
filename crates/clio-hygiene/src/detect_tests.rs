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

use crate::detect::{MAX_PREVIEW_CHARS, NoiseSignals, detect_noise, masked_preview};

#[test]
fn secret_classes_are_detected() {
    let labeled = "db connect failed for api_key: sk-live-abcdefgh1234";
    assert!(detect_noise(labeled).secret_hit());
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpA...";
    assert!(detect_noise(pem).secret_hit());
    let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefgh";
    let bearer = detect_noise(&format!("call with bearer {token}"));
    assert!(
        bearer.secret_kinds == vec!["bearer_token"],
        "a bearer credential is the only class detected"
    );
    assert!(
        bearer.secret_run_len == token.len(),
        "the measured credential run is the token"
    );
    let known = "deploy key ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    assert!(detect_noise(known).secret_hit());
}

#[test]
fn bearer_prose_is_not_a_credential() {
    let prose = "GET /api/v1/profile failed: the client sent bearer auth without a token and the retry returned 401 again";
    let signals = detect_noise(prose);
    assert!(
        signals.secret_kinds.is_empty(),
        "prose about bearer auth is not secret-class"
    );
    assert!(!signals.secret_hit());
    assert!(signals.secret_run_len == 0, "no credential run measured");
}

#[test]
fn token_shaped_run_is_not_a_credential() {
    let bare = "Zx3_9kQ2-wLp7RtYvBnM4cDf8GhJ5KmNpQrStUvWx12";
    let signals = detect_noise(bare);
    assert!(
        signals.secret_kinds.is_empty(),
        "bare token run is not secret-class"
    );
    assert!(!signals.secret_hit());
    assert!(
        signals.token_run_len == Some(bare.len()),
        "the whole text is one token-shaped run"
    );
    assert!(signals.secret_run_len == 0, "no credential value measured");
}

#[test]
fn long_hyphenated_id_is_not_a_credential() {
    let id = "failure-item-agents-clb016-long-id-probe-task-with-long-identifier-1792665330634632000-1792665446249679000-0";
    let bare = detect_noise(id);
    assert!(
        bare.secret_kinds.is_empty(),
        "long hyphenated id is not secret-class"
    );
    assert!(!bare.secret_hit());
    assert!(bare.token_run_len.is_some());
    let prose = detect_noise(&format!("clb016 ordinary failure text {id}"));
    assert!(
        prose.secret_kinds.is_empty(),
        "prose with long id is not secret-class"
    );
    assert!(!prose.secret_hit());
}

#[test]
fn known_secret_format_requires_a_token_boundary() {
    assert!(!detect_noise("task-list-risk-free").secret_hit());
    assert!(detect_noise("ghp_abcdefghijklmnopqrstuvwxyz0123456789").secret_hit());
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
    let signals = detect_noise(secret);
    assert!(signals.secret_hit());
    assert!(
        masked_preview(secret, &signals).as_deref() == Some("[REDACTED]"),
        "a secret-class preview is fully redacted"
    );
    // Fail closed: the scrubber itself marks labeled secrets secret-class, so
    // a "non-secret" preview never carries a raw labeled key either.
    let preview = masked_preview(secret, &NoiseSignals::default()).expect("preview present");
    assert!(!preview.contains("sk-live-abcdefgh1234"));
}

#[test]
fn non_secret_preview_is_truncated_and_scrubbed() {
    let long = "x".repeat(MAX_PREVIEW_CHARS + 50);
    let preview = masked_preview(&long, &NoiseSignals::default()).expect("preview");
    assert_eq!(preview.chars().count(), MAX_PREVIEW_CHARS);
    assert!(
        masked_preview("   ", &NoiseSignals::default()).is_none(),
        "empty text yields no preview"
    );
}

#[test]
fn proxy_preview_masks_the_run() {
    let run = "Zx3_9kQ2-wLp7RtYvBnM4cDf8GhJ5KmNpQrStUvWx12";
    let text = format!("scratch note about cache warmup {run} and the rest of the note");
    let signals = detect_noise(&text);
    assert!(signals.secret_kinds.is_empty(), "no credential evidence");
    assert!(
        signals.token_run_len == Some(run.len()),
        "the run is measured as a token run"
    );
    let preview = masked_preview(&text, &signals).expect("preview");
    assert!(!preview.contains(run), "the run is masked in the preview");
    assert!(
        preview.contains("scratch note about cache warmup"),
        "the prose around the run survives masking"
    );
    assert!(preview.contains("****"), "the masked run is starred out");
    assert!(
        preview.contains(&run[..4]),
        "first 4 chars of the run survive"
    );
    assert!(
        preview.contains(&run[run.len() - 4..]),
        "last 4 chars of the run survive"
    );
    // Masking a proxy never drops the labelled-key scrub: a key that is not
    // secret-class is still scrubbed before the preview is returned.
    let labelled =
        masked_preview("api_key: sk-live-abcdefgh1234", &NoiseSignals::default()).expect("preview");
    assert!(
        !labelled.contains("sk-live-abcdefgh1234"),
        "a labelled key is scrubbed even when not secret-class"
    );
}
