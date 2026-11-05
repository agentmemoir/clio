// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that pin the two `clio mcp`
//! transport help screens: the order of their blocks, the database block built
//! from an injected environment, the port range the HTTP screen states, and what
//! the shared `--tools` block offers.
//!
//! ## Owns
//! - The block order of both screens and the usage line's fallback
//! - The port range the HTTP screen derives from the shared constants
//! - The preset names, the `custom:` form, and the default the `--tools` block
//!   states
//!
//! ## Does not own
//! - The registry usage lines themselves (see `cli_help_usage`)
//! - The database block's content (see `cli_top_level`)
//! - Decoding `--tools` values (see `mcp_tools_flag`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the two help screens.

use super::{http_help, mcp_http_help, mcp_stdio_help, stdio_help, usage_text};
use crate::cli_help_usage::usage_line;
use crate::cli_top_level::database_help_block;
use crate::mcp_tools_flag::HELP_BLOCK as TOOLS_HELP;
use clio_types::ToolPreset;

/// An environment holding nothing, so a screen is composed without host state.
fn no_env(_: &str) -> Option<String> {
    None
}

/// The scan target the HTTP screen advertises, from the shared constants.
fn scanned_target() -> String {
    format!(
        "127.0.0.1:{}-{}",
        crate::port_scan::FIRST_PORT,
        crate::port_scan::LAST_PORT
    )
}

/// The stdio screen prints the command's usage line first, the database block
/// second, and the tool selection last.
#[test]
fn the_stdio_screen_orders_usage_then_database_then_tools() {
    let text = stdio_help(&no_env);
    let usage = usage_line("mcp stdio").expect("`mcp stdio` has a registry record");
    let database = database_help_block(&no_env);
    assert!(text.starts_with(&usage), "{text}");
    assert!(
        text.contains(&format!("\n\n{database}\n\n")),
        "the database block is not its own paragraph: {text}"
    );
    assert!(text.ends_with(TOOLS_HELP), "{text}");
}

/// The HTTP screen prints the usage line first, its flag prose next, the tool
/// selection after the flags, and the database block last.
#[test]
fn the_http_screen_puts_the_usage_first_and_the_database_block_last() {
    let text = http_help(&no_env);
    let usage = usage_line("mcp http").expect("`mcp http` has a registry record");
    assert!(text.starts_with(&usage), "{text}");
    assert!(text.ends_with(&database_help_block(&no_env)), "{text}");
    let bind = text.find("--bind ADDR").expect("the bind prose is printed");
    let tools = text
        .find(TOOLS_HELP)
        .expect("the tool selection is printed");
    assert!(
        bind < tools,
        "the flag prose precedes the tools block: {text}"
    );
}

/// The HTTP screen states the range its bare `--bind` scan covers, taken from the
/// constants the scan itself uses.
#[test]
fn the_http_screen_names_the_range_the_bind_scan_covers() {
    let text = http_help(&no_env);
    let target = scanned_target();
    assert!(text.contains(&target), "`{target}` is missing: {text}");
}

/// A command the registry knows takes its usage line from the record, so the
/// screen and the catalog cannot drift apart.
#[test]
fn a_registered_command_takes_its_usage_line_from_the_registry() {
    let recorded = usage_line("mcp http").expect("`mcp http` has a registry record");
    assert_eq!(usage_text("mcp http"), recorded);
}

/// A command the registry does not know still yields a usage line naming it.
#[test]
fn an_unregistered_command_falls_back_to_a_plain_usage_line() {
    assert_eq!(usage_text("mcp nowhere"), "clio mcp nowhere");
}

/// The stdio entry point reads the process environment, so this pins only the
/// host-independent parts of what it prints.
#[test]
fn the_stdio_entry_point_composes_its_screen() {
    let text = mcp_stdio_help();
    let usage = usage_line("mcp stdio").expect("`mcp stdio` has a registry record");
    assert!(text.starts_with(&usage), "{text}");
    assert!(text.ends_with(TOOLS_HELP), "{text}");
}

/// The HTTP entry point reads the process environment, so this pins only the
/// host-independent parts of what it prints.
#[test]
fn the_http_entry_point_composes_its_screen() {
    let text = mcp_http_help();
    let target = scanned_target();
    assert!(text.contains(&target), "{text}");
    assert!(text.contains(TOOLS_HELP), "{text}");
    assert!(
        !text.ends_with(TOOLS_HELP),
        "the database block is last: {text}"
    );
}

/// The database block follows the lookup the screen is given, so the screen is
/// testable without the host's environment, and a planted credential is redacted
/// rather than printed.
#[test]
fn the_screen_reads_the_environment_through_the_lookup_it_is_given() {
    let planted = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://user:s3cret@127.0.0.1:5432/am".to_owned()),
        _ => None,
    };
    let text = stdio_help(&planted);
    assert!(
        text.contains("postgres://***@127.0.0.1:5432/am"),
        "the injected target is not shown: {text}"
    );
    assert!(
        !text.contains("s3cret"),
        "the help printed the credential: {text}"
    );
}

/// Every preset the flag decodes is offered on both screens, so a preset added
/// without its help wording fails here.
#[test]
fn both_screens_name_every_preset_the_flag_decodes() {
    for text in [stdio_help(&no_env), http_help(&no_env)] {
        for preset in ToolPreset::ALL {
            let name = preset.as_str();
            assert!(text.contains(name), "`{name}` is not offered: {text}");
        }
    }
}

/// Both screens show the `--tools` flag and the `custom:` form that extends the
/// minimal set with named groups.
#[test]
fn both_screens_offer_the_custom_group_list() {
    for text in [stdio_help(&no_env), http_help(&no_env)] {
        assert!(text.contains("--tools PRESET"), "{text}");
        assert!(text.contains("custom:"), "{text}");
        assert!(text.contains("GROUP[,GROUP]"), "{text}");
    }
}

/// The block names the setting that decides when no flag is given, and names the
/// default preset as what applies when that setting is unset.
#[test]
fn the_tools_block_defers_to_the_config_setting_and_falls_back_to_the_default() {
    let text = stdio_help(&no_env);
    let setting = text
        .split(". ")
        .find(|sentence| sentence.contains("mcp.tool_profile"))
        .unwrap_or_default();
    assert!(
        !setting.is_empty(),
        "the block names the config setting: {text}"
    );
    let default = clio_types::DEFAULT_PRESET.as_str();
    assert!(
        setting.contains(default),
        "`{default}` is not the stated fallback: {setting}"
    );
}
