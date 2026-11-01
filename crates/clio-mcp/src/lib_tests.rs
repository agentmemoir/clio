// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the coverage of the crate-root tool bindings:
//! the exact write and read name sets this build advertises, their
//! disjointness, and the union the combined list reports.
//!
//! ## Owns
//! - Every test of `bound_write_tools`, `bound_read_tools`, and `bound_tools`
//!
//! ## Does not own
//! - The name lists themselves (see `clio_types::mcp_tools`)
//! - Per-tool schemas (see `schema_portability_defs_tests` and its siblings)
//! - Dispatch of a bound name (see the per-surface tool tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Adding or removing a bound tool should normally update the expected sets
//! here. Tool behaviour should not.
//!
//! Every assertion compares whole name sets, so iteration order of the source
//! constants never decides the result.

use super::{bound_read_tools, bound_tools, bound_write_tools};
use std::collections::BTreeSet;

/// The write/mutate tools this build binds, in the order it publishes them.
const EXPECTED_WRITE_TOOLS: &[&str] = &[
    "admit_preview",
    "store",
    "update",
    "invalidate",
    "discard",
    "correct",
    "consolidate",
    "persona_put_stable",
    "persona_observe_preference",
    "task_upsert",
    "failure_record",
    "triple_add",
    "triple_end",
    "belief_observe",
    "graph_link",
    "erase_request",
    "batch",
    "scratchpad_write",
    "scratchpad_read",
    "scratchpad_clear",
    "canonical_put",
    "shared_store",
    "shared_discard",
    "validate",
    "hygiene_clean",
    "export",
    "import",
    "import_provider",
    "review_export",
    "repair",
    "reindex",
    "sync_push",
    "sync_pull",
    "sync_serve",
    "sync_ack_skip",
    "retention_profile_set",
    "config_set",
    "config_profile_apply",
    "ranking_env_set",
    "subject_request_start",
    "subject_request_status",
    "subject_request_retry",
    "subject_request_copy",
    "subject_request_exception",
    "subject_request_events",
    "subject_request_path_complete",
    "subject_request_close",
    "subject_request_peer_status",
    "subject_request_peer_retry",
    "subject_request_peer_disposition",
];

/// The read/retrieve/compose tools this build binds, in the order it publishes
/// them.
const EXPECTED_READ_TOOLS: &[&str] = &[
    "intent_gate",
    "retrieve",
    "compose_context",
    "associations",
    "get",
    "get_snapshot",
    "get_gist",
    "maintenance_status",
    "persona_get",
    "task_get",
    "task_history",
    "failures_for_task",
    "memtree_query",
    "memtree_get",
    "temporal_history",
    "triple_query",
    "belief_history",
    "graph_query",
    "inspect",
    "audit_trail",
    "canonical_get",
    "shared_retrieve",
    "stats",
    "hygiene_audit",
    "hygiene_log_list",
    "diagnose",
    "verify",
    "doctor",
    "sync_status",
    "sync_key_status",
    "admit_preview_batch",
    "retention_profile_get",
    "summarize",
    "config_get",
    "config_profiles",
    "ranking_env_get",
];

#[test]
fn the_bound_write_tools_are_the_expected_set() {
    let names = bound_write_tools();

    assert_eq!(names, EXPECTED_WRITE_TOOLS);
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "the write set repeats a name: {names:?}"
    );
}

#[test]
fn the_bound_read_tools_are_the_expected_set() {
    let names = bound_read_tools();

    assert_eq!(names, EXPECTED_READ_TOOLS);
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "the read set repeats a name: {names:?}"
    );
}

/// A name bound on both surfaces would advertise one tool with two contracts,
/// and a dispatcher that tried the write chain first would answer a read call
/// with a mutation refusal.
#[test]
fn the_read_and_write_sets_are_disjoint() {
    let write: BTreeSet<&str> = bound_write_tools().iter().copied().collect();
    let read: BTreeSet<&str> = bound_read_tools().iter().copied().collect();
    let overlap: Vec<&str> = write.intersection(&read).copied().collect();

    assert!(
        overlap.is_empty(),
        "tools bound as both read and write: {overlap:?}"
    );
}

#[test]
fn the_bound_tool_list_is_the_union_of_the_two_sets() {
    let mut expected: BTreeSet<&str> = bound_write_tools().iter().copied().collect();
    expected.extend(bound_read_tools().iter().copied());

    let all = bound_tools();
    let reported: BTreeSet<&str> = all.iter().copied().collect();

    assert_eq!(reported, expected);
    assert_eq!(
        reported.len(),
        all.len(),
        "bound_tools repeats a name: {all:?}"
    );
    for name in bound_write_tools().iter().chain(bound_read_tools()) {
        assert!(
            reported.contains(name),
            "`{name}` is bound but missing from the combined list"
        );
    }
}
