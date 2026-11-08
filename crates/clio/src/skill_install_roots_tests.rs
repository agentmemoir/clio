// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests which skill roots the installer writes for each harness:
//! a native root plus the portable root where the harness documents one, the
//! portable root alone otherwise, the router-last write order, idempotence per
//! root, and the refusal and forced rewrite of a root whose recorded asset
//! changed.
//!
//! ## Owns
//! - The per-harness root mapping assertions over `install`
//! - The router-last assertion and the per-root rewrite assertion
//! - The multi-root outcome text
//!
//! ## Does not own
//! - The installer implementation under test (`skill_install`)
//! - The failure text and the portable-only flow (`skill_install_tests`)
//! - The marker, conflict, and `--force` matrix (`skill_install_marker_tests`)
//! - The harness registry (`agent_harness`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the
//! system temporary directory, so no test touches the real home directory or
//! another test's files.

use super::*;
use crate::agent_harness::{self, HarnessRecord};
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use std::fs;
use std::path::{Path, PathBuf};
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
            "clio-skill-roots-{label}-{}-{nanos}-{seq}",
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

/// The installed package directory under a home-relative root built from `parts`.
fn package(home: &Path, parts: &[&str]) -> PathBuf {
    let root = parts
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part));
    root.join(SKILL_DIR_NAME)
}

/// Every embedded asset is present and byte-identical in `dir`.
fn assert_all_assets(dir: &Path) {
    for (name, text) in SKILL_ASSETS {
        let bytes = fs::read(dir.join(name)).expect("the asset exists");
        assert_eq!(
            bytes,
            text.as_bytes(),
            "{name} differs from the embedded text"
        );
    }
}

/// The roots a first install wrote, or a panic naming the outcome it got instead.
fn written(outcome: &SkillOutcome) -> &[SkillRootOutcome] {
    match outcome {
        SkillOutcome::Written { roots } => roots,
        other => panic!("a first install writes every root: {other:?}"),
    }
}

#[test]
fn claude_writes_the_native_and_portable_roots() {
    let scratch = Scratch::new("claude-both");
    let outcome =
        install(record("claude"), &env(&scratch.root), false).expect("the assets install");
    let native = package(&scratch.root, &[".claude", "skills"]);
    let portable = package(&scratch.root, &[".agents", "skills"]);
    let roots = written(&outcome);
    assert_eq!(roots.len(), 2, "claude reads two roots");
    assert_eq!(roots[0].dir, native);
    assert_eq!(roots[1].dir, portable);
    assert_all_assets(&native);
    assert_all_assets(&portable);
    assert!(
        outcome_text(record("claude"), &outcome).contains("18 skill assets"),
        "the embedded payload still reads as 18 assets: {}",
        outcome_text(record("claude"), &outcome)
    );
}

#[test]
fn the_claude_code_alias_writes_the_native_and_portable_roots() {
    let scratch = Scratch::new("claude-code-alias");
    let outcome =
        install(record("claude-code"), &env(&scratch.root), false).expect("the assets install");
    let native = package(&scratch.root, &[".claude", "skills"]);
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert_eq!(written(&outcome).len(), 2, "the alias resolves to claude");
    assert_eq!(written(&outcome)[0].dir, native);
    assert_eq!(written(&outcome)[1].dir, portable);
}

#[test]
fn antigravity_writes_the_gemini_and_portable_roots() {
    let scratch = Scratch::new("antigravity-both");
    let outcome =
        install(record("antigravity"), &env(&scratch.root), false).expect("the assets install");
    let native = package(&scratch.root, &[".gemini", "config", "skills"]);
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert_eq!(written(&outcome).len(), 2, "antigravity reads two roots");
    assert_eq!(written(&outcome)[0].dir, native);
    assert_eq!(written(&outcome)[1].dir, portable);
    assert_all_assets(&native);
    assert_all_assets(&portable);
}

#[test]
fn codex_writes_only_the_portable_root() {
    let scratch = Scratch::new("codex-portable");
    let outcome = install(record("codex"), &env(&scratch.root), false).expect("the assets install");
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert_eq!(
        written(&outcome).len(),
        1,
        "codex reads the portable root only"
    );
    assert_eq!(written(&outcome)[0].dir, portable);
    assert!(
        !scratch.root.join(".codex").exists(),
        "no codex-native root is written"
    );
    assert_all_assets(&portable);
}

#[test]
fn github_copilot_writes_only_the_portable_root() {
    let scratch = Scratch::new("copilot-portable");
    let outcome =
        install(record("github copilot"), &env(&scratch.root), false).expect("the assets install");
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert_eq!(
        written(&outcome).len(),
        1,
        "github copilot reads the portable root only"
    );
    assert_eq!(written(&outcome)[0].dir, portable);
    assert!(
        !scratch.root.join(".copilot").exists(),
        "no copilot-native root is written"
    );
    assert!(
        !scratch.root.join(".claude").exists(),
        "no other native root is written"
    );
}

#[test]
fn hermes_uses_the_portable_root_and_states_the_reason() {
    let scratch = Scratch::new("hermes-portable");
    let outcome =
        install(record("hermes"), &env(&scratch.root), false).expect("the assets install");
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert_eq!(
        written(&outcome).len(),
        1,
        "hermes reads the portable root only"
    );
    assert_eq!(written(&outcome)[0].dir, portable);
    let text = outcome_text(record("hermes"), &outcome);
    assert!(
        text.contains("no documented native skill mechanism"),
        "{text}"
    );
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn a_reference_failure_leaves_no_router_in_either_root() {
    let scratch = Scratch::new("no-router");
    let native = package(&scratch.root, &[".claude", "skills"]);
    fs::create_dir_all(&native).expect("the native package directory can be created");
    fs::write(native.join("references"), b"not a directory")
        .expect("the blocking file can be written");

    let error = install(record("claude"), &env(&scratch.root), true)
        .expect_err("a file at the reference path blocks the forced write");
    assert!(
        failure_message(record("claude"), &error).contains("failed to create"),
        "{error:?}"
    );
    assert!(
        !native.join("SKILL.md").exists(),
        "the native router must not exist after a reference failure"
    );
    let portable = package(&scratch.root, &[".agents", "skills"]);
    assert!(
        !portable.join("SKILL.md").exists(),
        "the portable router must not exist after a reference failure"
    );
}

#[test]
fn the_outcome_text_names_every_written_root() {
    let scratch = Scratch::new("roots-text");
    let outcome =
        install(record("claude"), &env(&scratch.root), false).expect("the assets install");
    let text = outcome_text(record("claude"), &outcome);
    let SkillOutcome::Written { roots } = &outcome else {
        panic!("the first install writes both roots: {outcome:?}");
    };
    for root in roots {
        assert!(
            text.contains(&root.dir.display().to_string()),
            "the text must name {}: {text}",
            root.dir.display()
        );
    }
    assert!(text.contains("18 skill assets"), "{text}");
    assert!(!text.contains("loaded"), "{text}");
}

#[test]
fn a_locally_edited_root_is_refused_and_a_forced_run_rewrites_only_that_file() {
    let scratch = Scratch::new("rewrite");
    install(record("codex"), &env(&scratch.root), false).expect("the first run writes");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    let target = dir.join("references").join("01-retrieval.md");
    fs::write(&target, b"stale").expect("the asset can be changed");

    let refused =
        install(record("codex"), &env(&scratch.root), false).expect("a refusal is planned");
    let text = portable_outcome_text(&refused);
    assert!(
        matches!(refused, SkillOutcome::Refused { .. }),
        "a local edit is refused by default: {refused:?}"
    );
    assert!(
        text.contains("references/01-retrieval.md") && text.contains("local edit"),
        "{text}"
    );
    assert_eq!(
        fs::read(&target).expect("the edited asset is readable"),
        b"stale",
        "a refusal must not restore the file"
    );

    let forced =
        install(record("codex"), &env(&scratch.root), true).expect("the forced run rewrites");
    let SkillOutcome::Written { roots } = &forced else {
        panic!("a forced run writes: {forced:?}");
    };
    assert_eq!(
        roots[0].written,
        ["references/01-retrieval.md"],
        "only the changed file is written; the marker already matches"
    );
    let expected = SKILL_ASSETS
        .iter()
        .find(|(name, _)| *name == "references/01-retrieval.md")
        .expect("the reference is embedded")
        .1;
    assert_eq!(
        fs::read(&target).expect("the asset is readable"),
        expected.as_bytes(),
        "the forced run restores the embedded bytes"
    );
}

#[test]
fn a_second_run_writes_nothing_in_either_root() {
    let scratch = Scratch::new("idempotent-roots");
    install(record("claude"), &env(&scratch.root), false).expect("the first run writes");
    let second =
        install(record("claude"), &env(&scratch.root), false).expect("the second run is safe");
    let SkillOutcome::Unchanged { roots } = &second else {
        panic!("an identical second run is a no-op: {second:?}");
    };
    assert_eq!(roots.len(), 2);
    let text = outcome_text(record("claude"), &second);
    assert_eq!(
        text.matches("unchanged").count(),
        2,
        "every root says it is unchanged: {text}"
    );
    assert_eq!(
        text.matches("nothing written").count(),
        2,
        "every root says nothing was written: {text}"
    );
}
