// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the scratch-directory and snapshot harness the skill
//! installer's provenance tests share: one temporary home per test, the package
//! paths under it, a byte-and-mtime snapshot, and the outcome accessors.
//!
//! ## Owns
//! - The `Scratch` guard, `env`, `record`, `args`, `package`, `snapshot`,
//!   `modified`, `read_marker`, `first_install_order`, `written`, `conflicts`
//!
//! ## Does not own
//! - The installer under test (`skill_install`)
//! - Any assertion (`skill_install_marker_tests`)
//!
//! ## Boundary
//!
//! Test-only: every path lives under one `Scratch` dir in the system temp dir,
//! so no test touches the real home directory.

use super::*;
use crate::agent_harness::{self, HarnessRecord};
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::skill_marker::SkillMarker;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// One test's own directory, removed when the guard drops.
pub(super) struct Scratch {
    pub(super) root: PathBuf,
}

impl Scratch {
    pub(super) fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-skill-marker-{label}-{}-{nanos}-{seq}",
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

/// A Linux-layout environment rooted at the scratch directory.
pub(super) fn env(home: &Path) -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, home.to_path_buf(), None, None, None)
}

pub(super) fn record(name: &str) -> &'static HarnessRecord {
    agent_harness::resolve(name).expect("the harness is supported")
}

pub(super) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_owned()).collect()
}

/// The installed package directory under a home-relative root built from `parts`.
pub(super) fn package(home: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
        .join(SKILL_DIR_NAME)
}

/// Every file under `dir`, keyed by its path relative to `dir`, with its bytes
/// and its exact modification time. Two equal snapshots prove that no byte and
/// no timestamp changed.
pub(super) fn snapshot(dir: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    let mut found = BTreeMap::new();
    collect(dir, dir, &mut found);
    found
}

fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, (Vec<u8>, SystemTime)>) {
    for entry in fs::read_dir(dir).expect("the package directory is readable") {
        let path = entry.expect("the directory entry is readable").path();
        if path.is_dir() {
            collect(root, &path, found);
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        found.insert(
            relative.to_string_lossy().replace('\\', "/"),
            (
                fs::read(&path).expect("the file is readable"),
                modified(&path),
            ),
        );
    }
}

/// One file's exact modification time.
pub(super) fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("the file's metadata is readable")
        .modified()
        .expect("the file records a modification time")
}

/// The marker the package directory holds, parsed.
pub(super) fn read_marker(dir: &Path) -> SkillMarker {
    let bytes = fs::read(dir.join(SKILL_MARKER_FILE)).expect("the marker was written");
    SkillMarker::from_bytes(&bytes).expect("the installed marker parses")
}

/// The paths a first install writes, in write order: every reference, then the
/// router, then the marker last.
pub(super) fn first_install_order() -> Vec<String> {
    let mut names: Vec<String> = references()
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    names.push(SKILL_ASSETS[0].0.to_owned());
    names.push(SKILL_MARKER_FILE.to_owned());
    names
}

/// The written roots of a written outcome.
pub(super) fn written(outcome: &SkillOutcome) -> &[SkillRootOutcome] {
    match outcome {
        SkillOutcome::Written { roots } => roots,
        other => panic!("expected a written outcome, got {other:?}"),
    }
}

/// The conflicts of a refused outcome.
pub(super) fn conflicts(outcome: &SkillOutcome) -> &[RootConflict] {
    match outcome {
        SkillOutcome::Refused { conflicts } => conflicts,
        other => panic!("expected a refused outcome, got {other:?}"),
    }
}
