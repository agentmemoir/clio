// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill remover's path and retention decisions: the happy path with and without the directory this installation created, retention for a harness set, the no-op cases, the boundaries of a recorded file set, and the flow that never touches the server registration. Every refusal case lives in `skill_uninstall_refusal_tests`.
//!
//! ## Owns
//! - Assertions over `uninstall_skill_only` and its outcome text
//! - The wiring assertion that `--skill-only` skips both registration steps
//!
//! ## Does not own
//! - The remover under test (`skill_uninstall`)
//! - The refusal cases (`skill_uninstall_refusal_tests`)
//! - The scratch home and marker helpers (`skill_uninstall_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the system temporary directory, so no test touches the real home directory.
//!
//! Keep this module focused on the remover's successful and no-op paths.

use super::*;
use crate::agent_harness::{HarnessRecord, resolve};
use crate::agent_harness_targets::DirectWriteEnv;
use crate::skill_install::install;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall_test_support::{
    Scratch, env, native_package, portable_package, read_marker, write_marker,
};
use crate::skill_uninstall_text::outcome_text;
use std::fs;
use std::process::ExitCode;

fn claude() -> &'static HarnessRecord {
    resolve("claude").expect("claude is supported")
}

fn codex() -> &'static HarnessRecord {
    resolve("codex").expect("codex is supported")
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_owned()).collect()
}

/// Install the real package for `record` into the scratch home.
fn write_package(record: &HarnessRecord, env: &DirectWriteEnv) {
    install(record, env, false).expect("the skill package installs");
}

#[test]
fn removes_a_package_directory_this_installation_created() {
    let scratch = Scratch::new("created");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    let native = native_package(&scratch.root);
    assert!(native.is_dir(), "the install created the native package");

    let outcomes = uninstall_skill_only(Some(claude()), &env).expect("the package is removed");

    assert_eq!(outcomes.len(), 2, "claude reads two roots");
    assert!(matches!(
        &outcomes[0],
        RemovalOutcome::Removed {
            removed_dir: true,
            files,
            ..
        } if files.len() == clio_mcp::skill_assets::SKILL_ASSETS.len()
    ));
    assert!(matches!(
        &outcomes[1],
        RemovalOutcome::Removed {
            removed_dir: true,
            ..
        }
    ));
    assert!(!native.exists(), "the created directory is removed");
    assert!(!portable_package(&scratch.root).exists());
    let text = outcome_text(&outcomes);
    assert!(
        text.contains("; removed the package directory"),
        "the text names the directory removal: {text}"
    );
}

#[test]
fn leaves_the_package_directory_when_the_installation_did_not_create_it() {
    let scratch = Scratch::new("not-created");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    for dir in [
        native_package(&scratch.root),
        portable_package(&scratch.root),
    ] {
        let mut marker = read_marker(&dir);
        marker.created_package_dir = false;
        write_marker(&dir, &marker);
    }

    let outcomes = uninstall_skill_only(Some(claude()), &env).expect("the package is removed");

    for outcome in &outcomes {
        assert!(
            matches!(
                outcome,
                RemovalOutcome::Removed {
                    removed_dir: false,
                    ..
                }
            ),
            "{outcome:?}"
        );
    }
    for dir in [
        native_package(&scratch.root),
        portable_package(&scratch.root),
    ] {
        assert!(dir.is_dir(), "the directory stays: {}", dir.display());
        assert!(
            fs::read_dir(&dir)
                .expect("the directory reads")
                .next()
                .is_none(),
            "only an empty directory stays: {}",
            dir.display()
        );
    }
}

#[test]
fn retains_the_shared_copy_while_another_harness_names_it() {
    let scratch = Scratch::new("retain-two");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    write_package(codex(), &env);
    let portable = portable_package(&scratch.root);

    let outcomes =
        uninstall_skill_only(Some(claude()), &env).expect("claude's own copy is removed");

    assert!(matches!(&outcomes[0], RemovalOutcome::Removed { .. }));
    match &outcomes[1] {
        RemovalOutcome::Retained { dir, remaining } => {
            assert_eq!(dir, &portable);
            assert_eq!(remaining.iter().cloned().collect::<Vec<_>>(), ["codex"]);
        }
        other => panic!("expected a retained copy, got {other:?}"),
    }
    assert!(portable.join("SKILL.md").is_file(), "the assets stay");
    assert_eq!(
        read_marker(&portable)
            .harnesses
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["codex"],
        "the marker drops the harness this run named"
    );
    assert!(
        outcome_text(&outcomes).contains("still referenced by: codex"),
        "{}",
        outcome_text(&outcomes)
    );
}

#[test]
fn retains_a_non_empty_set_when_no_harness_is_named() {
    let scratch = Scratch::new("retain-none");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    let native = native_package(&scratch.root);
    let portable = portable_package(&scratch.root);

    let outcomes = uninstall_skill_only(None, &env).expect("the portable copy is inspected");

    assert_eq!(
        outcomes.len(),
        1,
        "no harness reads the portable root alone"
    );
    match &outcomes[0] {
        RemovalOutcome::Retained { dir, remaining } => {
            assert_eq!(dir, &portable);
            assert_eq!(remaining.iter().cloned().collect::<Vec<_>>(), ["claude"]);
        }
        other => panic!("expected a retained copy, got {other:?}"),
    }
    assert!(portable.join("SKILL.md").is_file());
    assert!(
        native.join("SKILL.md").is_file(),
        "the native copy is untouched"
    );
}

#[test]
fn a_second_run_reports_a_no_op() {
    let scratch = Scratch::new("second-run");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    uninstall_skill_only(Some(claude()), &env).expect("the first run removes the package");

    let outcomes = uninstall_skill_only(Some(claude()), &env).expect("the second run is a no-op");

    assert_eq!(outcomes.len(), 2);
    assert!(matches!(&outcomes[0], RemovalOutcome::NoOp { .. }));
    assert!(matches!(&outcomes[1], RemovalOutcome::NoOp { .. }));
    let text = outcome_text(&outcomes);
    assert!(text.contains("nothing to remove"), "{text}");
}

#[test]
fn an_absent_marker_reports_a_no_op() {
    let scratch = Scratch::new("no-marker");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    for dir in [
        native_package(&scratch.root),
        portable_package(&scratch.root),
    ] {
        fs::remove_file(dir.join(SKILL_MARKER_FILE)).expect("the marker is removed");
    }

    let outcomes = uninstall_skill_only(Some(claude()), &env).expect("an absent marker is a no-op");

    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, RemovalOutcome::NoOp { .. }))
    );
    assert!(native_package(&scratch.root).join("SKILL.md").is_file());
}

#[test]
fn removal_keeps_an_unrelated_same_named_sibling() {
    let scratch = Scratch::new("sibling");
    let env = env(&scratch.root);
    write_package(claude(), &env);
    let sibling = scratch
        .root
        .join(".agents")
        .join("skills")
        .join("other-package")
        .join("SKILL.md");
    fs::create_dir_all(sibling.parent().expect("the sibling has a parent"))
        .expect("the sibling directory is created");
    fs::write(&sibling, b"another package").expect("the sibling is written");

    uninstall_skill_only(Some(claude()), &env).expect("the recorded package is removed");

    assert!(sibling.is_file(), "a sibling outside the package survives");
}

#[test]
fn skill_only_never_calls_a_registration_step() {
    let scratch = Scratch::new("wiring");
    let direct_env = env(&scratch.root);
    write_package(claude(), &direct_env);
    let home = scratch.root.clone();

    let code = crate::install_cli::run_with_skills(
        crate::install_cli_decide::Verb::Uninstall,
        &args(&["--skill-only", "--harness", "claude"]),
        &|| panic!("skill-only must not resolve the executable"),
        &|| env(&home),
        &|_, _| panic!("skill-only must not register"),
        &|_| panic!("skill-only must not unregister"),
        &|_, _, _| panic!("skill-only must not write assets"),
    );

    assert_eq!(code, ExitCode::SUCCESS, "skill-only removal succeeds");
    assert!(
        !native_package(&scratch.root).exists(),
        "the package is gone"
    );
}

#[test]
fn a_plain_uninstall_still_needs_a_harness() {
    let code = crate::install_cli::run_with_skills(
        crate::install_cli_decide::Verb::Uninstall,
        &args(&[]),
        &|| panic!("a usage error stops before the executable lookup"),
        &|| panic!("a usage error stops before the filesystem"),
        &|_, _| panic!("a usage error stops before registration"),
        &|_| panic!("a usage error stops before unregistration"),
        &|_, _, _| panic!("a usage error stops before the skill step"),
    );

    assert_eq!(
        code,
        ExitCode::from(2),
        "a missing --harness stays a usage error"
    );
}
