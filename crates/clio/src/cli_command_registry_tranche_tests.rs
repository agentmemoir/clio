// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the registry's tranche order: the
//! order of `TRANCHES`, the reserved surfaces' group with its compose tail,
//! and the leaf surfaces' own record tranches that close it.
//!
//! ## Owns
//! - `TRANCHES`, index by index, and its length
//! - The reserved surfaces' 17 leaves across their tranche and the compose tail
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The per-record bytes and parser sets (the per-record tranche test modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the tranche order.
//
// It is its own module because `cli_command_registry_tests` was at the size
// limit and this test is a cohesive, self-contained part of it: it reads only
// `super::` items, so it moved whole.

use super::TRANCHES;

/// The registry registers the core reads first, then the group tranches, then
/// the two write tranches, then the reserved surfaces with their compose tail,
/// then the leaf surfaces' own five record tranches: backup, sync,
/// auth/config, ranking/retention, and compliance, in that order, fourteen
/// tranches in all.
#[test]
fn the_registry_registers_the_core_reads_then_the_group_and_write_tranches() {
    assert_eq!(
        TRANCHES[0],
        super::core_reads::CORE_READS,
        "the core reads are the first tranche"
    );
    assert_eq!(
        TRANCHES[1],
        super::task_history::TASK_HISTORY,
        "the task/history tranche follows"
    );
    assert_eq!(
        TRANCHES[2],
        super::graph_records::GRAPH_RECORDS,
        "the graph/records tranche follows"
    );
    assert_eq!(
        TRANCHES[3],
        super::persona_local::PERSONA_LOCAL,
        "the persona/local tranche follows"
    );
    assert_eq!(
        TRANCHES[4],
        super::shared_workspace::SHARED_WORKSPACE,
        "the shared/workspace tranche follows"
    );
    assert_eq!(
        TRANCHES[5],
        super::core_writes::CORE_WRITES,
        "the memory-write tranche follows"
    );
    assert_eq!(
        TRANCHES[6],
        super::write_operations::WRITE_OPERATIONS,
        "the operation-write tranche follows"
    );
    assert_eq!(
        TRANCHES[7],
        super::surface::SURFACE,
        "the reserved surfaces' tranche follows"
    );
    assert_eq!(
        TRANCHES[8],
        super::COMPOSE_LEAVES,
        "the compose records close the reserved surfaces' group"
    );
    assert_eq!(
        TRANCHES[9],
        super::backup_leaves::BACKUP_LEAVES,
        "the backup tranche follows"
    );
    assert_eq!(
        TRANCHES[10],
        super::sync_leaves::SYNC_LEAVES,
        "the sync tranche follows"
    );
    assert_eq!(
        TRANCHES[11],
        super::auth_config::AUTH_CONFIG_LEAVES,
        "the auth/config tranche follows"
    );
    assert_eq!(
        &TRANCHES[12..],
        &[
            super::ranking_retention::RANKING_RETENTION_LEAVES,
            super::compliance_leaves::COMPLIANCE_LEAVES
        ][..],
        "the ranking/retention and compliance tranches close the registry"
    );
    assert_eq!(TRANCHES.len(), 14, "fourteen tranches are registered");
    // The surface tranche and its compose tail together cover all 17 reserved
    // leaves, once each: the 15 surface records and `compose up`/`down`. The
    // five record tranches that follow are the leaf surfaces' own.
    let reserved: Vec<&str> = TRANCHES[7..9]
        .iter()
        .flat_map(|tranche| tranche.iter().map(|spec| spec.name))
        .collect();
    assert_eq!(
        reserved.len(),
        17,
        "the reserved surfaces' group covers all 17 leaves"
    );
    let mut unique = reserved.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 17, "no reserved leaf is registered twice");
    let compose: Vec<&str> = super::COMPOSE_LEAVES.iter().map(|spec| spec.name).collect();
    assert_eq!(compose, ["compose up", "compose down"]);
}
