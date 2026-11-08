// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill installer: the root each harness selects, the assets written, byte identity with the embedded table, idempotent re-runs, refusal on a blocked path, and the order of the skill step before registration.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and every assertion over `install`, `outcome_text`, and `failure_message`
//! - The wiring assertions over the install flow's skill step
//!
//! ## Does not own
//! - The installer implementation under test (`skill_install`)
//! - The harness registry (`agent_harness`) or the home seam (`agent_harness_targets`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the system temporary directory, so no test touches the real home directory or another test's files.

use super::*;
use crate::agent_harness::{self, HarnessRecord};
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::install_cli::{Verb, run_with_skills};
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

#[test]
fn claude_writes_into_the_native_skill_root() {
    let scratch = Scratch::new("claude");
    let outcome = install(record("claude"), &env(&scratch.root)).expect("the assets install");
    assert_eq!(
        outcome.dir,
        scratch
            .root
            .join(".claude")
            .join("skills")
            .join(SKILL_DIR_NAME)
    );
    assert_eq!(outcome.assets, 18);
    assert_eq!(outcome.written, 18);
}

#[test]
fn antigravity_writes_into_the_native_gemini_skill_root() {
    let scratch = Scratch::new("antigravity");
    let outcome = install(record("antigravity"), &env(&scratch.root)).expect("the assets install");
    assert_eq!(
        outcome.dir,
        scratch
            .root
            .join(".gemini")
            .join("config")
            .join("skills")
            .join(SKILL_DIR_NAME)
    );
}

#[test]
fn codex_writes_into_the_portable_skill_root() {
    let scratch = Scratch::new("codex");
    let outcome = install(record("codex"), &env(&scratch.root)).expect("the assets install");
    assert_eq!(
        outcome.dir,
        scratch
            .root
            .join(".agents")
            .join("skills")
            .join(SKILL_DIR_NAME)
    );
}

#[test]
fn hermes_uses_the_portable_root_and_states_the_reason() {
    let scratch = Scratch::new("hermes");
    let outcome = install(record("hermes"), &env(&scratch.root)).expect("the assets install");
    assert_eq!(
        outcome.dir,
        scratch
            .root
            .join(".agents")
            .join("skills")
            .join(SKILL_DIR_NAME)
    );
    let text = outcome_text(record("hermes"), &outcome);
    assert!(
        text.contains("no documented native skill mechanism"),
        "{text}"
    );
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn every_asset_is_written_byte_identical_to_the_embedded_table() {
    let scratch = Scratch::new("bytes");
    let outcome = install(record("claude"), &env(&scratch.root)).expect("the assets install");
    for (name, text) in SKILL_ASSETS {
        let bytes = fs::read(outcome.dir.join(name)).expect("the asset exists");
        assert_eq!(
            bytes,
            text.as_bytes(),
            "{name} differs from the embedded text"
        );
    }
    assert!(outcome.dir.join("references/01-retrieval.md").is_file());
    assert!(outcome.dir.join("references/17-compliance.md").is_file());
}

#[test]
fn a_second_run_writes_nothing() {
    let scratch = Scratch::new("idempotent");
    let first = install(record("claude"), &env(&scratch.root)).expect("the first run writes");
    assert_eq!(first.written, 18);
    let second = install(record("claude"), &env(&scratch.root)).expect("the second run is safe");
    assert_eq!(second.written, 0);
    assert!(outcome_text(record("claude"), &second).contains("nothing written"));
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

    install(record("claude"), &env(&scratch.root)).expect("the assets install");

    assert_eq!(
        fs::read(skills.join("keep.txt")).expect("the marker remains"),
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

    let error = install(record("claude"), &env(&scratch.root))
        .expect_err("a file at the package path blocks the directory");
    let message = failure_message(record("claude"), &error);
    assert!(message.contains(SKILL_DIR_NAME), "{message}");
    assert!(message.contains("failed to create"), "{message}");
}

#[test]
fn the_outcome_line_names_the_absolute_directory_and_the_count() {
    let scratch = Scratch::new("text");
    let outcome = install(record("claude"), &env(&scratch.root)).expect("the assets install");
    let text = outcome_text(record("claude"), &outcome);
    assert!(text.contains(&outcome.dir.display().to_string()), "{text}");
    assert!(text.contains("18 skill assets"), "{text}");
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn the_skill_step_runs_before_registration() {
    let scratch = Scratch::new("order");
    let order = RefCell::new(Vec::new());
    let skill = |_: &HarnessRecord, _: &DirectWriteEnv| {
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
    let skill = |_: &HarnessRecord, _: &DirectWriteEnv| Err("skill write failed".to_owned());

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
