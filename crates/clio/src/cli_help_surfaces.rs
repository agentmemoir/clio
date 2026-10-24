// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared usage-line wrapping and the `ops`
//! group's usage lines.
//!
//! ## Owns
//! - The wrapping every help surface and the catalog share, so one command's line
//!   breaks the same way in both places
//! - `ops_usage_lines`, which selects the `ops` subcommand's own line or all five
//!   from the registry records, because `ops` is one verb with five subcommands
//!   and each has its own flags
//! - `SURFACE_USAGE`, a test-only fixture of the reserved surfaces' authored
//!   bytes: the record rows' byte reference the reserved-surface and
//!   ranking/retention record tests read through the test-only `surface_usage_line`
//! - `surface_usage_line`, the test-only byte reference into `SURFACE_USAGE`
//!
//! ## Does not own
//! - A registered command's usage line (see `cli_command_registry` and
//!   `cli_command_spec`)
//! - The usage lines of the commands that bind an MCP tool (`cli_help_usage`)
//! - Each surface's own extra prose, which the surface keeps
//! - The catalog's layout (see `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to how help is laid out should normally require changes here. Changes
//! to what one of these commands accepts should not: they require changes to its
//! registry record.
//!
//! Keep this module focused on the shared wrapping and the `ops` group's lines.

/// A command's usage line wrapped to `width`, every line carrying `lead` spaces.
///
/// Wrapping belongs here rather than in each surface so every surface breaks the
/// same line the same way. A usage line longer than a terminal is unreadable, and
/// one surface wrapping while another did not is how two help screens for the same
/// command stopped matching. A continuation lines up under the first flag, which is
/// where `lead` plus the width of `clio <command> ` puts it.
///
/// A unit longer than the remaining width is never split: half a flag name names a
/// flag that does not exist. It goes on the next line instead.
pub(crate) fn wrapped_usage(command: &str, width: usize, lead: usize) -> Vec<String> {
    let Some(usage) = crate::cli_help_usage::usage_line(command) else {
        return vec![format!(
            "{:lead$}clio {command} (no usage line registered)",
            ""
        )];
    };
    let indent = " ".repeat(lead + 5 + command.len() + 1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for unit in flag_units(&usage) {
        let candidate = if current.is_empty() {
            unit.to_owned()
        } else {
            format!("{current} {unit}")
        };
        // The width has to include the indent the line will carry, or every
        // continuation runs `indent.len()` columns past the column it was given.
        let carried = if current.is_empty() {
            lead
        } else {
            indent.len()
        };
        if !current.is_empty() && carried + candidate.len() > width {
            lines.push(std::mem::take(&mut current));
            current.clone_from(&unit.to_owned());
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let pad = " ".repeat(lead);
            if index == 0 {
                format!("{pad}{line}")
            } else {
                format!("{indent}{line}")
            }
        })
        .collect()
}

/// The usage line's tokens, each flag glued to the placeholder that follows it.
///
/// `[--database-password P]` is two whitespace-separated tokens, and wrapping
/// between them leaves a line that starts with `P]` — which reads as a command
/// argument nobody can type. A flag and its placeholder are one thing to a reader,
/// so they are one unit here.
///
/// # Panics
/// Never. Every slice it returns is taken from `usage`, so the units are always in
/// bounds even when the text holds an unexpected number of spaces.
fn flag_units(usage: &str) -> Vec<&str> {
    let mut units: Vec<&str> = Vec::new();
    let mut rest = usage;
    while !rest.is_empty() {
        let (token, after_token) = split_first(rest);
        let (next, _) = split_first(after_token);
        // Both sides are trimmed of the bracket or pipe that may open them, or
        // `[--yes] [--no-input]` reads as one flag that takes the next flag's
        // bracket as its value and the two stop being separable.
        let names_a_flag = token.trim_start_matches(['[', '|']).starts_with("--");
        let next_is_a_flag = next.trim_start_matches(['[', '|']).starts_with("--");
        let takes_value = names_a_flag && !next.is_empty() && !next_is_a_flag;
        if takes_value {
            let end = token.len() + 1 + next.len();
            units.push(&rest[..end]);
            rest = &rest[end..];
        } else {
            units.push(token);
            rest = after_token;
        }
        rest = rest.trim_start_matches(' ');
    }
    units
}

/// The first whitespace-separated token of `text`, and everything after it.
fn split_first(text: &str) -> (&str, &str) {
    text.split_once(' ').unwrap_or((text, ""))
}

/// One command's registered usage line from the reserved surfaces, if any.
///
/// Test-only: no production caller reads the table through here, because
/// `cli_help_usage` answers every registered command from its record. The
/// reserved-surface record tests read it as the byte reference their records
/// are pinned against.
#[cfg(test)]
pub(crate) fn surface_usage_line(command: &str) -> Option<&'static str> {
    SURFACE_USAGE
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, line)| *line)
}

/// The `ops` usage lines, either the named subcommand's own or all five.
///
/// `ops` is one verb with five subcommands, and a shared line listing only the
/// flags they have in common is what let `ops repair --help` hide `--plan` and
/// `--actions`. Each subcommand's line comes from its registry record, and this
/// only selects among them in the order those records are declared.
pub(crate) fn ops_usage_lines(sub: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = crate::cli_command_registry::all()
        .filter(|spec| match sub {
            "" => spec.name.starts_with("ops "),
            sub => spec.name == format!("ops {sub}"),
        })
        .map(crate::cli_command_spec::CommandSpec::usage_line)
        .collect();
    (!lines.is_empty()).then_some(lines)
}

/// The reserved surfaces' authored bytes, kept only as a test fixture.
///
/// Every command's usage line lives on its registry record, and `cli_help_usage`
/// answers the catalog and each verb's `--help` from there. The reserved-surface
/// and ranking/retention record tests read these bytes through the test-only
/// `surface_usage_line` as the reference their records are pinned against. The
/// two `compose` rows are gone: their records' bytes are pinned directly by the
/// compose record tests.
#[cfg(test)]
const SURFACE_USAGE: &[(&str, &str)] = &[
    ("version", "clio version"),
    ("health", "clio health"),
    ("help", "clio help [--json] [--markdown --out DIR] [TOPIC]"),
    (
        "mcp schema-export",
        "clio mcp schema-export [--out FILE] [--path FILE]",
    ),
    (
        "mcp stdio",
        "clio mcp stdio [--db PATH] [--backend sqlite|postgres] [--bank BANK]",
    ),
    (
        "mcp http",
        "clio mcp http [--bind [ADDR]] [--db PATH] [--backend sqlite|postgres] \
         [--bank BANK] [--token-file PATH] [--sessions] [--insecure-no-auth]",
    ),
    (
        "ops diagnose",
        "clio ops diagnose [--db PATH] [--backend sqlite|postgres] [--bank BANK]",
    ),
    (
        "ops verify",
        "clio ops verify [--db PATH] [--backend sqlite|postgres] [--bank BANK]",
    ),
    (
        "ops doctor",
        "clio ops doctor [--db PATH] [--backend sqlite|postgres] [--bank BANK] [--dry-run false]",
    ),
    (
        "ops repair",
        "clio ops repair [--db PATH] [--backend sqlite|postgres] [--bank BANK] [--plan PLAN_ID] \
         [--actions a,b] [--confirm]",
    ),
    (
        "ops reindex",
        "clio ops reindex [--db PATH] [--backend sqlite|postgres] [--bank BANK] \
         [--target dense|lexical|all] [--dry-run] [--confirm]",
    ),
    (
        "status",
        "clio status [--db PATH] [--backend sqlite|postgres] [--dir DIR] [--output json|text]",
    ),
    (
        "setup",
        "clio setup [--database managed|sqlite|existing] [--config PATH] [--db-path PATH] \
         [--database-url ADDRESS] [--database-connect pasted|typed] [--database-host H] \
         [--database-port P] [--database-name N] [--database-user U] \
         [--database-password P] [--database-ssl MODE] [--database-ssl-rootcert PATH] \
         [--database-option NAME=VALUE]... \
         [--embed local|online] [--embed-url U] [--embed-model M] [--embed-key K] \
         [--embed-provider tei|openai] [--embed-dims N] [--rerank local|online|skip] \
         [--rerank-url U] [--rerank-model M] [--rerank-key K] [--rerank-provider tei|cohere] \
         [--extract local|skip|online] [--compose-dir DIR] [--key-store file|keystore] \
         [--sync-store file|keystore] \
         [--allow-untested] [--yes] [--non-interactive] [--interactive]",
    ),
    ("retention schema", "clio retention schema"),
    (
        "retention get",
        "clio retention get [--bank BANK] [--profiles FILE]",
    ),
    (
        "retention set",
        "clio retention set [--bank BANK] [--scope bank|deployment] --profile JSON \
         [--profile-file FILE] [--profiles FILE]",
    ),
    ("install", "clio install --harness <name>"),
    ("uninstall", "clio uninstall --harness <name>"),
];

#[cfg(test)]
#[path = "cli_help_surfaces_tests.rs"]
mod tests;
