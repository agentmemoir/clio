// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the flag documentation metadata of the
//! task, failure, memory-tree, and temporal-history records.
//!
//! ## Owns
//! - The `check_metadata` result over the `TASK_HISTORY` tranche
//! - The tranche's flag metadata: the enum list the consumer accepts and the
//!   value kinds its parsers use
//!
//! ## Does not own
//! - The records' usage text, parser sets, summaries, and required bits
//!   (`cli_command_registry_task_history_tests`)
//! - The metadata check itself (`cli_command_registry`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on these eight records' flag metadata.

use crate::cli_command_registry::check_metadata;
use crate::cli_command_spec::{CommandSpec, ValueKind};

/// Every documented flag in this tranche carries a description and, when it
/// takes a value, a placeholder and non-empty kind.
#[test]
fn the_task_history_tranche_passes_the_metadata_check() {
    let specs: Vec<&CommandSpec> = super::TASK_HISTORY.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// Every documented flag carries the kind its consumer parses:
/// `history temporal --time-axis` accepts exactly the two axes
/// `cli_read_args::validate_time_axis` accepts, every counted flag carries
/// `Uint` (the kind `cli_read_history` and `cli_write_persona` parse with
/// `parse_u64`), and every remaining flag stays free text or the shared `--output` global.
#[test]
fn the_task_history_flag_kinds_match_their_consumers() {
    for spec in super::TASK_HISTORY {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("history temporal", "time-axis") => ValueKind::Enum(&["valid", "transaction"]),
                ("task history" | "failure list" | "memtree query", "limit")
                | ("failure record", "attempt-n")
                | ("memtree query", "depth") => ValueKind::Uint,
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}
