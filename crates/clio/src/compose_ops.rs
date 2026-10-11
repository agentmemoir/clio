// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Compose lifecycle operations the
//! `clio setup` apply path calls: start, stop, and the volume-removing recreate
//! path.
//!
//! ## Owns
//! - Start/stop/recreate calls into the fixed `docker compose` shapes
//! - Mapping a non-zero Docker exit to a readable error for the wizard
//!
//! ## Does not own
//! - The standalone `clio compose up|down` command surface (see `compose_cli`)
//! - Docker argument construction (see `compose_docker`)
//! - The `.env`/marker content (see `compose_env`)
//!
//! ## Boundary
//!
//! Changes to what the apply path starts, stops, or recreates should normally
//! require changes here. Changes to the standalone command surface should not.
//!
//! Keep this module focused on the apply path's Compose calls.

use crate::compose_cli::{prepare_root, resolve_root};
use crate::compose_docker::{ComposeTarget, DockerRunner, Stack, compose_args, probe};
use std::path::Path;

/// How long `up` waits for every started service to report healthy.
///
/// The sidecars load a model before they answer, and the compose healthchecks
/// already allow 120 seconds for the embed sidecar and 180 for rerank. This is
/// above both plus their retry budget, so Compose stops waiting because a service
/// is healthy rather than because the clock ran out. Without a ceiling a broken
/// sidecar would leave the operator watching a spinner indefinitely.
pub(crate) const HEALTH_WAIT_SECONDS: &str = "600";

/// Start exactly `services` for the wizard apply path, materializing the
/// checked-in files and writing `.env` and the marker first.
///
/// An empty list starts nothing and succeeds without touching Docker (the
/// SQLite and hosted-models cases). Otherwise the exact allowlisted names are
/// passed after `up -d`; `env` carries the chosen port to the child process, so
/// Compose publishes the same port the stored address names.
///
/// `up` waits for the healthchecks. `docker compose up -d` returns as soon as
/// the containers have *started*, which for a model sidecar is several seconds
/// before it answers anything — so without the wait the health probe that
/// follows finds a refused socket on a container that is loading its model, and
/// reports a failure for a service that is working exactly as intended.
///
/// # Errors
/// An unknown service name, an unreachable Docker CLI/daemon, a
/// materialization or file-write failure, a missing model file for the
/// `extract` service, a service that never became healthy, or a non-zero
/// Compose exit.
#[allow(dead_code, reason = "the stage 6 apply path is its first caller")]
pub(crate) fn start_services(
    runner: &dyn DockerRunner,
    target: ComposeTarget,
    dir: Option<&str>,
    services: &[&str],
    env: &[(String, String)],
) -> Result<(), String> {
    let stack = Stack::from_services(services)?;
    if stack.services().is_empty() {
        return Ok(());
    }
    probe(runner)?;
    let root = resolve_root(dir)?;
    prepare_root(&root, target, stack, runner)?;
    let args = compose_args(
        target,
        stack,
        "up",
        &["-d", "--wait", "--wait-timeout", HEALTH_WAIT_SECONDS],
    );
    run_wizard(runner, &args, &root, "up", env)
}

/// Stop exactly the services a run started. Never `down`, never a
/// volume-removing argument, so a stopped stack can be started again with its
/// data intact. An empty list is a no-op.
///
/// # Errors
/// An unknown service name, an unresolvable directory, or a non-zero Compose
/// exit.
#[allow(dead_code, reason = "the stage 6 apply path is its first caller")]
pub(crate) fn stop_services(
    runner: &dyn DockerRunner,
    target: ComposeTarget,
    dir: Option<&str>,
    services: &[&str],
) -> Result<(), String> {
    let stack = Stack::from_services(services)?;
    if stack.services().is_empty() {
        return Ok(());
    }
    let root = resolve_root(dir)?;
    let args = compose_args(target, stack, "stop", &[]);
    run_wizard(runner, &args, &root, "stop", &[])
}

/// The recreate path: `docker compose down -v`, the one invocation allowed to
/// remove the data volume.
///
/// No wizard path calls this. `clio setup` never offers to delete a volume,
/// because it is asked for on a first install, where it is the loss of every
/// memory in it. This stays as the one place the shape exists, for an operator
/// who has decided to delete the volume on purpose.
///
/// # Errors
/// An unknown service name, an unresolvable directory, or a non-zero Compose
/// exit.
#[allow(
    dead_code,
    reason = "no wizard path calls it after the recreate path was removed"
)]
pub(crate) fn recreate_volume(
    runner: &dyn DockerRunner,
    target: ComposeTarget,
    dir: Option<&str>,
    services: &[&str],
) -> Result<(), String> {
    let stack = Stack::from_services(services)?;
    if stack.services().is_empty() {
        return Ok(());
    }
    let root = resolve_root(dir)?;
    let args = compose_args(target, stack, "down", &["-v"]);
    run_wizard(runner, &args, &root, "down", &[])
}

/// Run one fixed `docker compose` invocation for the wizard and map a non-zero
/// exit to a readable error.
#[allow(dead_code, reason = "the stage 6 apply path is its first caller")]
fn run_wizard(
    runner: &dyn DockerRunner,
    args: &[String],
    root: &Path,
    verb: &str,
    env: &[(String, String)],
) -> Result<(), String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match runner.run_inherited_env(&refs, root, env) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("docker compose {verb} exited with status {code}")),
        Err(e) => Err(format!("docker compose {verb} failed to start: {e}")),
    }
}

#[cfg(test)]
#[path = "compose_ops_tests.rs"]
mod tests;
