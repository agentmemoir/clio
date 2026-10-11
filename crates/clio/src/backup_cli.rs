// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio backup` CLI surface: the backup
//! directory verbs that deliberately bind no MCP tool. `backup dir` shows and
//! creates the resolved backup directory without opening the database.
//!
//! ## Owns
//! - The `backup` group dispatch and its did-you-mean hint, both worked over
//!   the subcommand list the registry derives (see `cli_help_groups`)
//! - `backup dir`: directory creation, owner-only tightening of the fresh
//!   leaf, the writability probe, and its text and JSON views
//! - The restore selectors: the snapshot-id rule and the target resolution
//!
//! ## Does not own
//! - The backup-directory precedence chain (see `clio_config::backup_path`)
//! - Archive containers, manifests, and snapshots (see `clio-backup`)
//! - Prune selection, deletion, and views (see `clio-backup`'s `prune` half and
//!   `backup_prune_cli`)
//! - The output/error contracts (see `cli_read`) or flag parsing (`cli_args`)
//!
//! ## Boundary
//!
//! Changes to a `backup` verb's flags, usage text, or summary should normally
//! require changes in `cli_command_registry_backup`; changes to its dispatch,
//! decoding, or views should normally require changes here. Changes to
//! directory resolution should not require changes here.
//!
//! Keep this module focused on the `clio backup` surface.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK, EXIT_USAGE};
use crate::cli_help;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_backup::SnapshotRow;
use clio_config::ResolvedBackupDir;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Dispatch `clio backup <subcommand>` with the process environment, streams,
/// and current directory.
pub(crate) fn backup(args: Vec<String>) -> ExitCode {
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
    // The resolver needs a current directory to absolute-ize a relative
    // fallback; failing to learn one is operational, not a usage error.
    let current_dir = match std::env::current_dir() {
        Ok(dir) => Ok(dir),
        Err(error) => Err(current_dir_error(&error)),
    };
    run(&mut io, args, current_dir)
}

/// The operational error for a current-directory lookup that failed.
fn current_dir_error(error: &std::io::Error) -> CliError {
    CliError::operational(
        "internal",
        format!("cannot determine the current directory: {error}"),
    )
}

/// Resolve, parse, and run one `backup` subcommand.
///
/// `current_dir` is the injected seam: the real entry resolves the process
/// current directory, tests inject one (or an error) directly.
pub(crate) fn run(
    io: &mut CliIo,
    args: Vec<String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(sub) = tokens.next() else {
        let _ = cli_output::write_text(&mut io.out, &group_help_text());
        return ExitCode::from(EXIT_USAGE);
    };
    if sub == "--help" || sub == "-h" || sub == "help" {
        let _ = cli_output::write_text(&mut io.out, &group_help_text());
        return ExitCode::from(EXIT_OK);
    }
    match sub.as_str() {
        "create" => crate::backup_create_cli::run(io, fallback, tokens, current_dir),
        "dir" => run_dir(io, fallback, tokens, current_dir),
        "list" => crate::backup_list_cli::run(io, fallback, tokens, current_dir),
        "prune" => crate::backup_prune_cli::run(io, fallback, tokens, current_dir),
        "restore" => crate::backup_restore_cli::run(io, fallback, tokens, current_dir),
        "verify" => crate::backup_verify_cli::run(io, fallback, tokens, current_dir),
        other => fail(io, fallback, &unknown_subcommand(other)),
    }
}

/// The `clio backup --help` body: a usage line plus the subcommand list.
fn group_help_text() -> String {
    let subs = cli_help::group_subcommands("backup").unwrap_or(&[]);
    cli_help::group_help("backup", subs)
}

/// Run `backup dir`: resolve, create when missing, probe, and emit.
fn run_dir(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: impl IntoIterator<Item = String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    let current_dir = match current_dir {
        Ok(dir) => dir,
        Err(error) => return fail(io, fallback, &error),
    };
    let parsed = match cli_args::parse(&dir_spec(), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if parsed.has("help") {
        let usage = crate::cli_help_usage::usage_for("backup dir")
            .unwrap_or_else(|| "usage: clio backup dir".to_owned());
        let _ = cli_output::write_text(&mut io.out, &usage);
        return ExitCode::from(EXIT_OK);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = crate::runtime_cli::no_positionals(&parsed) {
        return fail(io, fallback, &error);
    }
    let get = |key: &str| (io.env)(key);
    let resolved = resolve_backup_dir(&get, &current_dir);
    let created = match ensure_backup_dir(&resolved.path) {
        Ok(created) => created,
        Err(error) => return fail(io, mode, &error),
    };
    let writable = probe_writable(&resolved.path);
    let payload = json!({
        "ok": true,
        "backup_dir": resolved.path.display().to_string(),
        "tier": resolved.tier.as_str(),
        "created": created,
        "writable": writable,
    });
    emit(
        io,
        mode,
        &payload,
        &render_dir(&resolved, created, writable),
    );
    ExitCode::from(EXIT_OK)
}

/// The flags `backup dir` accepts: the `backup dir` record's own flags — none
/// beyond `--output` — decoded through the registry, plus the globals every
/// parser spec merges (so `--output` and its `-o` alias are accepted).
fn dir_spec() -> FlagSpec {
    match crate::cli_command_registry::find("backup dir") {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Create the backup directory when missing, then tighten the fresh leaf to
/// owner-only. An existing directory is left untouched — the operator may
/// have widened it deliberately — and reports `created: false`.
///
/// The rule itself lives in the archive half, so `create` and `dir` cannot
/// drift apart.
pub(crate) fn ensure_backup_dir(path: &Path) -> Result<bool, CliError> {
    clio_backup::staging::ensure_backup_dir(path)
        .map_err(|error| CliError::operational(error.code.as_str(), error.message))
}

/// Resolve the backup directory the `backup` verbs share.
///
/// Pure resolution: it never creates the directory and never fails, so the
/// callers that create it (every verb but `list`) stay in charge of that.
pub(crate) fn resolve_backup_dir(
    get: &impl Fn(&str) -> Option<String>,
    current_dir: &Path,
) -> ResolvedBackupDir {
    clio_config::resolve_backup_dir(get, current_dir)
}

/// One resolved snapshot target: the store to read, its backend, and the
/// key store that travels with it.
pub(crate) struct ResolvedTarget {
    /// The resolved database URL, which for SQLite is the file path.
    pub url: String,
    /// `sqlite` or `postgres`; the backend this build snapshots.
    pub backend: String,
    /// Credential-free file name (SQLite) or database name (Postgres) recorded
    /// in the manifest.
    pub database: String,
    /// What the reports show for this target: the store file's path for
    /// SQLite, the database name for Postgres. Never a URL with credentials.
    pub display: String,
    /// The store file to read, for SQLite; empty for Postgres, whose store is
    /// not a file.
    pub path: PathBuf,
    /// The key store to carry, when one exists for this target.
    pub kms_path: Option<PathBuf>,
}

impl ResolvedTarget {
    /// Whether this target is a Postgres database rather than a SQLite file.
    #[must_use]
    pub(crate) fn is_postgres(&self) -> bool {
        self.backend == "postgres"
    }
}

/// Resolve the store a snapshot command reads, refusing what it cannot read.
///
/// `--db` and `--backend` behave as they do elsewhere. A store this build does
/// not snapshot (an unknown backend) and an in-memory store are both refused
/// here, before any work: a refusal that named the file it wanted instead of
/// reading the wrong thing is the whole point. Both backends are first-class:
/// a SQLite target resolves to its file, a Postgres target to its database.
pub(crate) fn resolve_target(
    get: &impl Fn(&str) -> Option<String>,
    parsed: &ParsedArgs,
    command: &str,
) -> Result<ResolvedTarget, CliError> {
    resolve_store_target(get, parsed, command, true)
}

/// Resolve the store a restore writes.
///
/// Same rules as [`resolve_target`] except that a missing SQLite file is
/// allowed: a restore may place the first store at a path that does not exist
/// yet, and the target measurement reports the absence instead of guessing. A
/// Postgres target must be reachable either way, so this rule is only a
/// SQLite file rule.
pub(crate) fn resolve_restore_target(
    get: &impl Fn(&str) -> Option<String>,
    parsed: &ParsedArgs,
    command: &str,
) -> Result<ResolvedTarget, CliError> {
    resolve_store_target(get, parsed, command, false)
}

/// The shared store resolution, with the missing-file rule as the parameter.
fn resolve_store_target(
    get: &impl Fn(&str) -> Option<String>,
    parsed: &ParsedArgs,
    command: &str,
    must_exist: bool,
) -> Result<ResolvedTarget, CliError> {
    let resolved =
        crate::db_target::resolve_target(get, parsed.value("db"), parsed.value("backend"))
            .map_err(CliError::usage)?;
    if resolved.backend != "sqlite" && resolved.backend != "postgres" {
        return Err(unsupported_backend(&resolved.backend, command));
    }
    if resolved.backend == "postgres" {
        // The database name comes from the URL's path only, so a credential
        // never reaches a report, a member name, or the manifest.
        let database = clio_config::backup_path::store_slug(&resolved.url, "postgres");
        let kms_path = clio_config::kms_path::resolve_kms_path(get, &resolved.url);
        return Ok(ResolvedTarget {
            display: database.clone(),
            database,
            kms_path,
            url: resolved.url,
            backend: resolved.backend,
            path: PathBuf::new(),
        });
    }
    if resolved.url == "sqlite::memory:" {
        return Err(CliError::usage(format!(
            "`clio backup {command}` needs a store file; `sqlite::memory:` keeps the database in \
             this process and there is nothing to read"
        ))
        .with_hint("point `--db` at a SQLite database file"));
    }
    let path = PathBuf::from(&resolved.url);
    if must_exist && !path.is_file() {
        return Err(CliError::operational(
            "not_found",
            format!("no store at {} to snapshot", path.display()),
        )
        .with_hint("run `clio remember` or `clio setup` first, or point `--db` at the store"));
    }
    Ok(ResolvedTarget {
        database: path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        kms_path: clio_config::kms_path::resolve_kms_path(get, &resolved.url),
        display: path.display().to_string(),
        url: resolved.url,
        backend: resolved.backend,
        path,
    })
}

/// The snapshot to restore: the positional id, or the newest row of the
/// listing. A missing id is reported with the closest ids suggested.
///
/// # Errors
/// No id was given, `--latest` finds an empty backup directory, or the id is
/// not in the listing. The first case is usage; the other two are not-found.
pub(crate) fn resolve_restore_id(
    parsed: &ParsedArgs,
    rows: &[SnapshotRow],
) -> Result<String, CliError> {
    if parsed.has("latest") {
        return rows.first().map(|row| row.id.clone()).ok_or_else(|| {
            CliError::operational(
                "not_found",
                "there is nothing to restore: the backup directory is empty",
            )
            .with_hint("take one with `clio backup create`")
        });
    }
    let id = parsed.positional(0).unwrap_or_default();
    if rows.iter().any(|row| row.id == id) {
        return Ok(id.to_owned());
    }
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let hint = cli_args::suggest(id, &ids).map_or_else(
        || "run `clio backup list` to see the snapshots".to_owned(),
        |best| format!("did you mean `{best}`?"),
    );
    Err(CliError::operational(
        "not_found",
        format!("no snapshot `{id}` in the backup directory"),
    )
    .with_hint(hint))
}

/// The refusal for a backend this build does not snapshot.
///
/// The message says what the command reads and what would change the outcome,
/// and names nothing internal: an operator needs the flag or the backend, not
/// this project's planning documents.
fn unsupported_backend(backend: &str, command: &str) -> CliError {
    CliError::usage(format!(
        "`clio backup {command}` does not support the {backend} backend"
    ))
    .with_hint(format!(
        "backend must be sqlite or postgres, got `{backend}`"
    ))
}

/// Probe writability by creating and removing a uniquely named empty file
/// inside the directory. Informational: an unwritable but existing directory
/// still exits 0.
fn probe_writable(path: &Path) -> bool {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let probe = path.join(format!(
        ".clio-backup-dir-probe-{}-{nanos}",
        std::process::id()
    ));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Human text for `backup dir`: one line with the path, tier, and state.
fn render_dir(resolved: &ResolvedBackupDir, created: bool, writable: bool) -> String {
    let state = if created { "created" } else { "existed" };
    let writable = if writable { "writable" } else { "not writable" };
    format!(
        "backup dir {} ({}; {state}; {writable})",
        resolved.path.display(),
        resolved.tier.as_str()
    )
}

/// The error for an unknown subcommand, suggesting the closest verb.
///
/// The candidates are the derived subcommand list the group help body prints,
/// so the two surfaces cannot drift: a verb added to the registry is both
/// listed and suggested, with no second list to update. The suggestion keeps
/// the earlier candidate on a tie, which cannot change an answer here because
/// the six labels share no first letter.
fn unknown_subcommand(sub: &str) -> CliError {
    let candidates = cli_help::group_subcommands("backup").unwrap_or(&[]);
    let hint = cli_args::suggest(sub, candidates).map_or_else(
        || "try `clio backup dir`".to_owned(),
        |best| format!("did you mean `clio backup {best}`?"),
    );
    CliError::usage(format!("unknown `backup` subcommand `{sub}`")).with_hint(hint)
}

#[cfg(test)]
#[path = "backup_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "backup_cli_help_words_tests.rs"]
mod backup_cli_help_words_tests;
