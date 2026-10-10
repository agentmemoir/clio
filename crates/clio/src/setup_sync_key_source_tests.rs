// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync-key store question: the flag
//! that answers it, the recommended store on a non-interactive run, going back,
//! and the preview line.
//!
//! ## Owns
//! - Question behavior under test for `setup_sync_key_source`
//!
//! ## Does not own
//! - Where the step sits in the plan (see `setup_steps_sync_tests`)
//! - The other questions (see `setup_step_ask_tests`)
//!
//! ## Boundary
//!
//! Changes to what the question asks should normally update tests here. Changes
//! to the step order should not require changes here.
//!
//! Keep this module focused on the one question.

use super::*;
use crate::setup_preview_fixtures::{Harness, SERVICES, SQLITE};
use serde_json::json;

/// A multi-host effective config: the shape that puts the question in play.
fn multi_host() -> serde_json::Value {
    json!({ "sync": { "omit": false } })
}

/// The wizard records where the key will live and does not create it, so the
/// preview must not claim a file exists.
#[test]
fn the_summary_points_at_keygen_rather_than_claiming_a_created_file() {
    let file = summary(KeySource::File, &|_| None);
    assert!(file.contains("clio sync keygen"), "{file}");
    assert!(file.contains("the file store"), "{file}");
    assert!(
        !file.contains("a sync key file at"),
        "the run records the store and does not create the file: {file}"
    );

    let keystore = summary(KeySource::Keystore, &|_| None);
    assert!(
        keystore.contains("operating system's secret store"),
        "{keystore}"
    );
    assert!(keystore.contains("clio sync keygen"), "{keystore}");
    assert_ne!(file, keystore);
}

/// The printed path comes from the environment the run will record through, not
/// from the process environment.
#[test]
fn the_summary_resolves_its_path_from_the_runs_own_environment() {
    let env = |key: &str| {
        (key == clio_config::SYNC_KEY_FILE_ENV).then(|| "/run/dir/clio-sync.key".to_owned())
    };

    let line = summary(KeySource::File, &env);

    assert!(line.contains("/run/dir/clio-sync.key"), "{line}");
}

#[test]
fn an_unknown_store_flag_is_a_usage_refusal() {
    let run = Harness::fresh()
        .with_effective(multi_host())
        .flag("sync-store", "smartcard")
        .typing(SQLITE)
        .typing(SERVICES)
        .drive();

    assert!(
        matches!(
            run.signal(),
            Some(crate::setup_render::Signal::Usage(reason))
                if reason.contains("--sync-store must be file|keystore")
        ),
        "expected a usage refusal, got {:?}",
        run.signal()
    );
}

#[test]
fn a_non_interactive_multi_host_run_takes_the_recommended_store() {
    // A non-interactive run answers by flag, so no input is scripted at all.
    let run = Harness::fresh()
        .with_effective(multi_host())
        .non_interactive()
        .flag("database", "sqlite")
        .flag("embed", "local")
        .flag("rerank", "skip")
        .flag("extract", "skip")
        .run();

    assert_eq!(run.answers().sync_key_source, Some(KeySource::File));
    assert!(
        !run.out.contains("Where should Clio keep the sync key"),
        "{}",
        run.out
    );
}

#[test]
fn going_back_at_the_sync_question_re_enters_it() {
    // `b` at the question goes back one step; the encryption key was answered by
    // flag, so the walk re-enters the sync question and the next line answers it.
    let run = Harness::fresh()
        .with_effective(multi_host())
        .typing("b\n1\n")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();

    assert_eq!(run.answers().sync_key_source, Some(KeySource::File));
}
