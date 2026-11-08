// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill installer: the assets written, byte identity with the embedded table, idempotent re-runs, the failure text, and the order of the skill step before registration. The root each harness selects is tested in `skill_install_roots_tests`, and the marker, conflict, and `--force` contract in `skill_install_marker_tests`.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and every assertion over `install`, the outcome text, and the failure text
//! - The wiring assertions over the install flow's skill step
//!
//! ## Does not own
//! - The installer implementation under test (`skill_install`)
//! - The marker, conflict, and forced-write contract (`skill_install_marker_tests`)
//! - The harness registry (`agent_harness`) or the home seam (`agent_harness_targets`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the system temporary directory, so no test touches the real home directory or another test's files.

use super::*;
use crate::agent_harness::{self, HarnessRecord};
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::install_cli::run_with_skills;
use crate::install_cli_decide::Verb;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// One test's own directory, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-skill-install-{label}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("the scratch directory can be created");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A Linux-layout environment rooted at the scratch directory, so every resolved path lands inside the test's own directory.
fn env(home: &Path) -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, home.to_path_buf(), None, None, None)
}

fn record(name: &str) -> &'static HarnessRecord {
    agent_harness::resolve(name).expect("the harness is supported")
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_owned()).collect()
}

/// The package directory of the first written or already-current root.
fn first_dir(outcome: &SkillOutcome) -> &Path {
    match outcome {
        SkillOutcome::Written { roots } => &roots[0].dir,
        SkillOutcome::Unchanged { roots } => &roots[0],
        SkillOutcome::Refused { .. } => panic!("a refusal names no package directory to report"),
    }
}

#[test]
fn every_asset_is_written_byte_identical_to_the_embedded_table() {
    let scratch = Scratch::new("bytes");
    let outcome = install_portable(&env(&scratch.root), false).expect("the assets install");
    let dir = first_dir(&outcome);
    for (name, text) in SKILL_ASSETS {
        let bytes = fs::read(dir.join(name)).expect("the asset exists");
        assert_eq!(
            bytes,
            text.as_bytes(),
            "{name} differs from the embedded text"
        );
    }
    assert!(dir.join("references/01-retrieval.md").is_file());
    assert!(dir.join("references/17-compliance.md").is_file());
}

#[test]
fn a_second_run_writes_nothing() {
    let scratch = Scratch::new("idempotent");
    let first = install_portable(&env(&scratch.root), false).expect("the first run writes");
    assert!(
        matches!(first, SkillOutcome::Written { .. }),
        "the first run writes the package: {first:?}"
    );
    let second = install_portable(&env(&scratch.root), false).expect("the second run is safe");
    assert!(
        matches!(second, SkillOutcome::Unchanged { .. }),
        "the second run is a no-op: {second:?}"
    );
    let text = portable_outcome_text(&second);
    assert!(
        text.contains("unchanged") && text.contains("nothing written"),
        "{text}"
    );
    assert!(!text.contains("wrote"), "{text}");
}

#[test]
fn unrelated_files_outside_the_skill_directory_survive() {
    let scratch = Scratch::new("unrelated");
    let skills = scratch.root.join(".claude").join("skills");
    fs::create_dir_all(&skills).expect("the skills root can be created");
    fs::write(skills.join("keep.txt"), b"keep").expect("the marker can be written");
    let other = skills.join("other-skill");
    fs::create_dir_all(&other).expect("the other skill directory can be created");
    fs::write(other.join("SKILL.md"), b"other").expect("the other skill can be written");

    install(record("claude"), &env(&scratch.root), false).expect("the assets install");
    assert_eq!(
        fs::read(skills.join("keep.txt")).expect("the unrelated file remains"),
        b"keep"
    );
    assert_eq!(
        fs::read(other.join("SKILL.md")).expect("the other skill remains"),
        b"other"
    );
}

#[test]
fn a_write_failure_names_the_failing_path_and_cause() {
    let scratch = Scratch::new("failure");
    let skills = scratch.root.join(".claude").join("skills");
    fs::create_dir_all(&skills).expect("the skills root can be created");
    fs::write(skills.join(SKILL_DIR_NAME), b"not a directory")
        .expect("the blocking file can be written");

    let error = install(record("claude"), &env(&scratch.root), false)
        .expect_err("a file at the package path cannot be scanned as a directory");
    let message = failure_message(record("claude"), &error);
    assert!(message.contains(SKILL_DIR_NAME), "{message}");
    assert!(message.contains("failed to read"), "{message}");
}

#[test]
fn the_outcome_line_names_the_absolute_directory_and_the_count() {
    let scratch = Scratch::new("text");
    let outcome = install_portable(&env(&scratch.root), false).expect("the assets install");
    let text = portable_outcome_text(&outcome);
    assert!(
        text.contains(&first_dir(&outcome).display().to_string()),
        "{text}"
    );
    assert!(text.contains("18 skill assets"), "{text}");
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn no_harness_install_writes_only_the_portable_root() {
    let scratch = Scratch::new("portable");
    let outcome =
        install_portable(&env(&scratch.root), false).expect("the portable assets install");
    let SkillOutcome::Written { roots } = &outcome else {
        panic!("the first portable install writes: {outcome:?}");
    };
    assert_eq!(roots.len(), 1, "the portable install writes one root");
    assert_eq!(
        roots[0].dir,
        scratch
            .root
            .join(".agents")
            .join("skills")
            .join(SKILL_DIR_NAME)
    );
    assert_eq!(SKILL_ASSETS.len(), 18, "the package holds 18 assets");
    assert_eq!(
        roots[0].written.len(),
        SKILL_ASSETS.len() + 1,
        "18 assets and then the marker: {:?}",
        roots[0].written
    );
    assert!(
        !scratch.root.join(".claude").exists(),
        "no harness-native root is written"
    );
}

#[test]
fn the_portable_outcome_line_names_the_absolute_directory_and_the_count() {
    let scratch = Scratch::new("portable-text");
    let outcome =
        install_portable(&env(&scratch.root), false).expect("the portable assets install");
    let text = portable_outcome_text(&outcome);
    assert!(
        text.contains(&first_dir(&outcome).display().to_string()),
        "{text}"
    );
    assert!(text.contains("18 skill assets"), "{text}");
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn a_portable_write_failure_names_the_failing_path_and_cause() {
    let scratch = Scratch::new("portable-failure");
    let skills = scratch.root.join(".agents").join("skills");
    fs::create_dir_all(&skills).expect("the portable root can be created");
    fs::write(skills.join(SKILL_DIR_NAME), b"not a directory")
        .expect("the blocking file can be written");

    let error = install_portable(&env(&scratch.root), false)
        .expect_err("a file at the package path cannot be scanned as a directory");
    let message = portable_failure_message(&error);
    assert!(message.contains(SKILL_DIR_NAME), "{message}");
    assert!(message.contains("failed to read"), "{message}");
}

#[test]
fn the_skill_step_runs_before_registration() {
    let scratch = Scratch::new("order");
    let order = RefCell::new(Vec::new());
    let skill = |_: Option<&HarnessRecord>, _: &DirectWriteEnv, force: bool| {
        assert!(!force, "a plain install carries no force");
        order.borrow_mut().push("skill");
        Ok(())
    };
    let register = |_: &HarnessRecord, _: &Path| {
        order.borrow_mut().push("register");
        Ok("registered".to_owned())
    };

    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(PathBuf::from("clio")),
        &|| env(&scratch.root),
        &register,
        &|_: &HarnessRecord| panic!("uninstall must not run"),
        &skill,
    );

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(*order.borrow(), vec!["skill", "register"]);
}

#[test]
fn a_skill_failure_stops_registration() {
    let scratch = Scratch::new("stop");
    let skill = |_: Option<&HarnessRecord>, _: &DirectWriteEnv, _: bool| {
        Err("skill write failed".to_owned())
    };

    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(PathBuf::from("clio")),
        &|| env(&scratch.root),
        &|_: &HarnessRecord, _: &Path| panic!("registration must not run after a skill failure"),
        &|_: &HarnessRecord| panic!("uninstall must not run"),
        &skill,
    );

    assert_eq!(code, ExitCode::from(1));
}

/// An environment whose home directory is unknown, so no skill root resolves.
fn unknown_home_env() -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, PathBuf::new(), None, None, None)
}

#[test]
fn install_and_report_succeeds_for_a_resolved_root() {
    let scratch = Scratch::new("report");
    install_and_report(record("claude"), &env(&scratch.root), false).expect("the assets install");
    assert!(
        scratch
            .root
            .join(".claude")
            .join("skills")
            .join(SKILL_DIR_NAME)
            .join("SKILL.md")
            .is_file()
    );
}

#[test]
fn install_and_report_names_the_unknown_home_failure() {
    let message = install_and_report(record("claude"), &unknown_home_env(), false)
        .expect_err("an unknown home cannot resolve a skill root");
    assert!(
        message.contains("could not install the claude skill assets"),
        "{message}"
    );
    assert!(
        message.contains("cannot resolve a skill root for `claude`"),
        "{message}"
    );
}

#[test]
fn install_portable_and_report_succeeds_for_a_resolved_root() {
    let scratch = Scratch::new("portable-report");
    install_portable_and_report(&env(&scratch.root), false).expect("the portable assets install");
    assert!(
        scratch
            .root
            .join(".agents")
            .join("skills")
            .join(SKILL_DIR_NAME)
            .join("SKILL.md")
            .is_file()
    );
}

#[test]
fn install_portable_and_report_names_the_unknown_home_failure() {
    let message = install_portable_and_report(&unknown_home_env(), false)
        .expect_err("an unknown home cannot resolve the portable root");
    assert!(
        message.contains("could not install the portable clio-mcp skill assets"),
        "{message}"
    );
    assert!(
        message.contains("cannot resolve the portable skill root"),
        "{message}"
    );
}

#[test]
fn an_unknown_home_fails_install_with_a_plain_message() {
    let error = install(record("claude"), &unknown_home_env(), false)
        .expect_err("an unknown home cannot resolve a skill root");
    let message = failure_message(record("claude"), &error);
    assert!(
        message.contains("cannot resolve a skill root for `claude`"),
        "{message}"
    );
}

#[test]
fn an_unknown_home_fails_the_portable_install_with_a_plain_message() {
    let error = install_portable(&unknown_home_env(), false)
        .expect_err("an unknown home cannot resolve the portable root");
    let message = portable_failure_message(&error);
    assert!(
        message.contains("cannot resolve the portable skill root"),
        "{message}"
    );
}

#[test]
fn a_blocked_reference_directory_is_refused_by_default_and_fails_a_forced_write() {
    let scratch = Scratch::new("blocked-parent");
    let dir = scratch
        .root
        .join(".claude")
        .join("skills")
        .join(SKILL_DIR_NAME);
    fs::create_dir_all(&dir).expect("the package directory can be created");
    fs::write(dir.join("references"), b"not a directory")
        .expect("the blocking file can be written");

    let refused = install(record("claude"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");
    assert!(
        matches!(refused, SkillOutcome::Refused { .. }),
        "the unowned `references` file refuses the install: {refused:?}"
    );

    let error = install(record("claude"), &env(&scratch.root), true)
        .expect_err("a file at the reference path blocks the forced write");
    let message = failure_message(record("claude"), &error);
    assert!(message.contains("failed to create"), "{message}");
    assert!(message.contains("references"), "{message}");
}

#[test]
fn a_directory_at_an_asset_path_names_the_replace_failure() {
    let scratch = Scratch::new("blocked-asset");
    let dir = scratch
        .root
        .join(".claude")
        .join("skills")
        .join(SKILL_DIR_NAME);
    fs::create_dir_all(dir.join("SKILL.md")).expect("the blocking directory can be created");

    let error = install(record("claude"), &env(&scratch.root), false)
        .expect_err("a directory at the asset path blocks the replace");
    let message = failure_message(record("claude"), &error);
    assert!(message.contains("failed to replace"), "{message}");
    assert!(message.contains("SKILL.md"), "{message}");
}

#[test]
fn an_unparseable_skill_write_names_the_path_and_the_snippet() {
    let error = WriteError::Unparseable {
        path: PathBuf::from("/skills/clio-mcp/SKILL.md"),
        snippet: "line 4: broken".to_owned(),
    };
    let message = failure_message(record("claude"), &error);
    assert_eq!(
        message,
        "could not install the claude skill assets: /skills/clio-mcp/SKILL.md line 4: broken"
    );
}

#[test]
fn an_unparseable_portable_write_names_the_path_and_the_snippet() {
    let error = WriteError::Unparseable {
        path: PathBuf::from("/skills/clio-mcp/SKILL.md"),
        snippet: "line 4: broken".to_owned(),
    };
    let message = portable_failure_message(&error);
    assert_eq!(
        message,
        "could not install the portable clio-mcp skill assets: /skills/clio-mcp/SKILL.md line 4: broken"
    );
}
