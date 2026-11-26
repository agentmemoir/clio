// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the scaffolding the CLI state test modules
//! share: a self-cleaning directory per test, a boot over an explicit state
//! path, the reads that report what a boot restored, and the writes that put a
//! state file on disk.
//!
//! ## Owns
//! - `Scratch`, a directory unique per call and removed when the guard drops
//! - `boot`, a runtime over an explicit path with no host environment and no
//!   deployment overlay
//! - `theta_patch`, `set_and_flush`, and `write_overlay_state`, which produce
//!   the state a test then reads back
//! - `ranking_of`, `active_of`, `log_level_of`, `active_profile_of`, and
//!   `value_at`, the observable state a boot reports
//!
//! ## Does not own
//! - Any test (see `tests` and `restore_tests`)
//! - The implementation under test (see `cli_state`)
//! - The ranking schema itself (see `crate::ranking`)
//!
//! ## Boundary
//!
//! Nothing here reads the process environment or a fixed shared path: every
//! caller passes its own values, so a test can neither write to the machine it
//! runs on nor read another test's directory.
//!
//! Keep this module focused on test scaffolding, with no tests of its own.

use super::{CLI_STATE_FILE_NAME, Runtime};
use crate::error::ToolResult;
use crate::ranking::{RankingEnv, RankingEnvPatch};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-test scratch directory, removed on drop and unique by process and order.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    /// A fresh scratch directory for this test.
    pub(crate) fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("clio-cli-state-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    /// The scratch directory itself, for a path that is not the state file.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The state file inside it, under its production name.
    pub(crate) fn state_file(&self) -> PathBuf {
        self.0.join(CLI_STATE_FILE_NAME)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A patch that changes only the admission threshold.
pub(crate) fn theta_patch(theta: f64) -> RankingEnvPatch {
    RankingEnvPatch {
        theta_admit: Some(theta),
        ..RankingEnvPatch::default()
    }
}

/// Boot a runtime over `path` with no host environment and no deployment overlay.
pub(crate) fn boot(path: PathBuf) -> Runtime {
    Runtime::with_env_and_cli_state_file(&|_key: &str| None, path)
}

/// The ranking environment a runtime reports.
pub(crate) fn ranking_of(rt: &Runtime) -> RankingEnv {
    let ToolResult::Ok { data, .. } = rt.ranking_env_get() else {
        panic!("ranking_env_get must succeed");
    };
    data
}

/// Set `theta_admit` on a runtime, both in memory and on disk.
pub(crate) fn set_and_flush(rt: &mut Runtime, theta: f64) {
    let ToolResult::Ok { data, .. } = rt.ranking_env_set(&theta_patch(theta), false) else {
        panic!("ranking_env_set must succeed");
    };
    assert!(data.applied, "a non-dry-run set applies in memory");
    let flushed = rt.flush_cli_state().expect("flush");
    assert!(flushed.written(), "flush must write");
    assert!(
        flushed.dropped().is_empty(),
        "a ranking flush drops nothing"
    );
}

/// The profile an `effective_raw` read reports as active.
pub(crate) fn active_of(rt: &Runtime) -> Option<String> {
    rt.effective_raw()
        .pointer("/profile/active")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `log.level` a runtime's effective config carries.
pub(crate) fn log_level_of(rt: &Runtime) -> Value {
    rt.effective_raw()
        .pointer("/log/level")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The active profile `config_profiles` reports.
pub(crate) fn active_profile_of(rt: &Runtime) -> Option<String> {
    let ToolResult::Ok { data, .. } = rt.config_profiles() else {
        panic!("config_profiles must succeed");
    };
    data.active
}

/// Write a state file carrying `overlay`, under the profile that overlay
/// belongs to, so the restore path actually applies it.
pub(crate) fn write_overlay_state(path: &Path, overlay: &Value) {
    let saved = json!({
        "active_profile": "coding_local",
        "profile_overlay": overlay,
    });
    fs::write(
        path,
        serde_json::to_string(&saved).expect("serialize state"),
    )
    .expect("write state");
}

/// The effective value at `pointer`, for comparing a loaded file with a boot
/// that had no file at all.
pub(crate) fn value_at(rt: &Runtime, pointer: &str) -> Value {
    rt.effective_raw()
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}
