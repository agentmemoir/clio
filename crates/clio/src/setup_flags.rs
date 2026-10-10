// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio setup` flag grammar: the flag sets,
//! the parse from tokens to a map, the retirement message for `--type`, and the
//! `--help` text.
//!
//! ## Owns
//! - `BOOLEAN_FLAGS`, `VALUE_FLAGS`, and the repeatable flag
//! - `parse_setup_args`, including the `--type` replacement message
//! - `print_help`, which documents every accepted flag
//!
//! ## Does not own
//! - What the flags mean to the questions (see `setup_steps` and its modules)
//! - Running the run and mapping outcomes to exit codes (see `setup_cli`)
//! - The `--type` behavior itself, which is deleted rather than kept: the
//!   message here is the whole of it
//!
//! ## Boundary
//!
//! Changes to a flag's name, arity, or help text should normally require changes
//! here. Changes to what a flag is used for should not.
//!
//! Keep this module focused on the command line's shape.

use std::collections::HashMap;

/// Flags that may not be followed by a value.
pub(crate) const BOOLEAN_FLAGS: &[&str] = &[
    "yes",
    "non-interactive",
    "interactive",
    "help",
    "allow-untested",
];
/// Flags that take exactly one value.
pub(crate) const VALUE_FLAGS: &[&str] = &[
    "config",
    "compose-dir",
    "database",
    "db-path",
    "database-url",
    "database-connect",
    "database-host",
    "database-port",
    "database-name",
    "database-user",
    "database-password",
    "database-ssl",
    "database-option",
    "embed",
    "embed-url",
    "embed-model",
    "embed-key",
    "embed-provider",
    "key-store",
    "sync-store",
    "embed-dims",
    "rerank",
    "rerank-url",
    "rerank-model",
    "rerank-key",
    "rerank-provider",
    "extract",
];

/// The flag whose values accumulate rather than replace.
///
/// `--database-option` names one of the thirteen advanced settings and may be
/// given more than once, so the parser appends with a newline rather than
/// overwriting. The one that appends is named here rather than special-cased in
/// the loop, so the next repeatable flag is one line.
const REPEATABLE_FLAG: &str = "database-option";

/// `--type`'s dedicated usage error.
///
/// The flag is retired rather than ignored, and an operator carrying an old
/// script is told what to use instead: an unknown-flag error cannot say that.
pub(crate) const TYPE_REMOVED: &str = "--type was removed; choose the database with --database, \
     and each service with --embed/--rerank/--extract. Run clio setup --help";

/// `--rotate-database-password`'s dedicated usage error.
///
/// Retired for the same reason and by the same route as [`TYPE_REMOVED`]. The
/// managed database has no password, so there is nothing to rotate; a flag that
/// silently did nothing would be worse than an error that says so.
pub(crate) const ROTATE_REMOVED: &str = "--rotate-database-password was removed; the Postgres \
     Clio manages has no password and is published on this machine only, so there is nothing to \
     rotate. Run clio setup --help";

/// Parse `--flag value` / `--boolean` tokens against the known flag sets.
/// Boolean flags are recognized so a valued flag may follow them.
///
/// # Errors
/// An unexpected positional argument, an unknown flag, a missing value, or the
/// retired `--type` flag with its replacement message.
pub(crate) fn parse_setup_args(raw: &[String]) -> Result<HashMap<String, String>, String> {
    let mut flags = HashMap::new();
    let mut it = raw.iter();
    while let Some(token) = it.next() {
        let Some(name) = token.strip_prefix("--") else {
            return Err(format!(
                "unexpected argument `{token}`; try `clio setup --help`"
            ));
        };
        if BOOLEAN_FLAGS.contains(&name) {
            flags.insert(name.to_owned(), "true".to_owned());
            continue;
        }
        if VALUE_FLAGS.contains(&name) {
            let value = it
                .next()
                .ok_or_else(|| format!("--{name} requires a value"))?;
            if value.starts_with("--") {
                return Err(format!("--{name} requires a value"));
            }
            if value.trim().is_empty() {
                return Err(
                    crate::cli_error::reject_blank_value(&format!("--{name}"), value)
                        .expect_err("an empty value is always refused")
                        .message()
                        .to_owned(),
                );
            }
            if name == REPEATABLE_FLAG {
                flags
                    .entry(name.to_owned())
                    .and_modify(|kept| {
                        kept.push('\n');
                        kept.push_str(value);
                    })
                    .or_insert_with(|| value.clone());
                continue;
            }
            crate::cli_error::reject_repeated_flag(&format!("--{name}"), flags.contains_key(name))
                .map_err(|error| error.message().to_owned())?;
            flags.insert(name.to_owned(), value.clone());
            continue;
        }
        if name == "type" {
            return Err(TYPE_REMOVED.to_owned());
        }
        if name == "rotate-database-password" {
            return Err(ROTATE_REMOVED.to_owned());
        }
        return Err(format!("unknown flag `--{name}`; try `clio setup --help`"));
    }
    Ok(flags)
}

/// The `--help` text: every flag, the database choices, and the SSL rule.
pub(crate) fn print_help(out: &mut impl std::io::Write) {
    // The flag list is the shared registry's, wrapped here rather than written
    // out twice, so a flag the catalog lists cannot be missing from `setup --help`.
    for line in crate::cli_help_surfaces::wrapped_usage("setup", 74, 0) {
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(
        out,
        "[--help]\n\
         Ask where your memories and your models should live, show a preview, then write\n\
         the deployment config the binary reads without any environment variable.\n\n\
         Database:\n\
         \x20 managed   Clio runs Postgres in Docker on this machine only, no password\n\
         \x20 sqlite    a SQLite file on this machine, no Docker\n\
         \x20 existing  a Postgres that already exists; paste the address or type the fields\n\
\n\
         The managed database publishes its port on 127.0.0.1 and asks Postgres for trust\n\
         authentication, so it is unreachable off this machine and needs no password. Every\n\
         process on this machine can still reach it, which is the ordinary exposure of a local\n\
         development database. --database-password applies to the existing route only.\n\
         SSL modes: disable, prefer, require. verify-ca and verify-full are not offered,\n\
         because this store has no way to encrypt without checking the server certificate.\n\
         A remote host defaults to require; this machine defaults to prefer.\n\n\
         Advanced connection settings (--database-option, repeatable): options,\n\
         application_name, connect_timeout, target_session_attrs, channel_binding,\n\
         sslnegotiation, keepalives, keepalives_idle, keepalives_interval,\n\
         keepalives_retries, tcp_user_timeout, hostaddr, load_balance_hosts.\n\n\
         Encryption key (asked once, then remembered; --key-store answers it):\n\
         \x20 file      an owner-only key file; works on any machine (recommended)\n\
         \x20 keystore  the OS secret store; more secure, needs one to exist\n\
         Sync key (asked once and only when multi-host sync is in play; --sync-store answers it):\n\
         \x20 file      an owner-only sync key file; share it with another node (recommended)\n\
         \x20 keystore  the OS secret store; suits one operator on one machine\n\
         Services:\n\
         \x20 --embed local runs the local Docker sidecar and needs no key\n\
         \x20 --embed online needs --embed-url, --embed-model, and --embed-key, and https\n\
         \x20 --rerank also accepts skip\n\
         \x20 --extract online is reported and skipped; hosted extraction does not exist yet\n\n\
         Every prompt has a flag equivalent: non-interactive runs (piped stdin, --yes, or\n\
         --non-interactive) never prompt and fail fast when a required flag is missing.\n\
         --allow-untested saves the answers when a connection or endpoint test fails, with\n\
         a warning that Clio will not work until the failure is fixed.\n\
         The default config path is $XDG_CONFIG_HOME/clio/deployment.json\n\
         (fallback ~/.config/clio/deployment.json); --config overrides it and\n\
         CLIO_DEPLOYMENT_CONFIG overrides the default (an empty value disables loading).\n\
         The per-service flags above replaced an earlier single type selector."
    );
}

#[cfg(test)]
#[path = "setup_flags_tests.rs"]
mod tests;
