// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync device id resolver: the resolution order, the
//! minted value's shape, the persistence that lets the per-device ordering mark
//! outlive a restart, and the unchanged per-process answer for an ephemeral
//! store.
//!
//! ## Owns
//! - The mint-then-read-back round trip over one store
//! - The override precedence and the ephemeral answer
//! - Survival across a close and reopen of the same store file
//!
//! ## Does not own
//! - The settings accessors themselves (`clio-store`)
//! - The wiring into runtime state (`runtime_open`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_store::{LocalDevKms, SqliteStore};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinct suffix per test file-store, so two tests never share a database.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn memory_store() -> Arc<dyn OpsStore> {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("in-memory store");
    Arc::new(store)
}

fn file_store(path: &std::path::Path) -> Arc<dyn OpsStore> {
    let store = SqliteStore::open(path, Arc::new(LocalDevKms::new())).expect("file store");
    Arc::new(store)
}

/// One temporary database path this test owns, removed on drop.
struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("clio-device-id-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The shape every minted id has: the `device-` prefix, then base64url text.
fn assert_minted_shape(id: &str) {
    let body = id.strip_prefix(PREFIX).expect("a device- prefix");
    assert!(
        !body.is_empty(),
        "the minted body must not be empty: {id:?}"
    );
    assert!(
        body.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "the minted body must be url-safe: {id:?}"
    );
}

#[test]
fn first_resolve_mints_one_value_and_stores_it() {
    let store = memory_store();
    let id = resolve(&*store, false, None).expect("a first resolve mints");
    assert_minted_shape(&id);
    assert_eq!(
        store.setting_get(SETTING_KEY).expect("read back"),
        Some(id),
        "the minted id is written before it is returned"
    );
}

#[test]
fn second_resolve_returns_the_same_value() {
    let store = memory_store();
    let first = resolve(&*store, false, None).expect("first resolve");
    let second = resolve(&*store, false, None).expect("second resolve");
    assert_eq!(first, second, "one store serves one device id");
}

#[test]
fn explicit_override_wins_over_a_stored_value() {
    let store = memory_store();
    let stored = resolve(&*store, false, None).expect("mint a stored value");
    let chosen = resolve(&*store, false, Some("device-override")).expect("override");
    assert_eq!(chosen, "device-override", "the override wins");
    assert_eq!(
        store.setting_get(SETTING_KEY).expect("read back"),
        Some(stored),
        "an override never rewrites the stored value"
    );
}

#[test]
fn an_empty_override_falls_through_to_the_stored_value() {
    let store = memory_store();
    let stored = resolve(&*store, false, None).expect("mint a stored value");
    let chosen = resolve(&*store, false, Some("   ")).expect("blank override");
    assert_eq!(chosen, stored, "a blank override is not a choice");
}

#[test]
fn ephemeral_store_answers_the_process_form_and_stores_nothing() {
    let store = memory_store();
    let id = resolve(&*store, true, None).expect("ephemeral resolve");
    assert_eq!(id, format!("{PREFIX}{}", std::process::id()));
    assert_eq!(
        store.setting_get(SETTING_KEY).expect("read back"),
        None,
        "an ephemeral store keeps no row"
    );
}

#[test]
fn the_value_survives_reopening_the_same_store_file() {
    let db = TempDb::new();
    let first = {
        let store = file_store(&db.path);
        resolve(&*store, false, None).expect("first open mints")
    };
    let second = {
        let store = file_store(&db.path);
        resolve(&*store, false, None).expect("second open reads the row")
    };
    assert_eq!(first, second, "the id outlives the process-scoped open");
}
