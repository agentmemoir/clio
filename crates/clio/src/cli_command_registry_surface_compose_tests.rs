// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the compose records: their acceptance by the catalog
//! inventory, their authored usage-line and catalog-row bytes, and the flag
//! names their dispatcher parses with.
//!
//! ## Owns
//! - The `validate` contract for the two compose records
//! - The records' usage-line and summary bytes
//! - The shared accepted set and the record-derived parse names
//!
//! ## Does not own
//! - The registry's tranche registration (see `cli_command_registry_tests`)
//! - The compose dispatcher's exit codes (see `compose_cli_fail_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the compose records' data.

/// The compose records pass `validate` against the catalog inventory.
#[test]
fn the_compose_records_validate_against_the_catalog_inventory() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(
            &[&super::COMPOSE_UP, &super::COMPOSE_DOWN],
            &inventory
        ),
        Ok(())
    );
}

/// The records keep the usage-line and catalog-row bytes the authored rows
/// held, so the catalog and the surfaces render unchanged.
#[test]
fn the_compose_records_keep_their_authored_bytes() {
    assert_eq!(
        super::COMPOSE_UP.usage_line(),
        "clio compose up [--dir DIR] [--services LIST]"
    );
    assert_eq!(
        super::COMPOSE_DOWN.usage_line(),
        "clio compose down [--dir DIR] [--services LIST]"
    );
    assert_eq!(
        super::COMPOSE_UP.summary,
        "Materialize the compose stack and start the local Docker services (postgres, embed, rerank, extract)"
    );
    assert_eq!(
        super::COMPOSE_DOWN.summary,
        "Stop and remove the services `clio compose up` started; re-runnable"
    );
}

/// Every documented flag is a surface's own, and every extra the records name
/// is a declared global: the shared parser accepts the globals on every
/// surface, so the records state them rather than imply them.
#[test]
fn the_compose_records_state_their_extras_and_own_flags() {
    for spec in [&super::COMPOSE_UP, &super::COMPOSE_DOWN] {
        for flag in spec.documented {
            assert!(
                crate::cli_command_registry::global_flag(flag.long).is_none(),
                "{} documents --{}, which is a declared global",
                spec.name,
                flag.long
            );
        }
        for extra in spec.accepted_extra {
            assert!(
                crate::cli_command_registry::global_flag(extra).is_some(),
                "{} names --{extra}, which is not a declared global",
                spec.name
            );
        }
    }
}

/// The two subcommands accept one shared set, and the names their dispatcher
/// parses with are the records': `--dir` and `--services` in both value
/// spellings, a global accepted, and an unknown flag refused.
#[test]
fn the_compose_records_share_one_accepted_set() {
    let up = crate::cli_command_registry_surface::parse_flag_names("compose up");
    let down = crate::cli_command_registry_surface::parse_flag_names("compose down");
    assert_eq!(up, ["dir", "services"]);
    assert_eq!(up, down, "up and down accept the same set");
    let parse = |args: &[&str]| {
        crate::cli_top_level::parse_flags(args.iter().map(std::string::ToString::to_string), &up)
    };
    let dir = parse(&["--dir", "/tmp"]).expect("--dir is documented");
    assert_eq!(dir.get("dir").map(String::as_str), Some("/tmp"));
    let services = parse(&["--services=postgres,embed"]).expect("an inline value is accepted");
    assert_eq!(
        services.get("services").map(String::as_str),
        Some("postgres,embed")
    );
    assert!(parse(&["--yes"]).is_ok(), "a global is accepted");
    assert!(
        parse(&["--frobnicate"]).is_err(),
        "an unknown flag is refused"
    );
}
