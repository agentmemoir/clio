// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio config` CLI surface: reading the
//! effective configuration, setting a validated value at profile/deployment
//! scope, listing named profiles, and applying one.
//!
//! ## Owns
//! - The `config get|set|profiles|use` verbs and their text views
//! - Decoding the `VALUE` positional into the tool's JSON value
//! - Refusing the session scope, which a one-command process cannot observe
//! - Flushing a profile activation or profile-scoped set to the CLI state file
//!
//! ## Does not own
//! - Config merge order, validation, masking, or persistence (`clio-config`)
//! - The verbs' static records and verb list (see
//!   `cli_command_registry_auth_config`)
//! - The runtime-backed command engine (see `runtime_cli`)
//!
//! ## Boundary
//!
//! Changes to the config CLI flags or views should normally require changes here.
//! Changes to config semantics should not require changes here.
//!
//! Keep this module focused on the `clio config` surface.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_output;
use crate::cli_read::CliIo;
use crate::runtime_cli::{RuntimeGroup, no_positionals, pretty_without_ok};
use clio::Runtime;
use serde_json::{Map, Value, json};
use std::process::ExitCode;

/// The `clio config` command group.
pub(crate) struct ConfigGroup;

impl RuntimeGroup for ConfigGroup {
    /// The group's verb keys: the registry records' own names, so the
    /// dispatcher has no second list to drift.
    fn verbs(&self) -> &'static [&'static str] {
        crate::cli_command_registry_auth_config::CONFIG_VERBS
    }

    /// The flags `verb` accepts: the verb's registry record's parser spec,
    /// globals included.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(spec) => spec.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "config get" => get_call(parsed),
            "config set" => set_call(parsed),
            "config profiles" => {
                no_positionals(parsed)?;
                Ok(("config_profiles", json!({})))
            }
            _ => use_call(parsed),
        }
    }

    /// `config use` and a profile-scoped `config set` are the two writes a later
    /// command can observe, so the CLI saves them. A run that cannot save must
    /// not report the change as a success.
    fn commit(&self, verb: &str, payload: &Value, runtime: &mut Runtime) -> Result<(), CliError> {
        let durable = match verb {
            "config use" => true,
            "config set" => payload.get("scope").and_then(Value::as_str) == Some(SCOPE_PROFILE),
            _ => false,
        };
        if !durable {
            return Ok(());
        }
        match runtime.flush_cli_state() {
            Ok(true) => Ok(()),
            Ok(false) => Err(CliError::operational(
                "no_config_dir",
                "the config change applied to this process but was not saved: no clio config \
                 directory resolves, so later commands read the previous profile and values",
            )),
            Err(error) => Err(CliError::from_am_error(&error)),
        }
    }

    fn render(&self, verb: &str, payload: &Value) -> String {
        match verb {
            "config get" => render_get(payload),
            "config set" => render_set(payload),
            "config profiles" => render_profiles(payload),
            _ => render_use(payload),
        }
    }
}

/// Dispatch `clio config <subcommand>`.
pub(crate) fn config(args: Vec<String>) -> ExitCode {
    let env = |key: &str| std::env::var(key).ok();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &env,
    };
    crate::runtime_cli::run(&ConfigGroup, "config", args, &mut io, &mut Runtime::new())
}

/// `config get [PATH]` → `config_get`: effective config (secrets masked).
fn get_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let mut args = Map::new();
    match parsed.positionals() {
        [] => {}
        [path] => {
            args.insert("path".into(), Value::from(path.as_str()));
        }
        _ => {
            return Err(CliError::usage(format!(
                "unexpected extra argument `{}`",
                parsed.positional(1).unwrap_or_default()
            )));
        }
    }
    Ok(("config_get", Value::Object(args)))
}

/// The profile scope: the one `config set` scope this process can save.
pub(crate) const SCOPE_PROFILE: &str = "profile";
/// The session scope: in-memory only, so the CLI refuses it.
pub(crate) const SCOPE_SESSION: &str = "session";

/// `config set PATH VALUE [--scope profile|deployment]` → `config_set`.
///
/// `VALUE` is decoded as JSON when it parses (so numbers, booleans, null, and
/// quoted strings keep their type) and as a plain string otherwise. The tool
/// validates the path/value and the scope before any apply.
///
/// A session-scoped write is never sent: it would live in this process only,
/// which a one-command process ends immediately, so reporting it as applied
/// would be a promise nothing can keep.
fn set_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let [path, value] = parsed.positionals() else {
        return Err(CliError::usage_hint(
            "`config set` requires PATH and VALUE arguments",
            "try `clio config set backend postgres --scope deployment`",
        ));
    };
    let scope = parsed.value("scope").unwrap_or(SCOPE_SESSION);
    if scope == SCOPE_SESSION {
        return Err(CliError::operational(
            "invalid_argument",
            "a session-scoped value applies to the running process only, is discarded when this \
             command ends, and cannot be read by a later command; use `--scope deployment` to \
             persist the value in deployment.json, or `--scope profile` for the active profile",
        ));
    }
    let mut args = Map::new();
    args.insert("path".into(), Value::from(path.as_str()));
    args.insert("value".into(), parse_value(value));
    args.insert("scope".into(), Value::from(scope));
    Ok(("config_set", Value::Object(args)))
}

/// `config use NAME` → `config_profile_apply`: activate a profile.
fn use_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let [name] = parsed.positionals() else {
        return Err(CliError::usage_hint(
            "`config use` requires a profile NAME argument",
            "try `clio config use coding_local`",
        ));
    };
    Ok(("config_profile_apply", json!({ "name": name })))
}

/// Decode the `VALUE` positional: JSON when it parses, else the raw string.
fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// Human text for `config get`: a full effective config, or one dotted path.
fn render_get(payload: &Value) -> String {
    let path = payload.get("path").and_then(Value::as_str).unwrap_or("");
    let value = payload.get("value").unwrap_or(&Value::Null);
    if path.is_empty() {
        let profile = payload
            .get("active_profile")
            .and_then(Value::as_str)
            .unwrap_or("(none)");
        let body = serde_json::to_string_pretty(value).unwrap_or_default();
        return format!(
            "effective configuration (secrets masked; active profile {profile})\n{body}"
        );
    }
    let rendered = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!("config {path} = {rendered}")
}

/// Human text for `config set`: the path, scope, and applied (masked) value.
fn render_set(payload: &Value) -> String {
    let path = payload.get("path").and_then(Value::as_str).unwrap_or("?");
    let scope = payload
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let value = payload.get("value").unwrap_or(&Value::Null);
    let rendered = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!("config set {path} (scope {scope}) = {rendered}")
}

/// Human text for `config profiles`: one line per profile, active marked.
fn render_profiles(payload: &Value) -> String {
    let active = payload.get("active").and_then(Value::as_str);
    let mut lines = vec!["profiles:".to_owned()];
    if let Some(profiles) = payload.get("profiles").and_then(Value::as_array) {
        for profile in profiles {
            let name = profile.get("name").and_then(Value::as_str).unwrap_or("?");
            let marker = if Some(name) == active { "*" } else { " " };
            let single_host = profile
                .get("single_host")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let omit_sync = profile
                .get("omit_sync")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let description = profile
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            lines.push(format!(
                "{marker} {name} (single_host={single_host}, omit_sync={omit_sync}) {description}"
            ));
        }
    }
    lines.join("\n")
}

/// Human text for `config use`: the applied profile, its note, and the diff.
fn render_use(payload: &Value) -> String {
    let name = payload.get("name").and_then(Value::as_str).unwrap_or("?");
    let note = payload.get("note").and_then(Value::as_str).unwrap_or("");
    let bank_policy = payload
        .get("bank_policy")
        .map_or_else(|| "?".to_owned(), Value::to_string);
    let mut lines = vec![format!(
        "applied profile {name} (bank policy {bank_policy}); {note}"
    )];
    if let Some(diff) = payload.get("diff") {
        lines.push(pretty_without_ok(diff));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "config_cli_tests.rs"]
mod tests;
