// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the part of `clio setup` that happens after
//! the operator has agreed: starting the containers the answers need, checking
//! that they answer, and reporting what happened.
//!
//! ## Owns
//! - `start_and_check`, the one place the wizard touches Docker
//! - The per-service report the operator reads after the apply
//!
//! ## Does not own
//! - The answers and the preview (see `setup_steps` and `setup_review`)
//! - The config write, which has already happened (see `setup_wizard`)
//! - Compose argument construction and the `.env` file (see `compose_ops`)
//!
//! ## Boundary
//!
//! Changes to what runs after the apply should normally require changes here.
//! Changes to a question, a preview line, or the config write should not.
//!
//! Keep this module focused on starting containers and checking them.
//!
//! # Why nothing runs before the apply
//!
//! The wizard used to start the containers, test them, show the preview, and stop
//! them again when the operator quit. That made `clio setup` destructive to a
//! machine that already had a working stack: running it twice and quitting the
//! second time left the first run's containers stopped, and the operator had to
//! start them again by hand.
//!
//! So the order is now: answers, preview, the gate, the write, and only then
//! Docker. Quitting at the gate touches nothing at all.
//!
//! # Why a failure after the apply leaves the containers running
//!
//! The config has been written and names these containers, so stopping them would
//! leave a config pointing at a service nothing is running. The operator is told
//! what failed and which container to look at, and the containers stay up: they
//! are the evidence, and `docker logs` is the next command they need.

use crate::compose_docker::{ComposeTarget, DockerRunner};
use crate::setup_cli_flows::Ctx;
use crate::setup_services::ServiceRole;
use crate::setup_steps::Answers;

/// Start the containers the answers need and check that they answer.
///
/// Every error this returns happens after the config has been written, so every
/// message says so. There is no repair step: the managed database has no
/// password, so a failed connection test is reported rather than second
/// attempt.
///
/// # Errors
/// The platform is unsupported, Docker refused to start the stack, or a service
/// did not answer. The containers are left running.
pub(crate) fn start_and_check(
    ctx: &mut Ctx<'_>,
    answers: &Answers,
    runner: &dyn DockerRunner,
    dir: Option<&str>,
) -> Result<(), String> {
    let services = crate::setup_preview::local_services(answers);
    if services.is_empty() {
        return Ok(());
    }
    let target = ComposeTarget::detect(std::env::consts::OS, std::env::consts::ARCH)?;
    let env = crate::setup_wizard::managed_env(answers);
    crate::compose_ops::start_services(runner, target, dir, &services, &env).map_err(|reason| {
        format!(
            "{reason}. Your settings were saved, so re-running `clio setup` picks up from here; \
             the containers are not running, and `docker compose ps` in the project directory \
             shows what state they are in."
        )
    })?;
    check_database(ctx, answers)?;
    for (what, service) in [
        ("embedding", &answers.embed),
        ("reranking", &answers.rerank),
    ] {
        let Some(service) = service else { continue };
        if service.role != ServiceRole::Local {
            continue;
        }
        check_endpoint(ctx, what, &service.url)?;
    }
    Ok(())
}

/// The heading naming what was started, printed once the first check passes.
fn announce(ctx: &mut Ctx<'_>, services: &[&'static str]) {
    let _ = writeln!(
        ctx.writer(),
        "\n  Started {}. Checking each one:",
        services.join(", ")
    );
}

/// One `name  detail` line under the heading.
fn line(ctx: &mut Ctx<'_>, what: &str, detail: &str) {
    let _ = writeln!(ctx.writer(), "    {what:<11} {detail}");
}

/// The managed database's connection test.
///
/// An existing or hosted store was tested while its questions were asked, so it
/// is exactly as reachable now as it was a minute ago and is not tested twice.
///
/// There is no repair to attempt on failure. The managed database has no
/// password, so there is nothing to rotate and nothing a `ALTER ROLE` could
/// settle; a refusal here is a real one and is reported as it is.
fn check_database(ctx: &mut Ctx<'_>, answers: &Answers) -> Result<(), String> {
    let Some(database) = answers.database.as_ref() else {
        return Ok(());
    };
    if !database.plan.starts_database() {
        return Ok(());
    }
    let address = database.plan.address.clone();
    announce(ctx, &crate::setup_preview::local_services(answers));
    match ctx.test_database(&address) {
        Ok(detail) => {
            line(ctx, "database", &detail);
            Ok(())
        }
        Err(reason) => Err(format!(
            "your settings were saved, but the Postgres Clio started did not accept the \
             connection: {}. The container is still running, so `docker logs \
             clio-postgres-1` says why.",
            reason.reason()
        )),
    }
}

/// A local sidecar's health check, with the container named when it refuses.
fn check_endpoint(ctx: &mut Ctx<'_>, what: &str, url: &str) -> Result<(), String> {
    match ctx.test_endpoint(url, None) {
        Ok(detail) => {
            line(ctx, what, &detail);
            Ok(())
        }
        Err(reason) => {
            let reason = reason.reason();
            if !reason.contains("sidecar unreachable") {
                return Err(format!(
                    "your settings were saved, but {what} did not answer: {reason}"
                ));
            }
            let container = sidecar_container(what);
            Err(format!(
                "your settings were saved, but {what} did not answer: {reason}\n  The container is \
                 {container} and it is still running. `docker logs {container}` says why: on a first \
                 run it is downloading and loading its model, which takes a few minutes, and it \
                 refuses connections until that finishes."
            ))
        }
    }
}

/// The Compose container that runs one local model service.
fn sidecar_container(what: &str) -> &'static str {
    match what {
        "embedding" => "clio-embed-1",
        _ => "clio-rerank-1",
    }
}
