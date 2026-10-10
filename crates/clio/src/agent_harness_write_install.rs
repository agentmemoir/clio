// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for installing and uninstalling Clio's MCP
//! server entry in a direct-write harness's JSON config file: the shared
//! entry builder, the manual-edit snippet, the merge install, and the
//! uninstall wrapper.
//!
//! ## Owns
//! - The JSON entry shape Clio writes for a direct-write target, built from
//!   the harness descriptor and the server argv; the one builder for both
//!   the written entry and the manual snippet
//! - Install: create a missing config file, or merge Clio's entry beside
//!   foreign entries in an existing one, with backup and atomic replace
//! - Uninstall: remove only Clio's entry from the configured top-level key
//!
//! ## Does not own
//! - The file-safety primitives it reuses: backup, parse refusal, atomic
//!   replace, and entry removal (`agent_harness_write`)
//! - Which file each harness uses and its entry `type` field
//!   (`agent_harness_targets`)
//! - Deciding when an install runs, or reporting its result to the user
//!
//! ## Boundary
//!
//! Changes to how Clio's entry is shaped, merged into, or removed from a
//! direct-write config should normally require changes here. Changes to the
//! safety primitives or to which files the harnesses use should not.
//!
//! Keep this module focused on the direct-write install and uninstall merge.

use crate::agent_harness_targets::DirectWriteConfig;
use crate::agent_harness_write::{
    RemoveOutcome, WriteError, backup_file, read_json, remove_server_from_file, write_json_atomic,
};
use serde_json::{Map, Value};
use std::ffi::OsString;

/// What happened to a requested install.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    /// The config file did not exist and has been created holding only
    /// Clio's entry. No backup exists: there was nothing to back up.
    Created,
    /// The config file existed and was merged. The backup holds the file's
    /// bytes before the edit. Clio's own entry was replaced; every foreign
    /// entry and unknown top-level key was kept.
    Merged { backup: std::path::PathBuf },
}

/// The JSON entry Clio installs for a direct-write target: the entry `type`
/// when the harness wants one, then `command` from `argv[0]` and `args` from
/// the rest of the argv. The caller passes the same argv the harness will
/// run, built by `server_argv`.
pub(crate) fn direct_write_entry(config: &DirectWriteConfig, argv: &[OsString]) -> Value {
    let mut entry = Map::new();
    if let Some(entry_type) = config.entry_type {
        entry.insert("type".to_owned(), Value::String(entry_type.to_owned()));
    }
    if let Some(command) = argv.first() {
        entry.insert(
            "command".to_owned(),
            Value::String(command.to_string_lossy().into_owned()),
        );
    }
    entry.insert(
        "args".to_owned(),
        Value::Array(
            argv.iter()
                .skip(1)
                .map(|arg| Value::String(arg.to_string_lossy().into_owned()))
                .collect(),
        ),
    );
    Value::Object(entry)
}

/// The JSON fragment a manual edit adds for Clio's entry: the config's
/// top-level key holding only Clio's entry, pretty-rendered. Built from the
/// same entry builder install writes, so the printed snippet and the written
/// entry can never disagree.
pub(crate) fn direct_write_entry_snippet(
    config: &DirectWriteConfig,
    server_name: &str,
    argv: &[OsString],
) -> String {
    let mut servers = Map::new();
    servers.insert(server_name.to_owned(), direct_write_entry(config, argv));
    let mut fragment = Map::new();
    fragment.insert(config.top_level_key.to_owned(), Value::Object(servers));
    serde_json::to_string_pretty(&Value::Object(fragment))
        .expect("a fragment of strings and objects serializes")
}

/// Install Clio's entry into the direct-write config file, merging beside
/// every foreign entry.
///
/// A missing file is created (its parent directory is created too, mode
/// `0o644`), with no backup. An existing file must strict-parse: JSONC
/// comments or trailing commas refuse the install with the manual snippet
/// and the offending line's number, and the file stays byte-identical with no backup
/// and no temp file. A parseable file is backed up first, then Clio's entry
/// replaces only Clio's own entry under the configured top-level key —
/// foreign entries, unknown top-level keys, and other values such as a
/// VS Code `inputs` array are re-serialized unchanged. The rewrite is an
/// atomic same-directory replace preserving the file's permission bits. A
/// file (or server map) that parses but is not a JSON object is refused
/// without a rewrite.
///
/// # Errors
/// A read, backup, or write failure, an unparseable file, or a parsed
/// non-object where an object is required.
pub(crate) fn install_server_from_file(
    config: &DirectWriteConfig,
    server_name: &str,
    argv: &[OsString],
) -> Result<InstallOutcome, WriteError> {
    let entry = direct_write_entry(config, argv);
    let mut root = match read_json(&config.path) {
        Ok(Some(root)) => root,
        Ok(None) => return create_config_file(config, server_name, entry),
        Err(WriteError::Unparseable {
            path,
            snippet: line_note,
        }) => {
            return Err(WriteError::Unparseable {
                path,
                snippet: format!(
                    "{} {line_note}",
                    direct_write_entry_snippet(config, server_name, argv)
                ),
            });
        }
        Err(err) => return Err(err),
    };
    let Value::Object(map) = &mut root else {
        return Err(WriteError::Io {
            detail: format!(
                "{} does not hold a JSON object, so Clio cannot merge its entry into it",
                config.path.display()
            ),
        });
    };
    let servers = match map.entry(config.top_level_key) {
        serde_json::map::Entry::Occupied(entry) => match entry.into_mut() {
            Value::Object(servers) => servers,
            other => {
                return Err(WriteError::Io {
                    detail: format!(
                        "{} holds \"{}\" as a {}, not an object",
                        config.path.display(),
                        config.top_level_key,
                        json_type_name(other)
                    ),
                });
            }
        },
        serde_json::map::Entry::Vacant(entry) => match entry.insert(Value::Object(Map::new())) {
            Value::Object(servers) => servers,
            _ => unreachable!("the vacant entry was just set to an object"),
        },
    };
    servers.insert(server_name.to_owned(), entry);
    let backup = backup_file(&config.path)?.ok_or_else(|| WriteError::Io {
        detail: format!(
            "{} vanished before it could be backed up",
            config.path.display()
        ),
    })?;
    write_json_atomic(&config.path, &root)?;
    Ok(InstallOutcome::Merged { backup })
}

/// Remove Clio's entry from a direct-write config file, using the config's
/// top-level key and the manual snippet from the shared entry builder.
///
/// # Errors
/// The same failures `remove_server_from_file` reports.
pub(crate) fn remove_server_from_direct_write(
    config: &DirectWriteConfig,
    server_name: &str,
    argv: &[OsString],
) -> Result<RemoveOutcome, WriteError> {
    remove_server_from_file(
        &config.path,
        &[&[config.top_level_key]],
        server_name,
        &direct_write_entry_snippet(config, server_name, argv),
    )
}

/// Write a new config file holding only Clio's entry, creating its parent
/// directory when it is missing.
fn create_config_file(
    config: &DirectWriteConfig,
    server_name: &str,
    entry: Value,
) -> Result<InstallOutcome, WriteError> {
    if let Some(parent) = config.path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io {
            detail: format!("failed to create {}: {e}", parent.display()),
        })?;
    }
    let mut servers = Map::new();
    servers.insert(server_name.to_owned(), entry);
    let mut root = Map::new();
    root.insert(config.top_level_key.to_owned(), Value::Object(servers));
    write_json_atomic(&config.path, &Value::Object(root))?;
    Ok(InstallOutcome::Created)
}

/// The name of a JSON value's type, for a refusal message.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[path = "agent_harness_write_install_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_harness_write_install_refusal_tests.rs"]
mod refusal_tests;

#[cfg(test)]
#[path = "agent_harness_write_install_flow_tests.rs"]
mod flow_tests;
