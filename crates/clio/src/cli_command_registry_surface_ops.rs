// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the five `clio ops`
//! subcommands: `diagnose`, `verify`, `doctor`, `repair`, and `reindex`.
//!
//! ## Owns
//! - The five `CommandSpec` records and their usage examples
//! - The database trio every ops subcommand prints, and the mutating
//!   subcommands' extra flags: `--dry-run`, `--confirm`, the repair plan and
//!   action selectors, and the reindex target legs
//!
//! ## Does not own
//! - The `SURFACE` tranche and the shared flag facts its children read (see
//!   `cli_command_registry_surface`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The ops dispatch, the doctor plan composition, or the repair allowlist
//!   (see `ops_cli` and `clio_ops`)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags, usage line, summary text, or
//! examples should normally require changes here. Changes to what an ops
//! subcommand checks or repairs should not.
//!
//! Keep this module focused on the five `ops` records.
//

use super::{DB_BACKEND_BANK, DB_BANK_UNDOCUMENTED_GLOBALS};
use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_BANK, GLOBAL_DB};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// `ops doctor`'s dry-run pair: `--dry-run false` records the plan it composed.
const DRY_RUN_FALSE: Flag = Flag::text(
    "dry-run",
    "false",
    "`ops doctor` records the plan instead of only reporting it.",
);
/// `ops reindex`'s report-only switch: it applies nothing and is not `ok`.
const REINDEX_DRY_RUN: Flag = Flag::toggle(
    "dry-run",
    "Report-only: applies nothing and is not reported as ok (`--confirm` is the live switch).",
);
const CONFIRM: Flag = Flag::toggle(
    "confirm",
    "Signals the live path: `ops repair` applies only with it, and `ops reindex` is report-only \
     without it.",
);
const REPAIR_PLAN: Flag = Flag::text("plan", "PLAN_ID", "Plan id `ops_cli`'s repair resolves.");
const REPAIR_ACTIONS: Flag = Flag::text("actions", "a,b", "Comma list `ops_cli`'s repair applies.");
const REINDEX_TARGET: Flag = Flag::choice(
    "target",
    "dense|lexical|all",
    "Index legs `ops_cli`'s reindex rebuilds.",
    &["dense", "lexical", "all"],
);

/// `clio ops diagnose`: the read-only install and database health snapshot.
pub(crate) const OPS_DIAGNOSE: CommandSpec = CommandSpec {
    name: "ops diagnose",
    aliases: &[],
    summary: "Read-only install/database health snapshot",
    notes: &[],
    positionals: &[],
    documented: DB_BACKEND_BANK,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio ops diagnose",
            comment: Some("Snapshot install and database health."),
        },
        Example {
            command: "clio ops diagnose --db ./memory.db",
            comment: Some("Diagnose one SQLite store."),
        },
        Example {
            command: "clio ops diagnose --backend sqlite --db ./memory.db",
            comment: Some("Name the backend beside the store."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio ops verify`: the referential-integrity and index-coverage checks.
pub(crate) const OPS_VERIFY: CommandSpec = CommandSpec {
    name: "ops verify",
    aliases: &[],
    summary: "Referential-integrity and index-coverage checks",
    notes: &[],
    positionals: &[],
    documented: DB_BACKEND_BANK,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio ops verify",
            comment: Some("Run the integrity and coverage checks."),
        },
        Example {
            command: "clio ops verify --db ./memory.db",
            comment: Some("Verify one SQLite store."),
        },
        Example {
            command: "clio ops verify --bank default",
            comment: Some("Limit the checks to one bank."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio ops doctor`: compose a repair plan without touching the store.
pub(crate) const OPS_DOCTOR: CommandSpec = CommandSpec {
    name: "ops doctor",
    aliases: &[],
    summary: "Compose a repair plan (read-only)",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK, DRY_RUN_FALSE],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio ops doctor",
            comment: Some("Compose a repair plan without touching the store."),
        },
        Example {
            command: "clio ops doctor --db ./memory.db",
            comment: Some("Draft the plan for one SQLite store."),
        },
        Example {
            command: "clio ops doctor --bank default",
            comment: Some("Draft the plan for one bank."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio ops repair`: apply allowlisted repairs, live only with `--confirm`.
pub(crate) const OPS_REPAIR: CommandSpec = CommandSpec {
    name: "ops repair",
    aliases: &[],
    summary: "Apply allowlisted repairs (live run requires --confirm)",
    notes: &[],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        GLOBAL_BANK,
        REPAIR_PLAN,
        REPAIR_ACTIONS,
        CONFIRM,
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio ops repair --actions rebuild_lexical_index --confirm --bank default",
            comment: Some("Apply one allowlisted repair."),
        },
        Example {
            command: "clio ops repair --actions drop_orphan_assoc_edge --confirm --bank default",
            comment: Some("Drop one orphaned association edge."),
        },
        Example {
            command: "clio ops repair --actions drop_orphan_assoc_edge,rebuild_lexical_index --confirm --bank default --db ./memory.db",
            comment: Some("Apply two repairs to one store."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio ops reindex`: rebuild derived indexes, live only with `--confirm`.
pub(crate) const OPS_REINDEX: CommandSpec = CommandSpec {
    name: "ops reindex",
    aliases: &[],
    summary: "Rebuild derived indexes (live run requires --confirm); `--dry-run` is report-only and applies nothing, so a report-only run is not reported as ok. A lexical rebuild replaces legacy plaintext index rows with keyed term digests. Ops exit codes: 0 healthy or completed, 1 domain/runtime failure or findings, 2 usage or repair refusal, 3 reindex failed or ran report-only",
    notes: &[],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        GLOBAL_BANK,
        REINDEX_TARGET,
        REINDEX_DRY_RUN,
        CONFIRM,
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio ops reindex --target lexical --confirm",
            comment: Some("Rebuild the lexical index."),
        },
        Example {
            command: "clio ops reindex --target all",
            comment: Some("Report the cost of rebuilding both legs; applies nothing."),
        },
        Example {
            command: "clio ops reindex --target lexical --confirm --db ./memory.db",
            comment: Some("Rebuild one store's lexical index."),
        },
    ],
    tool: None,
    section: Section::Write,
};

#[cfg(test)]
#[path = "cli_command_registry_surface_ops_tests.rs"]
mod tests;
