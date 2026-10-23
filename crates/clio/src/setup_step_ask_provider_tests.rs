// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the provider flag's behavior under test: the
//! flag naming the answer, the stored value kept without one, the flag winning
//! their conflict, and a misspelled value refused before the key is asked.
//!
//! ## Owns
//! - Provider flag behavior under test for `setup_step_ask`
//!
//! ## Does not own
//! - The endpoint, model, and key questions (see `setup_step_ask_tests`)
//! - What a run writes (see `setup_wizard_apply_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to how the provider flag is read should normally update tests here.
//! Changes to the other service fields should not.
//!
//! Keep this module focused on the provider flag.

use crate::setup_preview_fixtures::Harness;
use serde_json::json;

/// A hosted embedding and a hosted rerank, every value answered by flag, with no
/// provider named: the shape a flags-only run has before it says which provider
/// to use.
const HOSTED_BY_FLAG: [(&str, &str); 11] = [
    ("database", "existing"),
    ("database-connect", "pasted"),
    (
        "database-url",
        "postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full",
    ),
    ("embed", "online"),
    ("embed-url", "https://embed.example.com"),
    ("embed-model", "text-embedding-3-small"),
    ("embed-key", "sk-embed-flag-1234567890"),
    ("rerank", "online"),
    ("rerank-url", "https://rerank.example.com"),
    ("rerank-model", "bge-rerank"),
    ("rerank-key", "sk-rerank-flag-1234567890"),
];

/// The same run without the embedding key, so a refusal cannot be blamed on it.
const HOSTED_WITHOUT_KEY: [(&str, &str); 10] = [
    ("database", "existing"),
    ("database-connect", "pasted"),
    (
        "database-url",
        "postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full",
    ),
    ("embed", "online"),
    ("embed-url", "https://embed.example.com"),
    ("embed-model", "text-embedding-3-small"),
    ("rerank", "online"),
    ("rerank-url", "https://rerank.example.com"),
    ("rerank-model", "bge-rerank"),
    ("rerank-key", "sk-rerank-flag-1234567890"),
];

#[test]
fn a_provider_flag_names_the_answer_on_a_run_with_no_config() {
    // The provider has no question of its own, so its flag is the only place an
    // operator names one. With no stored config there is nothing to fall back
    // to, which is the run where the role default used to answer instead.
    let run = Harness::fresh()
        .non_interactive()
        .flags(HOSTED_BY_FLAG)
        .flag("embed-provider", "tei")
        .flag("rerank-provider", "cohere")
        .run();
    let answers = run.answers();
    assert_eq!(
        answers
            .embed
            .as_ref()
            .expect("a hosted embedding")
            .provider
            .as_deref(),
        Some("tei"),
        "{}",
        run.out
    );
    assert_eq!(
        answers
            .rerank
            .as_ref()
            .expect("a hosted rerank")
            .provider
            .as_deref(),
        Some("cohere"),
        "{}",
        run.out
    );
}

#[test]
fn a_stored_provider_is_kept_when_no_flag_names_one() {
    // The flag overrides the stored provider, it is not the only source: a run
    // that names no provider keeps the one the last run wrote.
    let run = Harness::fresh()
        .non_interactive()
        .with_effective(json!({
            "embed": { "provider": "tei" },
            "rerank": { "provider": "cohere" }
        }))
        .flags(HOSTED_BY_FLAG)
        .run();
    let answers = run.answers();
    assert_eq!(
        answers
            .embed
            .as_ref()
            .expect("a hosted embedding")
            .provider
            .as_deref(),
        Some("tei"),
        "{}",
        run.out
    );
    assert_eq!(
        answers
            .rerank
            .as_ref()
            .expect("a hosted rerank")
            .provider
            .as_deref(),
        Some("cohere"),
        "{}",
        run.out
    );
}

#[test]
fn a_provider_flag_wins_over_a_stored_provider() {
    // The reported harm was exactly this conflict: an operator passing a
    // provider got the previous run's value instead. When both speak, the flag
    // decides.
    let run = Harness::fresh()
        .non_interactive()
        .with_effective(json!({
            "embed": { "provider": "openai" },
            "rerank": { "provider": "tei" }
        }))
        .flags(HOSTED_BY_FLAG)
        .flag("embed-provider", "tei")
        .flag("rerank-provider", "cohere")
        .run();
    let answers = run.answers();
    assert_eq!(
        answers
            .embed
            .as_ref()
            .expect("a hosted embedding")
            .provider
            .as_deref(),
        Some("tei"),
        "{}",
        run.out
    );
    assert_eq!(
        answers
            .rerank
            .as_ref()
            .expect("a hosted rerank")
            .provider
            .as_deref(),
        Some("cohere"),
        "{}",
        run.out
    );
}

#[test]
fn an_unknown_provider_flag_is_refused_before_the_key_is_asked() {
    // The provider is checked before the key question, so a misspelled provider
    // names itself even when the key is missing too: without the early check a
    // non-interactive run stops at the missing key and the typo hides behind it.
    let reason = Harness::fresh()
        .non_interactive()
        .flags(HOSTED_WITHOUT_KEY)
        .flag("embed-provider", "bogus")
        .expect_refusal();
    assert!(
        reason.contains("--embed-provider must be tei|openai"),
        "{reason}"
    );
    assert!(reason.contains("bogus"), "the refusal quotes it: {reason}");

    // A blank provider is refused the same way rather than silently falling
    // back to the role default.
    let blank = Harness::fresh()
        .non_interactive()
        .flags(HOSTED_WITHOUT_KEY)
        .flag("embed-provider", " ")
        .expect_refusal();
    assert!(
        blank.contains("--embed-provider must be tei|openai"),
        "{blank}"
    );

    // The rerank side has its own vocabulary.
    let rerank = Harness::fresh()
        .non_interactive()
        .flags(HOSTED_BY_FLAG)
        .flag("rerank-provider", "bogus")
        .expect_refusal();
    assert!(
        rerank.contains("--rerank-provider must be tei|cohere"),
        "{rerank}"
    );
}

#[test]
fn a_provider_flag_in_mixed_case_is_accepted_as_the_canonical_spelling() {
    // Role flags already accept any case, so the provider flag does too: what is
    // recorded is the canonical lowercase spelling the config validates.
    let run = Harness::fresh()
        .non_interactive()
        .flags(HOSTED_BY_FLAG)
        .flag("embed-provider", "TEI")
        .flag("rerank-provider", "Cohere")
        .run();
    let answers = run.answers();
    assert_eq!(
        answers
            .embed
            .as_ref()
            .expect("a hosted embedding")
            .provider
            .as_deref(),
        Some("tei"),
        "{}",
        run.out
    );
    assert_eq!(
        answers
            .rerank
            .as_ref()
            .expect("a hosted rerank")
            .provider
            .as_deref(),
        Some("cohere"),
        "{}",
        run.out
    );
}
