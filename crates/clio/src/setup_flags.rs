// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio setup` flag grammar: the accepted
//! flag table, the parse from tokens to a map, the retirement message for
//! `--type`, and the `--help` text.
//!
//! ## Owns
//! - `ACCEPTED`, the flag table the setup record documents and this parser
//!   reads
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

use crate::cli_command_spec::Flag;
use std::collections::HashMap;

/// The flags `clio setup` accepts, in the order its usage line prints them.
///
/// The setup record documents this table and `parse_setup_args` parses it, so
/// the wizard's flag set is stated once: a flag the grammar gains lands here
/// and reaches the record and the parser together. `--help` is not here; it is
/// the surface's own help spelling, answered before the parser runs, and the
/// parser accepts it by name the way the shared parser does.
pub(crate) static ACCEPTED: &[Flag] = &[
    Flag::value("database", "managed|sqlite|existing"),
    Flag::value("config", "PATH"),
    Flag::value("db-path", "PATH"),
    Flag::value("database-url", "ADDRESS"),
    Flag::value("database-connect", "pasted|typed"),
    Flag::value("database-host", "H"),
    Flag::value("database-port", "P"),
    Flag::value("database-name", "N"),
    Flag::value("database-user", "U"),
    Flag::value("database-password", "P"),
    Flag::value("database-ssl", "MODE"),
    Flag::value("database-ssl-rootcert", "PATH"),
    Flag {
        repeatable: true,
        ..Flag::value("database-option", "NAME=VALUE")
    },
    Flag::value("embed", "local|online"),
    Flag::value("embed-url", "U"),
    Flag::value("embed-model", "M"),
    Flag::value("embed-key", "K"),
    Flag::value("embed-provider", "tei|openai"),
    Flag::value("embed-dims", "N"),
    Flag::value("rerank", "local|online|skip"),
    Flag::value("rerank-url", "U"),
    Flag::value("rerank-model", "M"),
    Flag::value("rerank-key", "K"),
    Flag::value("rerank-provider", "tei|cohere"),
    Flag::value("extract", "local|skip|online"),
    Flag::value("compose-dir", "DIR"),
    Flag::value("key-store", "file|keystore"),
    Flag::value("sync-store", "file|keystore"),
    Flag::boolean("allow-untested"),
    Flag::boolean("yes"),
    Flag::boolean("non-interactive"),
    Flag::boolean("interactive"),
];

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

/// Parse `--flag value` / `--boolean` tokens against the setup record's
/// accepted flags. Boolean flags are recognized so a valued flag may follow
/// them.
///
/// # Errors
/// An unexpected positional argument, an unknown flag, a missing value, or the
/// retired `--type` flag with its replacement message.
pub(crate) fn parse_setup_args(raw: &[String]) -> Result<HashMap<String, String>, String> {
    let accepted = accepted_flags();
    let mut flags = HashMap::new();
    let mut it = raw.iter();
    while let Some(token) = it.next() {
        let Some(name) = token.strip_prefix("--") else {
            return Err(format!(
                "unexpected argument `{token}`; try `clio setup --help`"
            ));
        };
        // `--help` is the surface's own help spelling: value-less, never a
        // record entry, accepted by name the way the shared parser accepts it.
        if name == "help" {
            flags.insert(name.to_owned(), "true".to_owned());
            continue;
        }
        let Some(flag) = accepted.iter().find(|flag| flag.long == name) else {
            if name == "type" {
                return Err(TYPE_REMOVED.to_owned());
            }
            if name == "rotate-database-password" {
                return Err(ROTATE_REMOVED.to_owned());
            }
            return Err(format!("unknown flag `--{name}`; try `clio setup --help`"));
        };
        if flag.value.is_none() {
            flags.insert(name.to_owned(), "true".to_owned());
            continue;
        }
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
        if flag.repeatable {
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
    }
    Ok(flags)
}

/// The setup record's accepted flags: the one table the record documents and
/// this parser reads.
fn accepted_flags() -> &'static [Flag] {
    crate::cli_command_registry::find("setup")
        .unwrap_or_else(|| panic!("`setup` is registered"))
        .documented
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
         SSL modes: disable, prefer, verify-full, encrypt-no-verify. verify-full checks the\n\
         certificate and the server name. encrypt-no-verify encrypts without checking the server,\n\
         protects against eavesdropping only, and is never a default. verify-ca is not offered.\n\
         Add sslrootcert=PATH (or --database-ssl-rootcert PATH) to trust a private certificate\n\
         authority. A remote host defaults to verify-full; this machine defaults to prefer.\n\n\
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
