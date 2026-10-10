// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hermetic retention-store fixture shared by
//! the `clio-mcp` test modules.
//!
//! ## Owns
//! - `RetentionStoreFixture`, a test state whose durable retention store is a
//!   unique temporary file removed when the fixture drops
//!
//! ## Does not own
//! - Any test
//! - The retention store itself (`clio-config` `retention_store`)
//! - State construction policy (`crate::runtime` `McpOpenOptions`)
//!
//! ## Boundary
//!
//! Changes to how a test state isolates its retention store should normally
//! require changes here. Changes to a single test's assertions should not.
//!
//! Keep this module focused on the shared retention-store test fixture.

use crate::runtime::{McpOpenOptions, McpState};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A test [`McpState`] whose durable retention store is a unique temporary file.
///
/// A state opened from the process environment names the store the host
/// configured, so a profile written by a test would reach that store, and a
/// later test would read it back as if it were its own setup. The fixture
/// redirects the path to a file it owns and removes the file when it drops, so a
/// test depends only on the code under test. The fixture stands in for the state
/// through [`Deref`](std::ops::Deref), so a test reads as one that opens a state
/// directly.
pub(crate) struct RetentionStoreFixture {
    state: Arc<McpState>,
    path: PathBuf,
}

impl RetentionStoreFixture {
    /// Adopt a state opened with [`McpState::open`].
    pub(crate) fn open(options: &McpOpenOptions) -> Self {
        Self::adopt(McpState::open(options).expect("state"))
    }

    /// Adopt a state opened with [`McpState::open_with_effective`].
    pub(crate) fn open_with_effective(options: &McpOpenOptions, effective: &Value) -> Self {
        Self::adopt(McpState::open_with_effective(options, effective).expect("state"))
    }

    /// The owned state, for a caller that needs an `Arc` by value.
    #[must_use]
    pub(crate) fn state_arc(&self) -> Arc<McpState> {
        Arc::clone(&self.state)
    }

    fn adopt(mut state: Arc<McpState>) -> Self {
        let path = temp_store_path();
        // A stale file from a crashed run must not seed this fixture's store.
        let _ = std::fs::remove_file(&path);
        Arc::get_mut(&mut state)
            .expect("the fixture state is uniquely owned")
            .set_retention_path(&path);
        Self { state, path }
    }
}

impl Drop for RetentionStoreFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl std::ops::Deref for RetentionStoreFixture {
    type Target = McpState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// A per-call temporary path, unique within and across test processes.
fn temp_store_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let call = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "clio-mcp-retention-{}-{call}.json",
        std::process::id()
    ))
}
