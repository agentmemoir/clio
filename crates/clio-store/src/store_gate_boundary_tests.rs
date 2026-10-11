// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the admission gate-boundary tripwire tests:
//! only the gated admission wrapper may create long-term items.
//!
//! ## Owns
//! - Source scans for direct `create_memory_item(`, `commit_distilled_unit(`,
//!   and `BatchWrite::CreateItem` callers outside the documented allowlists
//!
//! ## Does not own
//! - The admission gate itself (`clio-admission`)
//! - The ungated repository primitives (`clio-store`)
//!
//! ## Boundary
//!
//! New ungated write surfaces should normally require allowlist updates here.
//! Changes to gating logic should not require changes here.
//!
//! Hub distillation is an admitted second write surface: it runs the
//! admission gate through `clio_admission::gated_create` and persists
//! through `Store::commit_distilled_unit` inside that wrapper's write
//! closure. The admission gate still runs; only the write-closure target
//! differs from the create primitive.
//!
//! The repository primitives stay public so the wrapper itself can call them,
//! which means bypass is one direct call away. These tests scan every crate in
//! the workspace, so a scoped run reaches them only when the diff happens to
//! pull `clio-store` into the closure. CI therefore runs `make gate-boundary`,
//! which selects this module and nothing else, in a step that ignores the diff,
//! so a new direct caller outside the allowlists below fails CI either way.
//! Test-only.

use std::path::{Path, PathBuf};

/// Files allowed to name `create_memory_item(` directly.
fn allowed_caller(file: &str) -> bool {
    file.ends_with("_tests.rs")
        // Shared test helpers, by the naming convention: `foo_test_support.rs`
        // and `foo_fixtures.rs` are declared `#[cfg(test)]` and never built into
        // production. A suffix rule rather than one named file, so a new helper
        // is exempt the day it is written instead of failing the gate later.
        //
        // `foo_seam.rs` is deliberately NOT exempt. A seam can be compiled into
        // production — `clio`'s is, from `main.rs` — so the name alone does
        // not prove the file is test-only. It calls no write primitive today,
        // so nothing depends on that exemption.
        || file.ends_with("_test_support.rs")
        || file.ends_with("_fixtures.rs")
        || file.contains("/tests/")
        || file.ends_with("crates/clio-store/src/store.rs")
        || file.ends_with("crates/clio-store/src/sqlite.rs")
        || file.ends_with("crates/clio-store/src/sqlite_store.rs")
        || file.ends_with("crates/clio-store/src/postgres.rs")
        || file.ends_with("crates/clio-store/src/postgres_store.rs")
        || file.ends_with("crates/clio-store/src/async_store.rs")
        || file.ends_with("crates/clio-admission/src/gated.rs")
        // The MCP `store` tool persists through `clio_write::gated_store_verified`
        // (which wraps `gated_create`) and only calls the primitive inside that
        // wrapper's write closure.
        || file.ends_with("crates/clio-mcp/src/store_write.rs")
        // The library raw-ingest API runs `clio_write::parallel_ingest`, which
        // persists only inside `gated_store_verified`'s write closure after span
        // verify and admission pass (the same pattern as `store_write`).
        || file.ends_with("crates/clio-write/src/raw_ingest.rs")
        // Sync apply persists ALREADY-ADMITTED peer creates: the
        // convergence rule forbids re-running local admission θ on trusted
        // remote creates, so the ungated primitive is the correct path here
        // (schema/category/authz/erasure checks still run inside the engine).
        || file.ends_with("crates/clio-sync/src/apply.rs")
        // The MCP `import_provider` adapter admits through `clio_admission::gated_create_raw`
        // and only calls the primitive inside that wrapper's write closure.
        || file.ends_with("crates/clio-mcp/src/provider_adapter.rs")
}

/// Files allowed to name `commit_distilled_unit(` (the distilled-unit commit
/// primitive). Hub distillation is an admitted second write surface: it runs
/// `clio_admission::gated_create` and persists through this primitive inside
/// the gated write closure, so the admission gate still runs.
fn allowed_distill_committer(file: &str) -> bool {
    file.ends_with("_tests.rs")
        || file.contains("/tests/")
        // The trait declares the primitive and the store impls dispatch it.
        || file.ends_with("crates/clio-store/src/store.rs")
        || file.ends_with("crates/clio-store/src/sqlite_store.rs")
        || file.ends_with("crates/clio-store/src/postgres_store.rs")
        // Hub distillation admits through `clio_admission::gated_create` and
        // persists through this primitive inside that wrapper's write closure.
        || file.ends_with("crates/clio-write/src/hub_distill.rs")
        // Shared test helpers, on the same suffix rule `allowed_caller` uses.
        || file.ends_with("_test_support.rs")
        || file.ends_with("_fixtures.rs")
}

/// Files allowed to stage `BatchWrite::CreateItem` (the batch executor's ungated
/// create arm). The same already-admitted/first-gate rules apply.
fn allowed_batch_caller(file: &str) -> bool {
    file.ends_with("_tests.rs")
        || file.contains("/tests/")
        // Batch executors themselves implement the primitive.
        || file.ends_with("crates/clio-store/src/batch_sqlite.rs")
        || file.ends_with("crates/clio-store/src/batch_postgres.rs")
        // Compliance import restores an exported bundle verbatim: the exported
        // rows were already admitted; re-running admission on restore would
        // reject history the deployment already accepted.
        || file.ends_with("crates/clio-compliance/src/import_apply.rs")
        // The MCP batch tool stages `BatchWrite` ops only after
        // `batch_preflight` scores every op through `gated_store_verified`.
        || file.ends_with("crates/clio-mcp/src/batch_preflight.rs")
        || file.ends_with("crates/clio-mcp/src/batch_tools.rs")
        // Shared test helpers, on the same suffix rule `allowed_caller` uses.
        // This replaces a named entry for `batch_test_support.rs`, which names a
        // file that does not exist; the real fixture is `batch_test_support_tests.rs`
        // and the `_tests.rs` rule above already covered it.
        || file.ends_with("_test_support.rs")
        || file.ends_with("_fixtures.rs")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read source dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn offenders_for(root: &Path, needle: &str, allowed: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut files = Vec::new();
    rs_files(root, &mut files);
    assert!(!files.is_empty(), "expected to find crate sources");
    let mut offenders = Vec::new();
    for file in &files {
        let name = file.to_string_lossy().replace('\\', "/");
        if allowed(&name) {
            continue;
        }
        let content = std::fs::read_to_string(file).expect("read source file");
        for (index, line) in content.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(needle) {
                offenders.push(format!("{}:{}", name, index + 1));
            }
        }
    }
    offenders
}

#[test]
fn only_gated_path_creates_items() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let offenders = offenders_for(&root.join("crates"), "create_memory_item(", &allowed_caller);
    assert!(
        offenders.is_empty(),
        "direct create_memory_item callers outside the gated path: {offenders:?} \
         (route new long-term writes through clio-admission gated_create)"
    );
}

#[test]
fn only_gated_path_commits_distilled_units() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let offenders = offenders_for(
        &root.join("crates"),
        "commit_distilled_unit(",
        &allowed_distill_committer,
    );
    assert!(
        offenders.is_empty(),
        "direct commit_distilled_unit callers outside the gated path: {offenders:?} \
         (hub distillation persists through Store::commit_distilled_unit inside \
         clio-admission gated_create's write closure; route new writes through \
         an admitted surface)"
    );
}

#[test]
fn batch_create_items_only_through_documented_paths() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let offenders = offenders_for(
        &root.join("crates"),
        "BatchWrite::CreateItem",
        &allowed_batch_caller,
    );
    assert!(
        offenders.is_empty(),
        "direct BatchWrite::CreateItem users outside the documented paths: {offenders:?} \
         (score every op through clio-admission gating before apply_batch)"
    );
}

/// Files the legacy-envelope scan may exempt: only this file, which names the
/// needles it scans for.
fn allowed_legacy_scan(file: &str) -> bool {
    file.ends_with("crates/clio-store/src/store_gate_boundary_tests.rs")
}

/// The legacy `ItemEnvelope` CRUD (`put_item`/`get_item`) was removed from the
/// `Store` contract and from every fixture. Nothing may name the type or call
/// the removed methods again: opaque at-rest rows arrive only through
/// `BatchWrite::PutOpaqueItem`, and typed items go through
/// `create_memory_item`.
#[test]
fn legacy_envelope_crud_stays_removed() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    for needle in ["ItemEnvelope", ".put_item(", ".get_item("] {
        let offenders = offenders_for(&root.join("crates"), needle, &allowed_legacy_scan);
        assert!(
            offenders.is_empty(),
            "legacy envelope CRUD is back ({needle}): {offenders:?}"
        );
    }
}
