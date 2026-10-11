// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reserved surfaces' usage lines and the
//! wrapping every help surface shares. A moved command's line lives on its
//! registry record; the table holds the same bytes as the record rows' byte
//! reference, and the three retention rows are the lines of the commands that
//! have not moved yet.
//!
//! ## Owns
//! - `SURFACE_USAGE`, the reserved surfaces' lines: the record rows' byte
//!   reference the reserved-surface record tests read, plus the retention
//!   lines of the commands without a record and the `ops` lines the ops
//!   surface renders as its group help
//! - `surface_usage_line`, the table half of the shared usage lookup
//! - The wrapping every help surface and the catalog share, so one command's line
//!   breaks the same way in both places
//! - `ops_usage_lines`, because `ops` is one verb with five subcommands and each
//!   has its own flags
//!
//! ## Does not own
//! - The usage lines of the commands that bind an MCP tool (`cli_help_usage`)
//! - A registered command's usage line (see `cli_command_registry`)
//! - Each surface's own extra prose, which the surface keeps
//! - The catalog's layout (see `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to what one of these commands accepts should normally require changes
//! to its registry record, and to the row beside it here while the row stays a
//! record's byte reference. A change to how help is laid out should not.
//!
//! Keep this module focused on these commands' flag lists and the shared wrapping.

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
/// The shared lookup lives in `cli_help_usage`, so this is the table half of it
/// rather than a second lookup that could answer differently.
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
/// `--actions`. Each subcommand has its own line in [`SURFACE_USAGE`], and this
/// only selects among them.
pub(crate) fn ops_usage_lines(sub: &str) -> Option<Vec<&'static str>> {
    let lines: Vec<&'static str> = SURFACE_USAGE
        .iter()
        .filter(|(name, _)| match sub {
            "" => name.starts_with("ops "),
            sub => *name == format!("ops {sub}"),
        })
        .map(|(_, line)| *line)
        .collect();
    (!lines.is_empty()).then_some(lines)
}

/// The usage line for a command this surface owns, if any.
/// Usage lines for the commands that bind no MCP tool and own their own `--help`.
///
/// Each of these is printed by the surface named beside it, and by the catalog.
/// The surface keeps its own extra prose; only the flag list is shared, because
/// the flag list is the part that drifts.
///
/// The fifteen reserved-surface rows stay: the reserved-surface record tests
/// read them through `surface_usage_line` as the byte reference their records
/// are pinned against. The five `ops` rows also render: the ops surface reads
/// them through `ops_usage_lines` for its group and per-subcommand help. The
/// other ten never render through the shared lookup, because `cli_help_usage`
/// answers a registered command from its record first. The two `compose` rows
/// are gone: their records' bytes are pinned directly by the compose record
/// tests.
const SURFACE_USAGE: &[(&str, &str)] = &[
    ("version", "clio version"),
    ("health", "clio health"),
    ("help", "clio help [--json] [TOPIC]"),
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
         [--bank BANK] [--token-file PATH] [--sessions]",
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
         [--target dense|lexical|all] [--dry-run false] [--confirm]",
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
