// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the scratch-directory and package helpers the skill remover's tests share: one temporary home per test, the two package paths under it, and a marker reader and writer for building a modified package.
//!
//! ## Owns
//! - The `Scratch` guard, `env`, `native_package`, `portable_package`, `read_marker`, and `write_marker`
//!
//! ## Does not own
//! - The remover under test (`skill_uninstall`)
//! - Any assertion (`skill_uninstall_tests`)
//!
//! ## Boundary
//!
//! Test-only: every path lives under one `Scratch` dir in the system temp dir, so no test touches the real home directory.
//!
//! Keep this module focused on the skill remover's shared test fixtures.

use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use crate::skill_marker::{SKILL_MARKER_FILE, SkillMarker};
use crate::skill_uninstall::{RefusalReason, UninstallError};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// One test's own directory, removed when the guard drops.
pub(crate) struct Scratch {
    pub(crate) root: PathBuf,
}

impl Scratch {
    pub(crate) fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-skill-uninstall-{label}-{}-{nanos}-{seq}",
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
pub(crate) fn env(home: &Path) -> DirectWriteEnv {
    DirectWriteEnv::with(OsLayout::Linux, home.to_path_buf(), None, None, None)
}

/// The package directory a claude install writes under its native root.
pub(crate) fn native_package(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("skills")
        .join(crate::skill_install::SKILL_DIR_NAME)
}

/// The package directory every harness shares under the portable root.
pub(crate) fn portable_package(home: &Path) -> PathBuf {
    home.join(".agents")
        .join("skills")
        .join(crate::skill_install::SKILL_DIR_NAME)
}

/// The marker the package directory holds, parsed.
pub(crate) fn read_marker(dir: &Path) -> SkillMarker {
    let bytes = fs::read(dir.join(SKILL_MARKER_FILE)).expect("the marker was written");
    SkillMarker::from_bytes(&bytes).expect("the installed marker parses")
}

/// Replace the marker the package directory holds.
pub(crate) fn write_marker(dir: &Path, marker: &SkillMarker) {
    let bytes = marker.to_bytes().expect("the marker serializes");
    fs::write(dir.join(SKILL_MARKER_FILE), bytes).expect("the marker is written");
}

/// The first recorded relative path, so a test edits one file the marker owns.
pub(crate) fn first_asset(dir: &Path) -> String {
    read_marker(dir)
        .files
        .keys()
        .next()
        .cloned()
        .expect("the marker records at least one file")
}

/// The directory and reasons of a refusal, for a test that expects one.
pub(crate) fn refused(error: UninstallError) -> (PathBuf, Vec<RefusalReason>) {
    match error {
        UninstallError::Refused { dir, reasons } => (dir, reasons),
        other @ UninstallError::Io { .. } => panic!("expected a refusal, got {other:?}"),
    }
}
