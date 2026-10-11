// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` binary process entrypoint and CLI dispatch.
//!
//! ## Owns
//! - Argument parsing and dispatch for scaffold commands (`version`, `health`, `help`)
//! - Dispatch into the `mcp`, `ops`, `retention`, `compose`, read, and write
//!   command surfaces
//! - Process exit codes for the top-level commands
//!
//! ## Does not own
//! - The `clio ops` command semantics and exit-code contract (see `ops_cli`)
//! - Library runtime, config tools, or ranking logic
//! - Long-running servers or MCP bindings
//!
//! ## Boundary
//!
//! Changes to CLI command surface and process lifecycle should normally require changes here.
//! Changes to in-process tool semantics should not require changes here.
//!
//! Keep this module focused on the `clio` CLI process entrypoint.

mod agent_harness;
mod agent_harness_delegate;
mod agent_harness_delegate_read;
mod agent_harness_targets;
mod agent_harness_write;
mod agent_harness_write_install;
mod auth_cli;
mod backup_cli;
mod backup_create_cli;
mod backup_create_postgres;
mod backup_create_report;
mod backup_list_cli;
mod backup_prune_cli;
mod backup_restore_cli;
mod backup_restore_exit;
mod backup_restore_flow;
mod backup_restore_postgres;
mod backup_restore_report;
mod backup_restore_schema;
mod backup_restore_space;
mod backup_restore_sqlite;
mod backup_verify_cli;

/// The shared `backup` command test fixtures (harness, scratch dir, store).
#[cfg(test)]
#[path = "backup_cli_fixtures_tests.rs"]
pub(crate) mod backup_cli_fixtures;

/// The hand-built archive fixture and the root-user guard.
#[cfg(test)]
#[path = "backup_cli_fixtures_handmade_tests.rs"]
pub(crate) mod backup_cli_fixtures_handmade;
mod cli_args;
mod cli_args_suggest;
mod cli_command_bindings;
mod cli_command_registry;
mod cli_command_registry_core_reads;
mod cli_command_registry_core_writes;
mod cli_command_registry_graph_records;
mod cli_command_registry_persona_local;
mod cli_command_registry_shared_workspace;
mod cli_command_registry_task_history;
mod cli_command_registry_write_operations;
mod cli_command_spec;
mod cli_confirm;
mod cli_error;
mod cli_error_names;
mod cli_help;
mod cli_help_surfaces;
mod cli_help_usage;
mod cli_output;
mod cli_read;
mod cli_read_args;
mod cli_read_belief;
mod cli_read_core;
mod cli_read_graph;
mod cli_read_group;
mod cli_read_history;
mod cli_read_hygiene;
mod cli_read_render;
mod cli_read_workspace;
mod cli_style;
mod cli_top_level;

use cli_top_level::{
    help_topic, print_health_help, print_help, print_version_help, report_usage_error,
    unknown_command, wants_help,
};
mod cli_write;
mod cli_write_batch;
mod cli_write_core;
mod cli_write_duplicate;
mod cli_write_graph;
mod cli_write_group;
mod cli_write_hygiene;
mod cli_write_mutate;
mod cli_write_persona;
mod cli_write_portability;
mod cli_write_shared;
mod cli_write_workspace;
mod command_catalog;
mod compose_assets;
mod compose_cli;
mod compose_docker;
mod compose_env;
mod compose_ops;
mod config_cli;
mod connection_text;
mod count_noun;
mod db_target;
mod http_bind;
mod install_cli;
mod mcp_cli;
mod ops_cli;
mod port_scan;
mod ranking_cli;
mod retention_cli;
mod runtime_cli;
mod setup_ask;
mod setup_cli;
mod setup_cli_flows;
mod setup_config;
mod setup_connect;
mod setup_db_fields;
mod setup_db_options;
mod setup_defaults;
mod setup_flags;
mod setup_flows_db;
mod setup_flows_db_fields;
mod setup_flows_db_paste;
mod setup_flows_db_typed;
mod setup_key_source;
mod setup_managed;
mod setup_plan_db;
mod setup_preview;
mod setup_preview_db;
/// The shared wizard test harness: injected streams, faked probes, and a
/// scripted masked reader. `cfg(test)` rather than a `#[cfg(test)] mod`, because
/// several sibling suites use it and a `#[path]` submodule of one module would
/// make them all siblings of the wrong parent.
#[cfg(test)]
mod setup_preview_fixtures;
/// One run's results and the three ways a harness is driven to produce one.
///
/// `cfg(test)` for the same reason as `setup_preview_fixtures`: the drive methods
/// need `Probes::ok` and the scripted secret reader, both of which are test-only.
#[cfg(test)]
mod setup_preview_run;
/// The answer scripts the wizard's test suites drive, one per route.
#[cfg(test)]
mod setup_preview_scripts;
mod setup_probes;
mod setup_prompt;
mod setup_render;
mod setup_report;
mod setup_report_amend;
mod setup_review;
mod setup_secrets;
mod setup_service_fields;
mod setup_services;
mod setup_step_ask;
mod setup_step_extract;
mod setup_steps;
mod setup_store_state;
mod setup_style;
mod setup_summary;
mod setup_sync_config;
mod setup_sync_key_source;
mod setup_test_seam;
mod setup_validate;
mod setup_verify;
mod setup_wizard;
mod sigpipe;
mod status_cli;
mod status_report;
mod sync_cli;
mod sync_cli_render;
mod sync_key_cli;

use std::env;
use std::process::ExitCode;

/// Restore the default `SIGPIPE` disposition, then dispatch.
///
/// The disposition itself lives in `sigpipe`, because the long-running
/// transports re-install the ignore it replaces; see that module for why.
fn main() -> ExitCode {
    sigpipe::restore_default();
    run(env::args().skip(1))
}

fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let collected: Vec<String> = args.into_iter().collect();
    let (rest, globals) = cli_args::split_leading_globals(&collected);
    let mut args = rest.into_iter();
    // With no verb at all the catalog is what a reader needs; a bare version line
    // is what they got, and it named no command and no flag.
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let trailing = cli_args::merge_leading_globals(args, globals);
    dispatch(&cmd, trailing)
}

/// Route one command to its surface.
///
/// Also the target of `clio help <topic>`, which re-enters here with `--help`
/// appended so a topic's help is whatever that topic's own `--help` prints.
/// Routing both through one function is what keeps the two from disagreeing.
fn dispatch(cmd: &str, trailing: Vec<String>) -> ExitCode {
    match cmd {
        "version" | "--version" | "-V" => {
            if wants_help(&trailing) {
                return print_version_help();
            }
            println!("{}", clio::identity());
            ExitCode::SUCCESS
        }
        "health" => {
            if wants_help(&trailing) {
                return print_health_help();
            }
            println!(
                "{{\"ok\":true,\"name\":\"{}\",\"version\":\"{}\"}}",
                clio::name(),
                clio::version()
            );
            ExitCode::SUCCESS
        }
        "help" | "--help" | "-h" => {
            if trailing.iter().any(|arg| arg == "--json") {
                cli_help::write_catalog(&mut std::io::stdout()).expect("write help catalog");
                return ExitCode::SUCCESS;
            }
            match help_topic(&trailing) {
                Ok(Some(topic)) => dispatch(&topic.0, topic.1),
                Ok(None) => {
                    print_help();
                    ExitCode::SUCCESS
                }
                Err(error) => report_usage_error(&error, &trailing),
            }
        }
        "backup" => backup_cli::backup(trailing),
        "auth" => auth_cli::auth(trailing),
        "mcp" => mcp_cli::mcp(trailing),
        "ops" => ops_cli::ops(trailing),
        "status" => status_cli::status(trailing),
        "retention" => retention_cli::retention(trailing),
        "compose" => compose_cli::compose(trailing),
        "setup" => setup_cli::setup(trailing),
        "config" => config_cli::config(trailing),
        "ranking" => ranking_cli::ranking(trailing),
        "sync" => sync_cli::sync(trailing),
        "install" => install_cli::install(&trailing),
        "uninstall" => install_cli::uninstall(&trailing),
        "recall" | "get" | "show" | "inspect" | "stats" | "task" | "failure" | "memtree"
        | "history" | "triple" | "belief" | "graph" | "persona" | "intent" | "maintenance"
        | "audit" | "associations" | "compose-context" | "scratchpad" | "canonical" | "shared"
        | "validate" | "batch" | "hygiene" => cli_read::dispatch(cmd, trailing),
        "remember" | "admit" | "summarize" | "consolidate" | "update" | "invalidate"
        | "discard" | "correct" | "export" | "import" | "erase" => {
            cli_write::dispatch(cmd, trailing)
        }
        other => {
            let error = unknown_command(other);
            report_usage_error(&error, &trailing)
        }
    }
}

#[cfg(test)]
#[path = "cli_flags_test_support.rs"]
mod cli_flags_test_support;

#[cfg(test)]
#[path = "help_flags_test_support.rs"]
mod help_flags_test_support;

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "main_read_tests.rs"]
mod read_tests;

#[cfg(test)]
#[path = "main_write_tests.rs"]
mod write_tests;

#[cfg(test)]
#[path = "cli_command_registry_test_support.rs"]
mod cli_command_registry_test_support;

#[cfg(test)]
#[path = "cli_read_support_tests.rs"]
mod cli_read_test_support;

#[cfg(test)]
#[path = "cli_write_support_tests.rs"]
mod cli_write_test_support;

#[cfg(test)]
#[path = "sync_key_cli_test_support.rs"]
mod sync_key_cli_test_support;

#[cfg(test)]
#[path = "setup_support_tests.rs"]
pub(crate) mod setup_support_tests;
