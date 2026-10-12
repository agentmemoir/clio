// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how the top level of the CLI reads its own
//! arguments and answers a request for help: which spellings mean help, what an
//! unknown command says, what `clio help <topic>` resolves to, and the block every
//! surface prints beneath its own flags.
//!
//! ## Owns
//! - `parse_flags`, the reserved surfaces' name-to-value flag map
//! - `record_flag_names`, the flag names a surface's registry record documents
//! - `wants_help`, `unknown_command` and `help_topic`, the top level's own refusals
//! - `print_record_help` and the two no-flag commands' help, rendered from
//!   their registry records
//! - `wants_json_catalog`, the help record's JSON-catalog request
//! - `markdown_request`, the help record's whole-manual request, validated
//!   before any write
//! - `database_help_block`, the shared precedence-and-environment block
//! - The catalog's own screen
//!
//! ## Does not own
//! - Which commands exist and what flags they take (`cli_help` and `cli_help_usage`)
//! - The verb dispatch that reaches this module (`main`)
//! - Each surface's own flag list and help prose
//!
//! ## Boundary
//!
//! Changes to what the top level accepts or prints should normally require changes
//! here. Adding a command or a flag should not.
//!
//! It is its own file because the crate root is at its own size limit and this is a
//! separate concern from running the program.
//!
//! Keep this module focused on how the top level reads its own arguments and
//! answers help.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli_error::CliError;
use crate::cli_help;
use crate::cli_output;
use crate::command_catalog;
use crate::port_scan;

/// Whether the arguments ask for help rather than for the command's work.
pub(crate) fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
}

/// The flag names a surface's registry record documents, in print order: the
/// names `parse_flags` gates the surface's own flags on.
pub(crate) fn record_flag_names(name: &str) -> Vec<&'static str> {
    let spec =
        crate::cli_command_registry::find(name).unwrap_or_else(|| panic!("`{name}` is registered"));
    spec.documented.iter().map(|flag| flag.long).collect()
}

/// Whether the raw arguments select the help record's JSON catalog: the `json`
/// boolean's spelling is read from the help record, and only `--json` selects
/// the catalog, so `--markdown` never does.
pub(crate) fn wants_json_catalog(args: &[String]) -> bool {
    let spec =
        crate::cli_command_registry::find("help").unwrap_or_else(|| panic!("`help` is registered"));
    spec.documented
        .iter()
        .find(|flag| flag.value.is_none() && flag.long == "json")
        .is_some_and(|json| args.iter().any(|arg| arg == &format!("--{}", json.long)))
}

/// The manual request the raw arguments make, validated before any write.
///
/// `Ok(Some(dir))` when `--markdown` names an `--out` directory; `Ok(None)`
/// when the arguments ask for one of the other help modes. The comparison is
/// against the literal `--markdown`/`--out`/`--out=` spellings the tests pin,
/// and the conflicts are refused here rather than at the writer: `--markdown`
/// excludes `--json` and a `TOPIC`, `--out` is required with `--markdown` and
/// refused without it, and each of the two flags is refused twice.
pub(crate) fn markdown_request(args: &[String]) -> Result<Option<PathBuf>, CliError> {
    let mut markdown = false;
    let mut out: Option<String> = None;
    let mut topic: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        // A lifted global's value is not a TOPIC: `main` appends it after the verb.
        if crate::cli_args::is_global_value_flag(arg)
            && args.get(index + 1).is_some_and(|v| !v.starts_with("--"))
        {
            index += 2;
            continue;
        }
        if arg == "--markdown" {
            crate::cli_error::reject_repeated_flag("--markdown", markdown)?;
            markdown = true;
        } else if arg == "--out" {
            crate::cli_error::reject_repeated_flag("--out", out.is_some())?;
            out = Some(out_value(args, index)?);
            index += 1;
        } else if let Some(value) = arg.strip_prefix("--out=") {
            crate::cli_error::reject_repeated_flag("--out", out.is_some())?;
            crate::cli_error::reject_blank_value("--out", value)?;
            out = Some(value.to_owned());
        } else if !arg.starts_with('-') {
            topic = topic.or(Some(arg));
        }
        index += 1;
    }
    if !markdown {
        if out.is_some() {
            return Err(CliError::usage_hint(
                "--out writes the manual, so it needs --markdown",
                "pass `--markdown --out DIR`, or drop --out",
            ));
        }
        return Ok(None);
    }
    if wants_json_catalog(args) {
        return Err(CliError::usage_hint(
            "`--json` and `--markdown` select different output modes",
            "print the catalog with `--json`, or write the manual with `--markdown --out DIR`",
        ));
    }
    if let Some(topic) = topic {
        return Err(CliError::usage_hint(
            format!("`--markdown` writes the whole manual and takes no topic (`{topic}`)"),
            "run `clio help --markdown --out DIR` for every page",
        ));
    }
    let Some(dir) = out else {
        return Err(CliError::usage_hint(
            "--markdown writes a manual and needs an output directory",
            "pass the directory to write into, as `--markdown --out DIR`",
        ));
    };
    Ok(Some(PathBuf::from(dir)))
}

/// The value token after a bare `--out`: refused when it is absent or starts
/// with `-`, so `--out -h` is a missing value, never a directory named `-h`;
/// `--out=DIR` keeps its token as typed. Blank values are refused too.
fn out_value(args: &[String], index: usize) -> Result<String, CliError> {
    match args.get(index + 1) {
        Some(value) if !value.starts_with('-') => {
            crate::cli_error::reject_blank_value("--out", value)?;
            Ok(value.clone())
        }
        _ => Err(CliError::usage_hint(
            "--out requires a directory value",
            "pass the directory after --out, or spell it `--out=DIR`",
        )),
    }
}

/// The unknown-command failure, with a suggestion when one is close.
pub(crate) fn unknown_command(other: &str) -> CliError {
    match cli_help::suggest_command(other) {
        Some(closest) => CliError::usage(format!("unknown command `{other}`"))
            .with_hint(format!("did you mean `clio {closest}`?")),
        None => CliError::usage(format!("unknown command `{other}`")).with_hint("try `clio help`"),
    }
}

/// Render a usage failure in whichever output mode the raw arguments select.
pub(crate) fn report_usage_error(error: &CliError, raw: &[String]) -> ExitCode {
    report_surface_usage_error(error, raw)
}

/// The topic `clio help <topic>` names, and the arguments to reach it with
/// `--help` appended.
///
/// `Ok(None)` is a plain `clio help`. A topic the CLI does not know is an error,
/// never a silent full catalog: the old behaviour made `clio help frobnicate`
/// indistinguishable from `clio help remember`.
///
/// A two-word topic such as `ops diagnose` becomes the verb plus its own
/// subcommand, so it reaches the same code `clio ops diagnose --help` reaches.
/// A global value flag's value is not the topic and not a second topic word.
pub(crate) fn help_topic(args: &[String]) -> Result<Option<(String, Vec<String>)>, CliError> {
    let words = crate::cli_args::word_positions(args);
    let Some(&position) = words.first() else {
        return Ok(None);
    };
    let verb = args[position].trim_start_matches("clio ").to_owned();
    if !cli_help::is_known_command(&verb) {
        let mut error = unknown_command(&verb);
        if words.len() > 1 {
            error = error.with_hint(format!(
                "`clio help` takes one topic; try `clio help {verb}`"
            ));
        }
        return Err(error);
    }
    let mut kept: Vec<String> = args[position + 1..].to_vec();
    kept.push("--help".to_owned());
    Ok(Some((verb, kept)))
}

/// One registry record's `--help` screen: its usage line, a blank line, then
/// its notes, then the same options block `help_for` appends, so the surface's
/// prose lives on the record beside its flags and every help path shows the
/// same flag data.
pub(crate) fn print_record_help(name: &str) -> ExitCode {
    let spec =
        crate::cli_command_registry::find(name).unwrap_or_else(|| panic!("`{name}` is registered"));
    let mut text = format!("{}\n\n{}", spec.usage_line(), spec.notes.join("\n\n"));
    let options = spec.options_help();
    if !options.is_empty() {
        text.push_str("\n\n");
        text.push_str(&options);
    }
    println!("{text}");
    ExitCode::SUCCESS
}

/// `clio version --help`: the version record's own usage line and notes.
pub(crate) fn print_version_help() -> ExitCode {
    print_record_help("version")
}

/// `clio health --help`: the health record's own usage line and notes.
pub(crate) fn print_health_help() -> ExitCode {
    print_record_help("health")
}

/// Read the reserved surfaces' flags: a name-to-value map that fails closed on
/// any flag the surface does not accept.
///
/// Each reserved surface declares the flags it takes (`known`), because these
/// surfaces own their own `--help` and their flags are not described by a shared
/// spec. A typo or an unsupported flag is a usage error (exit 2), the same class
/// the shared parser gives, rather than a value stored and silently ignored.
/// `--output`/`-o`, `--help` and `-h` are accepted everywhere.
///
/// The global flags are accepted on every surface too. `split_leading_globals`
/// lifts them out from before the verb and `merge_leading_globals` hands them
/// back with the surface's own arguments, so a surface that did not name them
/// would refuse the invocation the top level just reconstructed.
///
/// The reserved surfaces take no positional operand, so a stray token is refused
/// rather than dropped, the same way an unknown flag is. A bare `--` is the one
/// exception: it is the top level's delimiter before positional text, so the
/// token itself is accepted and what follows it is the refusal.
///
/// It is also strict about an empty value: `clio ops diagnose --db ''` used to
/// fall back to the default database without a word, which is a different
/// outcome from the refusal every other surface gives and left a caller with no
/// way to predict it. The rule itself is `cli_error::reject_blank_value`, so
/// both parsers share it.
pub(crate) fn parse_flags(
    args: impl IntoIterator<Item = String>,
    known: &[&str],
) -> Result<std::collections::HashMap<String, String>, CliError> {
    let mut flags = std::collections::HashMap::new();
    let mut pending: Option<String> = None;
    let mut it = args.into_iter();
    // `--flag=value` carries its own value. A bare `--flag` takes the next
    // token as its value only when that token is not itself a `--flag`;
    // otherwise `--flag` is value-less and the token is reprocessed. Without
    // this, `--bind --db X` would bind the value `--db` and `--bind=ADDR`
    // would be ignored, silently falling back to auto-bind.
    while let Some(flag) = pending.take().or_else(|| it.next()) {
        // A bare `--` is the top level's documented delimiter before positional
        // text. The delimiter itself is accepted; anything after it is refused,
        // because these surfaces have no operand slot to hand it to. This runs
        // before the `--` prefix strip, which also matches the bare token.
        if flag == "--" {
            if let Some(token) = it.next() {
                return Err(unexpected_argument(&token));
            }
            break;
        }
        // `-o` and `-o=…` are the short spelling of the global `--output` value
        // flag; `-h` is the short spelling of `--help`. Every other single-dash
        // token is an unknown flag and fails closed.
        let name_value = if let Some(name_value) = flag.strip_prefix("--") {
            name_value
        } else if let Some(value) = flag.strip_prefix("-o=") {
            crate::cli_error::reject_blank_value("-o", value)?;
            crate::cli_error::reject_repeated_flag("--output", flags.contains_key("output"))?;
            flags.insert("output".to_owned(), value.to_owned());
            continue;
        } else if flag == "-o" {
            "output"
        } else if flag == "-h" {
            flags.insert("help".to_owned(), "true".to_owned());
            continue;
        } else if flag.starts_with('-') {
            return Err(unknown_surface_flag(&flag, known));
        } else {
            return Err(unexpected_argument(&flag));
        };
        if let Some((name, value)) = name_value.split_once('=') {
            crate::cli_error::reject_blank_value(&format!("--{name}"), value)?;
            crate::cli_error::reject_repeated_flag(&format!("--{name}"), flags.contains_key(name))?;
            insert_known(&mut flags, name, value.to_owned(), known)?;
            continue;
        }
        let name = name_value;
        // `--help` is value-less, so it never consumes the token after it.
        if name == "help" {
            flags.insert("help".to_owned(), "true".to_owned());
            continue;
        }
        match it.next() {
            Some(value) if !value.starts_with("--") => {
                crate::cli_error::reject_blank_value(&format!("--{name}"), &value)?;
                crate::cli_error::reject_repeated_flag(
                    &format!("--{name}"),
                    flags.contains_key(name),
                )?;
                insert_known(&mut flags, name, value, known)?;
            }
            Some(value) => {
                insert_known(&mut flags, name, "true".into(), known)?;
                pending = Some(value);
            }
            None => {
                insert_known(&mut flags, name, "true".into(), known)?;
            }
        }
    }
    Ok(flags)
}

/// Record a flag, or fail closed when the surface does not accept it.
fn insert_known(
    flags: &mut std::collections::HashMap<String, String>,
    name: &str,
    value: String,
    known: &[&str],
) -> Result<(), CliError> {
    if accepted_by_surface(name, known) {
        flags.insert(name.to_owned(), value);
        Ok(())
    } else {
        Err(unknown_surface_flag(&format!("--{name}"), known))
    }
}

/// Whether a surface accepts `name`: the flags it declares, the global flags
/// every command takes before or after the verb, and `help`, which every surface
/// answers. `output` is one of the globals, so the shared output mode is accepted
/// everywhere by the same rule.
fn accepted_by_surface(name: &str, known: &[&str]) -> bool {
    name == "help"
        || known.contains(&name)
        || crate::cli_command_registry::global_value_names().any(|flag| flag == name)
        || crate::cli_command_registry::global_bool_names().any(|flag| flag == name)
}

/// The flags a suggestion may name: the surface's own, then the globals and
/// `help`, which it also accepts.
fn suggestion_candidates<'a>(known: &'a [&'a str]) -> Vec<&'a str> {
    let mut candidates: Vec<&'a str> = known.to_vec();
    for name in crate::cli_command_registry::global_value_names() {
        candidates.push(name);
    }
    for name in crate::cli_command_registry::global_bool_names() {
        candidates.push(name);
    }
    candidates.push("help");
    candidates
}

/// The reserved surfaces' unknown-flag failure, matching the shared parser:
/// an exit-2 usage error, with a near-miss suggestion when one flag is close.
fn unknown_surface_flag(flag: &str, known: &[&str]) -> CliError {
    let rendered = if flag.starts_with('-') {
        flag.to_owned()
    } else {
        format!("--{flag}")
    };
    match crate::cli_args::suggest(flag.trim_start_matches('-'), &suggestion_candidates(known)) {
        Some(closest) => CliError::usage(format!("unknown flag `{rendered}`"))
            .with_hint(format!("did you mean `--{closest}`?")),
        None => CliError::usage(format!("unknown flag `{rendered}`")),
    }
}

/// A token with no leading dash, which these surfaces have no operand slot for.
/// It is refused so a mistyped flag cannot arrive as a silent no-op.
fn unexpected_argument(token: &str) -> CliError {
    CliError::usage(format!("unexpected argument `{token}`"))
        .with_hint("these commands take flags only; run `clio help` for the list")
}

/// Report a usage failure from one of the reserved surfaces and return its exit
/// code, honouring the caller's `--output` so the envelope is JSON when asked
/// for and text otherwise.
///
/// Every surface needs this the moment its flag parsing can fail, and the mode
/// rule is the same one `report_usage_error` applies to the verbs.
pub(crate) fn report_surface_usage_error(error: &CliError, raw: &[String]) -> ExitCode {
    let is_tty = cli_output::stdout_is_tty();
    let mode = cli_output::mode_from_raw(raw, is_tty);
    cli_output::report_error(error, mode, is_tty, cli_output::no_color_requested())
}

/// The database default and precedence chain, credentials masked. Shared by
/// `clio help` and the subcommand `--help` surfaces; never opens the database.
pub(crate) fn database_help_block<F>(get: &F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    // Help must print even when the configured target cannot be opened, so the
    // reason is shown in place of the target rather than swallowed.
    let target = match crate::db_target::resolve_target(get, None, None) {
        Ok(resolved) => format!(
            "  default:    {}  (backend: {})",
            clio_ops::redact_credentials(&resolved.url),
            resolved.backend
        ),
        Err(reason) => format!("  default:    cannot be resolved: {reason}"),
    };
    format!(
        "Database (no flags or config file needed):\n{target}\n  \
         precedence: --db > the config file > CLIO_DATABASE_URL > CLIO_DATA_DIR > \
         XDG_DATA_HOME > HOME > ./clio.db\n  \
         env:        CLIO_DATA_DIR names the directory; the resolver appends clio.db\n  \
         empty:      a value flag given an empty value (--db '') is refused; leave the flag off \
         to take the default"
    )
}

pub(crate) fn help_text() -> String {
    let range = format!("{}-{}", port_scan::FIRST_PORT, port_scan::LAST_PORT);
    format!(
        "clio — {}\n\nUsage:\n{}",
        clio::identity(),
        command_catalog::catalog_text().replace("{range}", &range)
    )
}

pub(crate) fn print_help() {
    println!("{}", help_text());
    println!("\n{}", database_help_block(&|key| std::env::var(key).ok()));
}

#[cfg(test)]
#[path = "cli_top_level_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_top_level_markdown_tests.rs"]
mod markdown_tests;
