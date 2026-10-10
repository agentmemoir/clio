// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the one way the test suites read a flag name out of help
//! text, so the catalog's parity checks and the error-message vocabulary check
//! cannot disagree about what counts as a flag.
//!
//! ## Owns
//! - `flags_in`, the flag names a help string names
//! - `entry_block`, the one catalog entry a command name selects
//!
//! ## Does not own
//! - Any assertion about those flags, which each suite makes for its own surface
//! - The usage lines themselves (`cli_help_usage`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds, and holds no test function:
//! two suites need it, so it cannot belong to either one.

/// Every `--flag` a string names, sorted and de-duplicated.
///
/// The split is on every character that cannot be part of a name, so a flag
/// inside `[...]`, after a `|`, or at the end of a sentence yields the same name.
/// Splitting on spaces alone silently missed `[--min-semantic`, which is how a
/// parity check passed while the flag it was looking for was right there.
///
/// `--help` is excluded. Every command accepts it, so it is not part of any one
/// command's flag list and naming it would prove nothing.
pub(crate) fn flags_in(text: &str) -> Vec<String> {
    let mut flags: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter_map(|token| token.strip_prefix("--"))
        .filter(|flag| !flag.is_empty() && *flag != "help")
        .map(|flag| format!("--{flag}"))
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

/// The catalog entry `command` names, plus every line below it in the same entry.
///
/// An entry's own line begins with the command, and its wrapped continuation and
/// description lines are indented past it, so the next entry is the next line
/// whose trimmed text begins with `clio `. Reading the block this way is what
/// makes an assertion about one command's flags immune to where the renderer
/// happened to break the line.
///
/// # Panics
/// When the catalog does not list `command`, which is the failure the caller is
/// testing for.
pub(crate) fn entry_block(text: &str, command: &str) -> String {
    let head = format!("clio {command}");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed == head || trimmed.starts_with(&format!("{head} "))
        })
        .unwrap_or_else(|| panic!("the catalog must list `{command}`"));
    std::iter::once(lines[start])
        .chain(
            lines[start + 1..]
                .iter()
                .copied()
                .take_while(|line| !line.trim_start().starts_with("clio ")),
        )
        .collect::<Vec<&str>>()
        .join("\n")
}
