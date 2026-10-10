// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the sync key's place in the step
//! plan: when the question exists, where it sits, and what a recorded answer
//! means.
//!
//! ## Owns
//! - Sync-key plan behavior under test for `setup_steps`
//!
//! ## Does not own
//! - The rest of the step list (see `setup_steps_tests`)
//! - What the question asks (see `setup_step_ask_tests`)
//!
//! ## Boundary
//!
//! Changes to when the sync key is asked should normally update tests here.
//! Changes to a single other question should not.
//!
//! Keep this module focused on the sync key's place in the plan.

use super::*;

#[test]
fn the_sync_key_question_is_in_the_plan_only_when_sync_is_in_play() {
    // Single-host is the default and the common path, so the plan must not grow
    // a question for it.
    assert!(!Answers::default().plan().contains(&Step::SyncKeySource));

    let multi_host = Answers {
        sync_in_play: true,
        ..Answers::default()
    };
    let plan = multi_host.plan();
    let key = plan
        .iter()
        .position(|step| *step == Step::KeySource)
        .expect("the encryption key is asked on a fresh machine");
    assert_eq!(
        plan.get(key + 1),
        Some(&Step::SyncKeySource),
        "the sync key is asked directly after the encryption key: {plan:?}"
    );
}

#[test]
fn an_answered_sync_key_store_is_not_asked_again() {
    let answered = Answers {
        sync_in_play: true,
        sync_key_source: Some(clio_config::KeySource::Keystore),
        ..Answers::default()
    };
    assert!(!answered.plan().contains(&Step::SyncKeySource));
}

#[test]
fn going_back_from_the_sync_key_step_keeps_the_choice_on_record() {
    // The record lives beside the sync key file, and dropping it would make the
    // question come back, which is the one thing a recorded choice must never do.
    let mut answers = Answers {
        sync_in_play: true,
        sync_key_source: Some(clio_config::KeySource::File),
        ..Answers::default()
    };
    answers.drop_from(Step::SyncKeySource);
    assert_eq!(answers.sync_key_source, Some(clio_config::KeySource::File));
}

#[test]
fn the_sync_key_step_is_its_own_section() {
    assert_eq!(Step::SyncKeySource.section(), "Sync key");
    assert_ne!(Step::SyncKeySource.section(), Step::KeySource.section());
}
