// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio setup` command surface: wiring the
//! real streams, deciding interactive versus flag-driven, running the collect →
//! start → test → preview → apply order, and mapping outcomes to exit codes.
//!
//! ## Owns
//! - The `setup` entrypoint and the injectable `setup_core`
//! - `run`, the one ordering every apply path follows
//! - The preflight that refuses a contradictory flag set before a prompt
//! - Usage vs. failure exit-code mapping and the TTY gate
//!
//! ## Does not own
//! - The flag grammar (see `setup_flags`)
//! - The questions, preview, and switch gate (see `setup_steps`/`setup_review`)
//! - Starting Docker, testing, and writing the config (see `setup_wizard`)
//!
//! ## Boundary
//!
//! Changes to how a run is ordered, or to which exit code an outcome reaches,
//! should normally require changes here. Changes to a flag's shape should not.
//!
//! Keep this module focused on the command surface and the run's order.
//!
//! # Why the runner is injected
//!
//! `run` starts and stops containers, so it needs a `DockerRunner`. The real
//! binary passes [`RealDocker`]; a test passes a stub that records every call
//! and decides its own exit code, so the apply path is exercised without Docker
//! and without starting anything on the machine running the suite.
//!
//! # Why nothing is written before the preview
//!
//! One write, after the operator has seen the preview and cleared the switch
//! gate. Every failure before it stops what this run started and leaves the old
//! config exactly as it was.

use crate::cli_style::Palette;
use crate::compose_docker::{DockerRunner, RealDocker};
use crate::setup_cli_flows::Ctx;
use crate::setup_defaults::Defaults;
use crate::setup_extract_model::{ModelFetcher, RealFetcher};
use crate::setup_flags::{parse_setup_args, print_help};
use crate::setup_review::Verdict;
use crate::setup_test_seam::Probes;
use clio_config::{Runtime, resolve_deployment_path};
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// A CLI failure carrying the process exit code: 2 for usage, 1 for a failed step.
///
/// Code 0 with an empty message is the one "failure" that is not one: `b` at the
/// first question and `q` at any question are a clean exit, and a clean exit must
/// not print `clio setup:` with nothing after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliError {
    pub(crate) code: u8,
    pub(crate) message: String,
}

impl CliError {
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    pub(crate) fn failed(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }

    /// A clean exit, with nothing to report.
    pub(crate) fn clean() -> Self {
        Self {
            code: 0,
            message: String::new(),
        }
    }
}

/// `clio setup` entrypoint: wire the real stdin/stdout/stderr and Docker.
pub(crate) fn setup(args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let is_tty = std::io::stdin().is_terminal();
    let probes = crate::setup_probes::real_probes();
    setup_core(
        args,
        &mut stdin,
        &mut stdout,
        &mut stderr,
        is_tty,
        &real_env,
        &RealDocker,
        &RealFetcher,
        &probes,
    )
}

/// The operator's own environment, which is where a `CLIO_DATABASE_URL` comes from.
///
/// A variable exported in the shell is a real way to point Clio at a database, so
/// the wizard offers it as the address already in use rather than ignoring it. The
/// seam exists so a test can hand the wizard an environment of its own: a suite
/// that inherits whatever the developer exported would otherwise assert different
/// things on different machines, and a test that reads process state it does not
/// control is a test whose result is not a fact about the code.
fn real_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Testable `clio setup` core over injected streams, TTY state, environment, and
/// Docker runner.
///
/// `env` is injected for the same reason the streams are: the wizard offers the
/// database the environment already names, so a test that inherited the machine's
/// `CLIO_DATABASE_URL` would assert one thing on a developer laptop and another in CI.
#[allow(
    clippy::too_many_arguments,
    reason = "one argument per thing the caller injects"
)]
pub(crate) fn setup_core<R: BufRead, W: Write, E: Write>(
    args: impl IntoIterator<Item = String>,
    reader: &mut R,
    out: &mut W,
    err: &mut E,
    is_tty: bool,
    env: &dyn Fn(&str) -> Option<String>,
    runner: &dyn DockerRunner,
    fetcher: &dyn ModelFetcher,
    probes: &Probes,
) -> ExitCode {
    let raw: Vec<String> = args.into_iter().collect();
    if is_help(&raw) {
        print_help(out);
        return ExitCode::SUCCESS;
    }
    let flags = match parse_setup_args(&raw) {
        Ok(flags) => flags,
        Err(message) => return fail(err, &CliError::usage(message)),
    };
    match run(
        &flags, reader, out, err, is_tty, env, runner, fetcher, probes,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(err, &e),
    }
}

fn is_help(raw: &[String]) -> bool {
    crate::cli_help_request::help_requested(raw, &crate::setup_flags::takes_value)
}

fn fail(err: &mut impl Write, e: &CliError) -> ExitCode {
    if e.message.is_empty() {
        return ExitCode::SUCCESS;
    }
    let _ = writeln!(err, "clio setup: {}", e.message);
    ExitCode::from(e.code)
}

/// Collect every answer, then run the start → test → preview → apply loop.
///
/// The order is fixed and is the plan's: nothing is started until the answers are
/// complete, nothing is written until the preview is accepted, and every path out
/// of the loop after a start stops exactly what this run started.
#[allow(
    clippy::too_many_arguments,
    reason = "one argument per thing the caller injects"
)]
fn run<R: BufRead, W: Write, E: Write>(
    flags: &HashMap<String, String>,
    reader: &mut R,
    out: &mut W,
    err: &mut E,
    is_tty: bool,
    env: &dyn Fn(&str) -> Option<String>,
    runner: &dyn DockerRunner,
    fetcher: &dyn ModelFetcher,
    probes: &Probes,
) -> Result<(), CliError> {
    let interactive = interactive_mode(flags, is_tty)?;
    let Some(config_path) = resolve_deployment_path(flags.get("config").map(PathBuf::from)) else {
        return Err(CliError::usage(
            "cannot resolve a config directory; set XDG_CONFIG_HOME or HOME, or pass --config",
        ));
    };
    let mut runtime = Runtime::with_env_and_deployment_file(env, &config_path);
    let effective = runtime.effective_raw();
    let defaults = Defaults::from_effective(&effective);
    // Asked once, before the first question, so a menu can say whether its Docker
    // options work on this machine. A run that only needs a SQLite file pays two
    // `docker` invocations for the privilege of never offering a choice that
    // would fail, which is cheaper than the failure.
    let docker = crate::compose_docker::status(runner);
    let mut ctx = Ctx::new(
        flags,
        reader,
        out,
        err,
        interactive,
        Palette::new(false),
        probes,
        defaults,
        // The same resolver every CLI command uses for its store, so the path the
        // wizard offers as a SQLite default is the path the store would open.
        clio_config::db_path::default_sqlite_path(&|key| env(key)),
        docker,
        env,
        // The wizard only asks anything when a terminal is attached, and this is
        // the same fact, so the prompts and the questions are driven by one
        // value rather than two that could disagree.
        crate::setup_prompt::terminal_available(),
    );
    let answers = crate::setup_steps::drive(&mut ctx, preflight).map_err(signal_error)?;
    let compose_dir = flags.get("compose-dir").map(String::as_str);
    // Nothing is started, stopped, or probed until the operator has agreed. The
    // answers, the preview, and the gate all run against files only, so quitting
    // at the gate leaves a machine that already had a working stack untouched.
    let will_back_up = config_path
        .exists()
        .then(|| config_path.display().to_string());
    let (backend, url) = match &answers.database {
        Some(database) => (database.plan.backend_name(), database.plan.address.as_str()),
        // `drive` finishes only with a database answer; an empty pair makes
        // the gate compare nothing rather than guess a store.
        None => ("", ""),
    };
    let store = crate::setup_store_state::current(env, &config_path, backend, url);
    let (answers, verdict) = crate::setup_review::review(
        &mut ctx,
        answers,
        &|path: &str| runtime.deployment_field(path).cloned(),
        will_back_up.as_deref(),
        compose_dir,
        store,
    )
    .map_err(signal_error)?;
    if verdict == Verdict::Quit {
        return Ok(());
    }
    // The extract model is put on the host before anything is written: a refused
    // download then leaves no config naming a stack whose model is absent, and no
    // `Applied` line for a run that did not apply.
    let writer = ctx.writer();
    crate::setup_extract_model::ensure_present(writer, &answers, runner, compose_dir, fetcher)
        .map_err(CliError::failed)?;
    // Every remaining line goes through `ctx`, because `ctx` holds the only
    // mutable borrow of the output stream. The write happens first, so the
    // summary and the checks are all reporting on a config that is already there.
    let writer = ctx.writer();
    crate::setup_wizard::apply(&answers, &mut runtime, &config_path, writer, env)
        .map_err(CliError::failed)?;
    let writer = ctx.writer();
    crate::setup_summary::stale_env_note(compose_dir, writer).map_err(CliError::failed)?;
    // The config is written and the operator has agreed. Only now does anything
    // run, and nothing is stopped afterwards: a failure here leaves a config that
    // names containers which are still up, which is the state an operator can act
    // on. See `setup_verify` for why the order is this way round.
    crate::setup_verify::start_and_check(&mut ctx, &answers, runner, compose_dir)
        .map_err(CliError::failed)
}

/// Refuse a contradictory flag set before the first question.
///
/// A contradiction must fail before a single prompt has been shown, or an
/// operator answers ten questions and is then told the flags they passed in
/// contradicted each other.
///
/// # Errors
/// Two flags cannot both hold.
fn preflight(ctx: &Ctx<'_>) -> Result<(), String> {
    // `--embed-dims` describes a hosted endpoint's dimension count, and a local
    // sidecar reports its own, so the pair is a contradiction rather than a value
    // that would be ignored.
    if ctx.has_flag("embed-dims") {
        match ctx.flag("embed") {
            Some(role)
                if !role.eq_ignore_ascii_case("online") && !role.eq_ignore_ascii_case("hosted") =>
            {
                return Err(format!(
                    "--embed-dims applies only to a hosted embed service, but --embed is `{role}`"
                ));
            }
            _ => {}
        }
        if let Ok(dims) = ctx.flag("embed-dims").unwrap_or_default().parse::<u64>() {
            if dims == 0 {
                return Err("--embed-dims must be a whole number above zero".to_owned());
            }
        } else {
            return Err(format!(
                "--embed-dims must be a whole number, got `{}`",
                ctx.flag("embed-dims").unwrap_or_default()
            ));
        }
    }
    // A local rerank needs no endpoint and a hosted one needs all three, so a key
    // beside a local rerank is a contradiction rather than a leftover.
    if ctx.has_flag("rerank-key") && ctx.flag("rerank") == Some("local") {
        return Err("--rerank-key cannot be given with a local rerank".to_owned());
    }
    Ok(())
}

/// Turn a flow signal into the CLI's error type.
///
/// `b` at the first question and `q` at any question are a clean exit, not a
/// failure, so they carry exit code 0 and no message. Anything else is a refusal
/// the operator has to act on.
fn signal_error(signal: crate::setup_render::Signal) -> CliError {
    match signal {
        crate::setup_render::Signal::Back | crate::setup_render::Signal::Quit => CliError::clean(),
        crate::setup_render::Signal::Usage(reason) => CliError::usage(reason),
        crate::setup_render::Signal::Refused(reason) => CliError::failed(reason),
    }
}

/// Decide whether to prompt. `--non-interactive` and `--yes` force off; an
/// explicit `--interactive` requires a TTY; otherwise auto-detect stdin.
fn interactive_mode(flags: &HashMap<String, String>, is_tty: bool) -> Result<bool, CliError> {
    if flags.contains_key("non-interactive") || flags.contains_key("yes") {
        return Ok(false);
    }
    if flags.contains_key("interactive") {
        if !is_tty {
            return Err(CliError::usage(
                "--interactive requires a terminal; pass the flags instead",
            ));
        }
        return Ok(true);
    }
    Ok(is_tty)
}

#[cfg(test)]
#[path = "setup_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "setup_cli_fail_tests.rs"]
mod fail_tests;
