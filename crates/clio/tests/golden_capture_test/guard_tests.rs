// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module proves the guarded release build's own behavior: the freshness
//! verdict on injected cargo output, the executable check, source-mutation
//! detection, and the one-retry rule a cold tree's first capture uses.
//!
//! ## Owns
//! - Every assertion over the guarded build: `freshness_verdict`,
//!   `require_executable`, `source_mutated`, and `guarded_build_retrying`
//!
//! ## Does not own
//! - The guard implementation itself (`golden_capture_support`)
//! - The comparator, fixture, plan, and capture assertions (`main`)
//!
//! ## Boundary
//!
//! Changes to what is asserted about the guarded build should normally require
//! changes here. Changes to how captures are compared should not.
//!
//! Keep this module focused on the guarded build's contract.

use super::golden_capture_support::{
    GuardRefusal, GuardedBuildError, Scratch, freshness_verdict, guarded_build_retrying,
    require_executable, source_mutated,
};
use std::cell::Cell;
use std::fs;

/// Injected `cargo build -p clio --release --locked --message-format=json`
/// stdout: the clio bin artifact, fresh, with cargo's executable path.
const FRESH_BUILD_JSON: &str = concat!(
    "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"clio\",\"kind\":[\"bin\"]},",
    "\"fresh\":true,\"executable\":\"/repo/target/release/clio\"}\n",
);

/// Fresh JSON — cargo reused the prepared cache — returns the artifact's executable.
#[test]
fn fresh_clio_json_verdict_returns_the_artifact_executable() {
    let verdict = freshness_verdict(0, FRESH_BUILD_JSON);
    assert_eq!(verdict, Ok("/repo/target/release/clio".to_owned()));
}

/// A `fresh: false` artifact refuses as `StaleArtifact`, naming why in its Display.
#[test]
fn rebuilt_clio_json_refuses_as_stale_artifact() {
    let stale = FRESH_BUILD_JSON.replace("\"fresh\":true", "\"fresh\":false");
    let refusal = freshness_verdict(0, &stale).expect_err("stale build must refuse");
    assert_eq!(refusal, GuardRefusal::StaleArtifact);
    let text = refusal.to_string();
    assert!(text.contains("re-run the capture"), "{text}");
}

/// Stdout without a fresh clio bin artifact refuses as `NoFreshnessProof`: empty
/// output, non-JSON lines, a lib-only artifact, and a null executable.
#[test]
fn json_without_a_fresh_bin_artifact_refuses_as_no_freshness_proof() {
    let no_proof = |label: &str, stdout: &str| {
        let refusal = freshness_verdict(0, stdout).expect_err("must refuse");
        assert_eq!(refusal, GuardRefusal::NoFreshnessProof, "{label}");
        assert!(refusal.to_string().contains("freshness proof"), "{label}");
    };
    no_proof("empty stdout", "");
    no_proof("non-JSON lines", "not json at all\n");
    let lib_only = FRESH_BUILD_JSON.replace("\"kind\":[\"bin\"]", "\"kind\":[\"lib\"]");
    no_proof("lib artifact", &lib_only);
    let null_path = FRESH_BUILD_JSON.replace("\"/repo/target/release/clio\"", "null");
    no_proof("null executable", &null_path);
}

/// A non-zero cargo exit refuses as `BuildFailed` with the exit carried through.
#[test]
fn failed_cargo_exit_refuses_as_build_failed() {
    let refusal = freshness_verdict(2, FRESH_BUILD_JSON).expect_err("failed build must refuse");
    assert_eq!(refusal, GuardRefusal::BuildFailed { exit: 2 });
    assert!(refusal.to_string().contains("build failed"), "{refusal}");
}

/// A missing path refuses as `MissingExecutable`; an existing scratch file is accepted.
#[test]
fn require_executable_refuses_a_missing_path_and_accepts_an_existing_one() {
    let scratch = Scratch::new("require-executable");
    let missing = scratch.0.join("missing").join("clio");
    let refusal = require_executable(&missing).expect_err("a missing path must refuse");
    assert_eq!(refusal, GuardRefusal::MissingExecutable);
    let text = refusal.to_string();
    assert!(text.contains("release executable is missing"), "{text}");
    let file = scratch.0.join("clio");
    fs::write(&file, b"binary").expect("write the scratch file");
    assert_eq!(require_executable(&file), Ok(()));
}

/// Identical source identity is not a mutation; any text change is.
#[test]
fn source_mutation_is_exactly_a_text_change() {
    assert!(
        !source_mutated("abc", "abc"),
        "identical text is not a mutation"
    );
    assert!(source_mutated("abc", "abd"), "changed text is a mutation");
}

/// A stale first guarded build is retried exactly once, and the artifact the
/// retry returns is the one the capture uses.
#[test]
fn a_stale_first_build_is_retried_once() {
    let calls = Cell::new(0);
    let outcome = guarded_build_retrying(|| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Err(stale_build("Compiling clio v0.1.0\n"))
        } else {
            Ok("/repo/target/release/clio".to_owned())
        }
    });
    assert_eq!(outcome.ok(), Some("/repo/target/release/clio".to_owned()));
    assert_eq!(
        calls.get(),
        2,
        "the stale build must be retried exactly once"
    );
}

/// A first build that is already fresh is returned without a second build, so
/// the retry never costs a warm tree an extra cargo invocation.
#[test]
fn a_fresh_first_build_is_not_retried() {
    let calls = Cell::new(0);
    let outcome = guarded_build_retrying(|| {
        calls.set(calls.get() + 1);
        Ok("/repo/target/release/clio".to_owned())
    });
    assert!(outcome.is_ok());
    assert_eq!(calls.get(), 1, "a fresh build must not be retried");
}

/// When both builds recompile clio the capture still refuses, and the refusal
/// carries cargo's stderr tail so the recompiled package is visible.
#[test]
fn a_second_stale_build_refuses_with_cargos_stderr() {
    let calls = Cell::new(0);
    let outcome = guarded_build_retrying(|| {
        calls.set(calls.get() + 1);
        Err(stale_build("Compiling clio v0.1.0\n"))
    });
    assert_eq!(calls.get(), 2);
    let message = outcome.expect_err("two stale builds must refuse").render();
    assert!(message.contains("recompiled clio"), "{message}");
    assert!(message.contains("Compiling clio v0.1.0"), "{message}");
}

/// A non-stale refusal is not retried: only the stale verdict means the first
/// build warmed the cache a retry needs.
#[test]
fn a_non_stale_refusal_is_not_retried() {
    let calls = Cell::new(0);
    let outcome = guarded_build_retrying(|| {
        calls.set(calls.get() + 1);
        Err(GuardedBuildError::Refused(
            "capture refused: the release build failed with exit 2".to_owned(),
        ))
    });
    assert_eq!(calls.get(), 1, "only a stale refusal is retried");
    let message = outcome.expect_err("the refusal must pass through").render();
    assert!(message.contains("build failed"), "{message}");
}

/// One `GuardedBuildError::Stale` with a cargo stderr tail: the shape the
/// guarded build returns when its build recompiled clio.
fn stale_build(cargo_stderr: &str) -> GuardedBuildError {
    GuardedBuildError::Stale {
        cargo_stderr: cargo_stderr.to_owned(),
    }
}
