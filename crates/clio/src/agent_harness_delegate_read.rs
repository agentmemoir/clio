// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading a harness's config file back and
//! saying whether Clio's MCP server entry is present and points at Clio's own
//! executable. The delegate runner uses it to decide the outcome of a vendor
//! command instead of trusting the vendor's exit code.
//!
//! ## Owns
//! - The parsed-entry shape returned to the delegate runner: a JSON value or,
//!   for a text config, whether that text names this executable
//! - Reading and checking each read-back shape: JSON key prefixes, a TOML
//!   server table, and a YAML server map
//! - Deciding whether an entry names this executable with the `mcp stdio`
//!   server arguments
//!
//! ## Does not own
//! - The harness registry, including each config file name and read-back
//!   shape (see `agent_harness`)
//! - Running any vendor command or editing any config file
//!
//! ## Boundary
//!
//! Changes to how a config file is read back or how an entry is judged should
//! normally require changes here. Changes to which harnesses exist, where
//! their config files live, or how their vendor commands run should not
//! require changes here.
//!
//! Keep this module focused on reading a config file back.

use crate::agent_harness::{HarnessRecord, ReadBack, server_argv};
use std::path::Path;

/// A server entry read back from a harness config file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Entry {
    /// The JSON value of the server entry.
    Json(serde_json::Value),
    /// A TOML/YAML config's own block for the server: `names_exe` records
    /// whether that block names this executable, `has_server_args` whether it
    /// also carries the `mcp stdio` server arguments.
    Text {
        names_exe: bool,
        has_server_args: bool,
    },
}

/// Read the server entry from the record's config file. `Ok(None)` when the
/// file is missing or has no such entry. `Err(detail)` when the file exists
/// but cannot be read or, for JSON, parsed.
pub(crate) fn read_entry(
    record: &HarnessRecord,
    config: &Path,
    server_name: &str,
    exe: Option<&Path>,
) -> Result<Option<Entry>, String> {
    match record.read_back {
        ReadBack::Json { key_prefixes } => read_json_entry(config, key_prefixes, server_name),
        ReadBack::TomlServerTable => read_text_entry(config, server_name, exe, toml_server_block),
        ReadBack::YamlServerMap => read_text_entry(config, server_name, exe, yaml_server_block),
    }
}

/// Read a JSON config, then look the server up under each key prefix in
/// order. The first prefix that names the server wins.
fn read_json_entry(
    config: &Path,
    key_prefixes: &[&[&str]],
    server_name: &str,
) -> Result<Option<Entry>, String> {
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{} could not be read: {err}", config.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", config.display()))?;
    for prefix in key_prefixes {
        let mut node = &value;
        let mut found = true;
        for key in *prefix {
            if let Some(next) = node.get(*key) {
                node = next;
            } else {
                found = false;
                break;
            }
        }
        if found && let Some(entry) = node.get(server_name) {
            return Ok(Some(Entry::Json(entry.clone())));
        }
    }
    Ok(None)
}

/// Read a text config and ask the shape predicate for the server's own block.
/// `names_exe` is whether that block names this executable, `has_server_args`
/// whether it also carries the `mcp stdio` arguments, so an executable under
/// a different server's block does not count.
fn read_text_entry(
    config: &Path,
    server_name: &str,
    exe: Option<&Path>,
    server_block: fn(&str, &str) -> Option<String>,
) -> Result<Option<Entry>, String> {
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{} could not be read: {err}", config.display())),
    };
    let Some(block) = server_block(&text, server_name) else {
        return Ok(None);
    };
    let names_exe = exe.is_some_and(|exe| block.contains(exe.to_string_lossy().as_ref()));
    let has_server_args = block.contains("mcp") && block.contains("stdio");
    Ok(Some(Entry::Text {
        names_exe,
        has_server_args,
    }))
}

/// Whether an entry names this executable and the `mcp stdio` server args.
pub(crate) fn entry_points_at(entry: &Entry, exe: &Path) -> bool {
    match entry {
        Entry::Text {
            names_exe,
            has_server_args,
        } => *names_exe && *has_server_args,
        Entry::Json(value) => json_points_at(value, exe),
    }
}

/// Whether a JSON server entry's `command` is this executable and its `args`
/// are exactly `mcp stdio`. A vendor shape that stores the whole argv in
/// `command` is accepted only when that array is exactly this executable
/// followed by `mcp stdio`. Anything else is rejected, so an executable named
/// elsewhere in the entry can never count as a match.
fn json_points_at(entry: &serde_json::Value, exe: &Path) -> bool {
    if command_is_exe(entry, exe) && args_are_server(entry, exe) {
        return true;
    }
    command_array_is_server(entry, exe)
}

fn command_is_exe(entry: &serde_json::Value, exe: &Path) -> bool {
    entry
        .get("command")
        .and_then(|value| value.as_str())
        .is_some_and(|command| Path::new(command) == exe)
}

fn args_are_server(entry: &serde_json::Value, exe: &Path) -> bool {
    let expected: Vec<String> = server_argv(exe)
        .iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    entry
        .get("args")
        .and_then(|value| value.as_array())
        .is_some_and(|args| {
            args.len() == expected.len()
                && args
                    .iter()
                    .zip(&expected)
                    .all(|(actual, want)| actual.as_str() == Some(want.as_str()))
        })
}

/// Whether `command` holds the whole server argv: this executable followed by
/// `mcp stdio`. Some vendors store the argv as one array instead of a command
/// string plus an args array.
fn command_array_is_server(entry: &serde_json::Value, exe: &Path) -> bool {
    let expected: Vec<String> = server_argv(exe)
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    entry
        .get("command")
        .and_then(|value| value.as_array())
        .is_some_and(|command| {
            command.len() == expected.len()
                && command
                    .iter()
                    .zip(&expected)
                    .all(|(actual, want)| actual.as_str() == Some(want.as_str()))
        })
}

/// The `[mcp_servers.<name>]` table's own lines, from its header to the next
/// table header, or `None` when the table is absent. An optionally quoted
/// name is accepted.
fn toml_server_block(text: &str, server_name: &str) -> Option<String> {
    let mut block = String::new();
    let mut in_block = false;
    for line in text.lines() {
        if in_block && line.trim_start().starts_with('[') {
            break;
        }
        if in_block || toml_header_names(line, server_name) {
            block.push_str(line);
            block.push('\n');
            in_block = true;
        }
    }
    in_block.then_some(block)
}

/// Whether `line` is the `[mcp_servers.<name>]` header. A trailing comment
/// after the header is allowed.
fn toml_header_names(line: &str, server_name: &str) -> bool {
    strip_toml_comment(line)
        .trim()
        .strip_prefix("[mcp_servers.")
        .and_then(|rest| rest.strip_suffix(']'))
        .is_some_and(|inner| inner.trim().trim_matches('"') == server_name)
}

/// `line` without a trailing `#` comment. A `#` inside quotes belongs to the
/// value, so only a `#` outside quotes starts a comment.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if let Some(mark) = quote {
            if mark == '"' && escaped {
                escaped = false;
            } else if mark == '"' && ch == '\\' {
                escaped = true;
            } else if ch == mark {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '#' {
            return line[..index].trim_end();
        }
    }
    line
}

/// The indented block belonging to the `<name>:` key under a top-level
/// `mcp_servers:` map, or `None` when that key is absent. The block ends at
/// the next line indented no deeper than the key.
fn yaml_server_block(text: &str, server_name: &str) -> Option<String> {
    let mut in_map = false;
    let mut key_indent: Option<usize> = None;
    let mut block = String::new();
    for line in text.lines() {
        if let Some(indent) = key_indent {
            if line.trim().is_empty() || indent_of(line) > indent {
                block.push_str(line);
                block.push('\n');
                continue;
            }
            break;
        }
        if line.trim_end() == "mcp_servers:" {
            in_map = true;
            continue;
        }
        if !in_map || line.trim().is_empty() {
            continue;
        }
        if indent_of(line) == 0 {
            in_map = false;
            continue;
        }
        if line.trim().strip_suffix(':') == Some(server_name) {
            key_indent = Some(indent_of(line));
            block.push_str(line);
            block.push('\n');
        }
    }
    key_indent.map(|_| block)
}

/// The number of leading whitespace bytes in `line`.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[cfg(test)]
#[path = "agent_harness_delegate_read_tests.rs"]
mod tests;
