// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio compose` CLI surface: dispatching
//! `up` / `down`, resolving the compose root, ordering the precondition
//! checks, and mapping outcomes to process exit codes.
//!
//! ## Owns
//! - The `clio compose up|down` dispatcher and flag handling (`--dir`,
//!   `--services`)
//! - Compose-root resolution (CWD or `--dir`) with clobber protection
//! - The ordered `up` pipeline: detect target, probe Docker, materialize,
//!   verify bind mounts, generate `.env`, select services, invoke Docker
//! - The `down` pipeline with its selection resolution and idempotency
//!
//! ## Does not own
//! - Embedded-asset materialization and bind-mount checks (see `compose_assets`)
//! - `.env` and selection-marker files (see `compose_env`)
//! - Docker target detection, probe, and argument shapes (see `compose_docker`)
//!
//! ## Boundary
//!
//! Changes to the command surface, flag handling, or exit-code mapping should
//! normally require changes here. Changes to what gets materialized or which
//! Docker arguments run should not require changes here.
//!
//! Keep this module focused on the `clio compose` command surface.

use crate::cli_top_level::parse_flags;
use crate::cli_top_level::report_surface_usage_error;
use crate::compose_assets;
use crate::compose_docker::{ComposeTarget, DockerRunner, RealDocker, Stack, compose_args, probe};
use crate::compose_env;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The flags `clio compose` accepts. Any other flag is a usage error rather than
/// a silently ignored token.
const COMPOSE_FLAGS: &[&str] = &["dir", "services"];

/// Exit-code contract for `clio compose`:
/// - `0`: success (Docker reported success).
/// - `1`: a precondition failed (unsupported platform, Docker CLI/daemon not
///   reachable, compose root or materialization problem, missing bind-mount
///   source, unknown/missing service selection) or Docker exited non-zero.
/// - `2`: unknown or missing `clio compose` subcommand.
pub(crate) fn compose(args: impl IntoIterator<Item = String>) -> ExitCode {
    let raw: Vec<String> = args.into_iter().collect();
    let (sub, rest) = raw
        .split_first()
        .map_or(("", &[][..]), |(s, r)| (s.as_str(), r));
    let flags = match parse_flags(rest.to_vec(), COMPOSE_FLAGS) {
        Ok(flags) => flags,
        Err(error) => return report_surface_usage_error(&error, &raw),
    };
    if matches!(sub, "help" | "--help" | "-h") || flags.contains_key("help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let target = match ComposeTarget::detect(std::env::consts::OS, std::env::consts::ARCH) {
        Ok(target) => target,
        Err(e) => return fail(&e),
    };
    match sub {
        "up" => compose_up(&flags, &RealDocker, target, &read_prompt_line),
        "down" => compose_down(&flags, &RealDocker, target),
        "" => {
            eprintln!("missing compose subcommand; try `clio compose up|down`");
            ExitCode::from(2)
        }
        other => {
            eprintln!("unknown compose command `{other}`; try `clio compose up|down`");
            ExitCode::from(2)
        }
    }
}

/// Both usage lines come from the shared registry, so the second one cannot lose
/// its `clio` prefix the way a hand-written pair did: `am compose down` was a line
/// a reader could copy and a shell could not run.
fn print_help() {
    for (command, note) in [
        (
            "compose up",
            "Materialize the compose stack and start the chosen services. Services: \
             postgres (always on), embed, rerank, extract — or a profile name \
             (local-retrieval, local-rft). Defaults to a prompt.",
        ),
        (
            "compose down",
            "Stop and remove the environment started by `clio compose up`. Re-runnable; \
             Docker output and exit status are surfaced.",
        ),
    ] {
        println!(
            "{}\n{}",
            crate::cli_help_usage::usage_line(command).unwrap_or_else(|| command.to_owned()),
            note
        );
    }
}

/// One CLI failure shape: a clear message on stderr, exit 1.
fn fail(message: &str) -> ExitCode {
    eprintln!("clio compose: {message}");
    ExitCode::from(1)
}

/// Resolve the compose root: `--dir` value or the current working directory.
/// Created when missing; a non-directory path is refused (never clobbered).
pub(crate) fn resolve_root(dir: Option<&str>) -> Result<PathBuf, String> {
    resolve_root_with(dir, std::env::current_dir)
}

/// [`resolve_root`] with the process cwd lookup injected, so both the success
/// and the "cannot read cwd" failure paths are testable without mutating the
/// process working directory.
fn resolve_root_with(
    dir: Option<&str>,
    cwd: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Result<PathBuf, String> {
    let root = match dir {
        Some(dir) => PathBuf::from(dir),
        None => cwd().map_err(|e| format!("cannot read cwd: {e}"))?,
    };
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "{} exists and is not a directory; pass a directory to `--dir`",
            root.display()
        ));
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    Ok(root)
}

/// `clio compose up`: order matters — every failing step happens BEFORE files
/// are written, so unsupported targets and unreachable Docker leave no
/// partial materialization behind.
///
/// There is no credential to pass through. The managed Postgres has no password,
/// so Compose reads only the variables in `.env` and its own defaults.
pub(crate) fn compose_up(
    flags: &HashMap<String, String>,
    runner: &dyn DockerRunner,
    target: ComposeTarget,
    prompt: &dyn Fn() -> Option<String>,
) -> ExitCode {
    compose_up_with_env(flags, runner, target, prompt, &[])
}

/// [`compose_up`] with the child environment supplied by the caller, so a test
/// can point it at values it owns.
pub(crate) fn compose_up_with_env(
    flags: &HashMap<String, String>,
    runner: &dyn DockerRunner,
    target: ComposeTarget,
    prompt: &dyn Fn() -> Option<String>,
    env: &[(String, String)],
) -> ExitCode {
    if let Err(e) = probe(runner) {
        return fail(&e);
    }
    let root = match resolve_root(flags.get("dir").map(String::as_str)) {
        Ok(root) => root,
        Err(e) => return fail(&e),
    };
    let stack = match select_stack(flags, prompt) {
        Ok(stack) => stack,
        Err(e) => return fail(&e),
    };
    if let Err(e) = prepare_root(&root, target, stack) {
        return fail(&e);
    }
    let args = compose_args(target, stack, "up", &["-d"]);
    run_docker_env(runner, &args, &root, "up", env)
}

/// Warn when `extract` was chosen but its GGUF is not in `models/`; every other
/// service still starts.
fn warn_missing_gguf(root: &Path, stack: Stack) {
    if stack.needs_extract()
        && !compose_assets::gguf_path(root, compose_assets::GGUF_FILE_NAME).exists()
    {
        eprintln!(
            "warning: extract selected but {} is missing; place the GGUF in models/ \
             (see models/README.md). Other services start normally.",
            compose_assets::GGUF_FILE_NAME
        );
    }
}

/// `clio compose down`: stops and removes what `up` started. The selection
/// recorded by `up` (marker file) is reused so profile services are removed
/// too; `--services` overrides it. Re-running on a stopped stack succeeds.
pub(crate) fn compose_down(
    flags: &HashMap<String, String>,
    runner: &dyn DockerRunner,
    target: ComposeTarget,
) -> ExitCode {
    if let Err(e) = probe(runner) {
        return fail(&e);
    }
    let root = match resolve_root(flags.get("dir").map(String::as_str)) {
        Ok(root) => root,
        Err(e) => return fail(&e),
    };
    if !root.join("compose.yml").exists() {
        return fail(&format!(
            "no materialized compose.yml in {}; run `clio compose up` first",
            root.display()
        ));
    }
    let selection = flags
        .get("services")
        .map(String::as_str)
        .map(str::to_owned)
        .or_else(|| compose_env::read_marker(&root));
    let stack = match selection {
        Some(raw) => match Stack::parse_selection(&raw) {
            Ok(stack) => stack,
            Err(e) => return fail(&e),
        },
        None => {
            return fail(&format!(
                "no recorded selection in {} (and no --services given); pass `--services` or \
                 re-run `clio compose up`",
                root.display()
            ));
        }
    };
    let args = compose_args(target, stack, "down", &[]);
    run_docker(runner, &args, &root, "down")
}

/// Service selection: `--services` when given, otherwise an interactive
/// prompt. Menu answers (`1`/`2`/`3`), an empty answer (default:
/// `local-retrieval`), a profile name, or an explicit service list all map
/// onto the allowlist. Non-TTY stdin without `--services` fails closed.
fn select_stack(
    flags: &HashMap<String, String>,
    prompt: &dyn Fn() -> Option<String>,
) -> Result<Stack, String> {
    match flags.get("services").map(String::as_str) {
        Some(raw) => Stack::parse_selection(raw),
        None => match prompt() {
            Some(answer) => {
                let raw = match answer.trim() {
                    "" | "2" => "local-retrieval",
                    "1" => "postgres",
                    "3" => "local-rft",
                    other => other,
                };
                Stack::parse_selection(raw)
            }
            None => Err(
                "stdin is not interactive; pass `--services postgres|local-retrieval|local-rft` \
                 (or an explicit service list such as `--services postgres,embed,rerank`)"
                    .into(),
            ),
        },
    }
}

fn marker_value(stack: Stack) -> &'static str {
    match stack.profile() {
        Some(profile) => profile,
        None => "postgres",
    }
}

/// The service menu `clio compose up` shows before it reads an answer.
const SERVICE_MENU: &str = "Services to run: [1] postgres only, [2] postgres + embed + rerank \
     (local-retrieval, default), [3] + extract (local-rft)";

/// Read one line from stdin when it is a TTY. `None` means "not interactive";
/// the answer (menu digit, profile, or service list) is parsed by
/// `select_stack`.
fn read_prompt_line() -> Option<String> {
    prompt_tty(std::io::stdin().is_terminal(), &mut std::io::stdin().lock())
}

/// Show the service menu and read one answer from the given reader (only
/// reached when stdin is a TTY; `prompt_tty(false, ..)` is the
/// non-interactive path). The reader is injectable so tests never block on
/// the process stdin. The mechanics live in [`prompt_line`], the one place a
/// question is asked.
fn prompt_tty<R: std::io::BufRead>(is_tty: bool, input: &mut R) -> Option<String> {
    prompt_line(is_tty, SERVICE_MENU, &mut std::io::stdout(), input)
}

/// Ask one question on an interactive terminal: print `prompt` to `out`, then
/// read one line from `input`. `None` means "not interactive, or nothing was
/// readable", and no prompt is written when `is_tty` is false.
///
/// This is the one place that knows how to ask a question: the TTY gate, the
/// read-one-line mechanics, and the injected streams live here, and the reader
/// is injectable so tests never block on the process stdin. Commands with a
/// prompt of their own call this instead of adding a parallel mechanism.
pub(crate) fn prompt_line<R: std::io::BufRead + ?Sized>(
    is_tty: bool,
    prompt: &str,
    out: &mut dyn Write,
    input: &mut R,
) -> Option<String> {
    if !is_tty {
        return None;
    }
    let _ = writeln!(out, "{prompt}");
    read_answer(input)
}

/// Read one prompt answer from any reader. A closed or failed read (EOF,
/// interrupted) means "no answer". Split from [`prompt_line`] so the answer
/// mechanics are testable without a TTY.
pub(crate) fn read_answer<R: std::io::BufRead + ?Sized>(input: &mut R) -> Option<String> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// The effective embed model from the resolved config (`.env` is populated
/// from the effective config where applicable). Falls back to the documented
/// compose default when the config tree does not carry it.
fn effective_embed_model() -> String {
    clio_config::Runtime::new()
        .effective_raw()
        .get("embed")
        .and_then(|embed| embed.get("model"))
        .and_then(|model| model.as_str())
        .unwrap_or("BAAI/bge-small-en-v1.5")
        .to_string()
}

/// Materialize the checked-in files, write `.env` and the selection marker,
/// and warn about a missing GGUF. Shared by the standalone `up` and the
/// wizard's start call, so both write the same files in the same order.
///
/// # Errors
/// A materialization failure or a `.env`/marker write failure.
pub(crate) fn prepare_root(root: &Path, target: ComposeTarget, stack: Stack) -> Result<(), String> {
    materialize_and_check(root, target)?;
    compose_env::write_env(root, &compose_env::env_contents(&effective_embed_model()))?;
    compose_env::write_marker(root, marker_value(stack))?;
    warn_missing_gguf(root, stack);
    Ok(())
}

fn materialize_and_check(root: &Path, target: ComposeTarget) -> Result<(), String> {
    compose_assets::materialize(root, target)?;
    compose_assets::verify_bind_mounts(root)?;
    Ok(())
}

/// Run `docker compose …` with inherited stdio in the compose root and
/// propagate a non-zero Docker exit status.
fn run_docker(runner: &dyn DockerRunner, args: &[String], root: &Path, verb: &str) -> ExitCode {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match runner.run_inherited(&arg_refs, root) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => {
            eprintln!("docker compose {verb} exited with status {code}");
            ExitCode::from(propagate(code))
        }
        Err(e) => {
            eprintln!("docker compose {verb} failed to start: {e}");
            ExitCode::from(1)
        }
    }
}

/// [`run_docker`] with `env` set on the child process, for runs that carry a
/// secret such as the managed password.
fn run_docker_env(
    runner: &dyn DockerRunner,
    args: &[String],
    root: &Path,
    verb: &str,
    env: &[(String, String)],
) -> ExitCode {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match runner.run_inherited_env(&arg_refs, root, env) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => {
            eprintln!("docker compose {verb} exited with status {code}");
            ExitCode::from(propagate(code))
        }
        Err(e) => {
            eprintln!("docker compose {verb} failed to start: {e}");
            ExitCode::from(1)
        }
    }
}

/// Docker exit codes above 255 cannot map to a process exit code; clamp so a
/// failure is still reported as failure rather than wrapping to success.
fn propagate(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

#[cfg(test)]
#[path = "compose_cli_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "compose_cli_fail_tests.rs"]
mod fail_tests;
