// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for running one step's group of questions and
//! recording what came back in [`Answers`].
//!
//! ## Owns
//! - `ask_group`, the dispatch from a step to its questions
//! - The role-to-detail rule: a local role answers nothing more, a hosted one
//!   answers the endpoint, model, and key
//! - The dispatch to the extraction question, which owns itself
//!
//! ## Does not own
//! - The step list and the going-back loop (see `setup_steps`)
//! - The database questions (see `setup_flows_db`)
//! - The extraction question and its not-available notice (see
//!   `setup_step_extract`)
//! - The per-service defaults a hosted role offers (see `setup_service_fields`)
//! - The endpoint check and the probe (see `setup_validate` and `setup_test_seam`)
//!
//! ## Boundary
//!
//! Changes to what one step asks should normally require changes here. Changes to
//! the order or to going back should not.
//!
//! Keep this module focused on one step's questions.
//!
//! # Why a hosted service asks for its key
//!
//! The plan's rule is that a remote endpoint must be `https`, because the key is
//! sent as a header on every request. So a hosted service always has a key, and
//! the masked reader is the only way one is read. A local service asks for
//! nothing, which is what lets a first-timer press Enter through the whole
//! wizard with no credential typed at all.
//!
//! # Why a failed probe is not a refusal of one field
//!
//! A probe refusal does not say which of the three values was wrong. Naming one
//! would be a guess that sends an operator to change something already right, so
//! the group reports the failure and the loop decides what to do about it.

use crate::setup_cli_flows::Ctx;
use crate::setup_prompt::Choice;
use crate::setup_render::{Reply, Signal};
use crate::setup_service_fields::{embed_defaults, rerank_defaults, title};
use crate::setup_services::{Service, ServiceDefaults, ServiceRole};
use crate::setup_steps::{Answers, Step};

/// What one group of questions did.
///
/// A `Copy` enum rather than the `Signal` the render helpers return, because this
/// is the loop's vocabulary: `Back` and `Quit` are the two moves the loop can act
/// on, and a refusal is an error that must not be mistaken for either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The group finished and its answers are recorded.
    Done,
    /// The operator asked to go back to the previous question.
    Back,
    /// The operator left: a back on the first question, or a cancel at the gate.
    Quit,
}

/// Run one step's questions.
///
/// # Errors
/// A refusal the loop cannot act on: a probe that failed its attempts, or a
/// value no rule accepts.
pub(crate) fn ask_group(
    ctx: &mut Ctx<'_>,
    step: Step,
    answers: &mut Answers,
) -> Result<Outcome, Signal> {
    match step {
        Step::KeySource => {
            // The run's own environment, for the reason `apply` documents: this
            // reads a marker beside the key, and the process environment would make
            // a test read the developer's real data directory.
            let recorded = clio_config::secret_key_source::recorded(&|key| ctx.env(key));
            crate::setup_key_source::ask(ctx, answers, recorded)
        }
        Step::SyncKeySource => {
            // The same rule for the sync key: its own marker and the run's own
            // environment, never the process's. A marker that exists but cannot
            // be read is a refusal rather than "no choice": asking again would
            // re-ask a question the machine already answered, and the answer
            // could not then be recorded.
            let recorded = clio_config::sync_key::recorded_source(&|key| ctx.env(key))
                .map_err(|error| Signal::Refused(error.message))?;
            crate::setup_sync_key_source::ask(ctx, answers, recorded)
        }
        Step::Database => ask_database(ctx, answers),
        Step::EmbedRole => ask_role(ctx, "embed", false, answers),
        Step::EmbedService => ask_service(ctx, "embed", answers),
        Step::RerankRole => ask_role(ctx, "rerank", true, answers),
        Step::RerankService => ask_service(ctx, "rerank", answers),
        Step::Extract => crate::setup_step_extract::ask_extract(ctx, answers),
    }
}

/// The database group, which owns its own sub-choice.
fn ask_database(ctx: &mut Ctx<'_>, answers: &mut Answers) -> Result<Outcome, Signal> {
    ctx.section("Database");
    match crate::setup_flows_db::ask_database(ctx) {
        Ok(database) => {
            answers.database = Some(database);
            Ok(Outcome::Done)
        }
        Err(Signal::Back) => Ok(Outcome::Back),
        Err(Signal::Quit) => Ok(Outcome::Quit),
        Err(other) => Err(other),
    }
}

/// One service's role question, and what the role decides next.
///
/// A local role is complete on its own: the sidecar's address and model are
/// constants in `compose.yml`, so asking would be asking for something the
/// operator cannot change without editing that file. A hosted role records the
/// role and leaves the answer empty, so the detail step appears.
fn ask_role(
    ctx: &mut Ctx<'_>,
    name: &str,
    allow_skip: bool,
    answers: &mut Answers,
) -> Result<Outcome, Signal> {
    let previous = if name == "embed" {
        answers.embed_role
    } else {
        answers.rerank_role
    };
    if let Some(raw) = ctx.flag(name) {
        let role = ServiceRole::parse(raw).ok_or_else(|| {
            Signal::usage(format!(
                "--{name} must be local|online{}, got `{raw}`",
                if allow_skip { "|skip" } else { "" }
            ))
        })?;
        record_role(name, role, answers);
        return Ok(Outcome::Done);
    }
    if !ctx.interactive() {
        return Err(Signal::usage(format!(
            "--{name} local|online{} is required without a terminal",
            if allow_skip { "|skip" } else { "" }
        )));
    }
    ctx.section(if name == "embed" {
        "Embedding"
    } else {
        "Reranking"
    });
    let label = if name == "embed" {
        "Where should embeddings come from?"
    } else {
        "Where should reranking come from?"
    };
    // The same Docker check the database question uses, because a local model is
    // a container exactly as much as a managed Postgres is. The label says so
    // rather than letting the operator pick it and meet the failure later.
    let local = if ctx.docker().is_available() {
        Choice::new("local", "Local Docker", "Runs the model on this machine.")
            .with_note("Docker is running")
    } else {
        Choice::new(
            "local",
            "Local Docker",
            "Not available on this machine right now.",
        )
        .with_note("Docker is not running")
    };
    // Skip is always the last option, so the option that turns the feature off is
    // never one keystroke from Enter.
    let mut options: Vec<Choice> = vec![
        local,
        Choice::new(
            "hosted",
            "A hosted API",
            "Sends your text to a service on the internet. Needs an API key.",
        ),
    ];
    if allow_skip {
        options.push(Choice::new(
            "skip",
            "Skip",
            "Turns this feature off. You can add it later.",
        ));
    }
    // Local is the default for a fresh install because it needs no key, so a
    // first-timer can press Enter through the whole wizard without typing a
    // credential. That only holds when Docker can run it; otherwise the default
    // moves to the option that needs nothing from the machine.
    let default = match previous {
        Some(ServiceRole::Skip) if allow_skip => "skip",
        Some(ServiceRole::Local) | None if ctx.docker().is_available() => "local",
        Some(role) => role.as_str(),
        None if allow_skip => "skip",
        None => "hosted",
    };
    loop {
        // `ask_choice` refuses anything outside the options above and re-asks, so
        // the answer is one of them or the run has already stopped. The parse
        // cannot fail, and an arm no answer can reach would be a refusal about a
        // bug rather than about anything the operator did. See `setup_flows_db`
        // for the same decision on the database choices.
        let value = match ctx.ask_choice(label, &options, default)? {
            Reply::Back => return Ok(Outcome::Back),
            Reply::Value(value) => value,
        };
        if value == "local" && !ctx.docker().is_available() {
            ctx.error(&format!(
                "Docker is not available, so Clio cannot run the model on this machine: {}. Start \
                 Docker and run this again, or choose a hosted API.",
                ctx.docker().reason()
            ));
            continue;
        }
        let role = ServiceRole::parse(&value).expect("one of the options above");
        record_role(name, role, answers);
        return Ok(Outcome::Done);
    }
}

/// Record a role and whatever service it answers outright.
fn record_role(name: &str, role: ServiceRole, answers: &mut Answers) {
    let service = match role {
        ServiceRole::Local => Some(Service::local(name)),
        ServiceRole::Hosted => None,
        ServiceRole::Skip => Some(Service::skipped()),
    };
    if name == "embed" {
        answers.embed_role = Some(role);
        answers.embed = service;
    } else {
        answers.rerank_role = Some(role);
        answers.rerank = service;
    }
}

/// The provider one hosted service records.
///
/// The provider is the one hosted field with no question, so its flag is read
/// and checked here rather than through a prompt: the flag wins, then what this
/// run already collected, then what the last run wrote.
fn service_provider(
    ctx: &Ctx<'_>,
    name: &str,
    defaults: &ServiceDefaults,
) -> Result<Option<String>, Signal> {
    let allowed: &[&str] = if name == "embed" {
        &["tei", "openai"]
    } else {
        &["tei", "cohere"]
    };
    match ctx.flag(&format!("{name}-provider")) {
        Some(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            if !allowed.contains(&normalized.as_str()) {
                return Err(Signal::usage(format!(
                    "--{name}-provider must be {}, got `{raw}`",
                    allowed.join("|")
                )));
            }
            Ok(Some(normalized))
        }
        None => Ok(defaults.provider.clone()),
    }
}

/// One hosted service's questions, then its probe.
///
/// The endpoint, the model, and the provider come from their flags when given.
/// The key comes from the masked reader, which is the only place a secret is
/// ever read.
fn ask_service(ctx: &mut Ctx<'_>, name: &str, answers: &mut Answers) -> Result<Outcome, Signal> {
    let previous = if name == "embed" {
        answers.embed.clone()
    } else {
        answers.rerank.clone()
    };
    // A re-ask offers what this run already collected, not only what the last run
    // wrote. Going back and returning forward must not make the operator retype an
    // endpoint they gave a minute ago, and the two differ whenever the config
    // predates the answer.
    let mut defaults = if name == "embed" {
        embed_defaults(ctx)
    } else {
        rerank_defaults(ctx)
    };
    if let Some(earlier) = &previous {
        if !earlier.url.is_empty() {
            defaults.url = Some(earlier.url.clone());
        }
        if !earlier.model.is_empty() {
            defaults.model = Some(earlier.model.clone());
        }
        defaults.provider.clone_from(&earlier.provider);
        defaults.dims = earlier.dims.or(defaults.dims);
    }
    let label = if name == "embed" {
        "Embedding"
    } else {
        "Reranking"
    };
    ctx.section(label);

    let url = take_flag_or_prompt(
        ctx,
        &format!("{name}-url"),
        &format!("{} endpoint URL", title(name)),
        defaults.url.as_deref(),
        crate::setup_validate::check_endpoint,
    )?;
    let Some(url) = url else {
        return Ok(Outcome::Back);
    };
    let model = take_flag_or_prompt(
        ctx,
        &format!("{name}-model"),
        &format!("{} model", title(name)),
        defaults.model.as_deref(),
        |value| {
            if value.trim().is_empty() {
                Err("the model name is required".to_owned())
            } else {
                Ok(())
            }
        },
    )?;
    let Some(model) = model else {
        return Ok(Outcome::Back);
    };
    // Checked before the key so a misspelled provider fails fast instead of
    // costing a secret entry and an endpoint probe.
    let provider = service_provider(ctx, name, &defaults)?;
    let stored_key = previous.as_ref().and_then(|service| service.key.clone());
    let key = match ctx.flag(&format!("{name}-key")) {
        Some(flagged) => Some(flagged.to_owned()),
        None if !ctx.interactive() => {
            return Err(Signal::usage(format!(
                "--{name}-key is required without a terminal"
            )));
        }
        // An empty answer means "keep the stored key", so an empty string must
        // not replace a key the operator already has.
        None if defaults.has_key && stored_key.is_some() => stored_key,
        None => Some(ctx.ask_password(&format!("{} API key", title(name)))?),
    };
    // A waived endpoint test records nothing and keeps the answers, which is the
    // flag-driven equivalent of the interactive "save anyway".
    let detail = match ctx.test_endpoint(&url, key.as_deref()) {
        Ok(detail) => Some(detail),
        Err(_) if ctx.has_flag("allow-untested") => {
            ctx.warn(crate::setup_connect::UNTESTED_WARNING);
            None
        }
        Err(other) => return Err(other),
    };
    let service = Service {
        role: ServiceRole::Hosted,
        url,
        model,
        key,
        provider,
        dims: defaults.dims,
        tested: detail,
    };
    if name == "embed" {
        answers.embed = Some(service);
    } else {
        answers.rerank = Some(service);
    }
    Ok(Outcome::Done)
}

/// A value from its flag, or from a question, or from the last run.
///
/// `Ok(None)` means the operator pressed `b`.
fn take_flag_or_prompt<C>(
    ctx: &mut Ctx<'_>,
    flag: &str,
    label: &str,
    default: Option<&str>,
    mut check: C,
) -> Result<Option<String>, Signal>
where
    C: FnMut(&str) -> Result<(), String> + 'static,
{
    if let Some(flagged) = ctx.flag(flag) {
        return check(flagged)
            .map(|()| Some(flagged.to_owned()))
            .map_err(|reason| Signal::usage(format!("--{flag} {reason}")));
    }
    if !ctx.interactive() {
        return Err(Signal::usage(format!(
            "--{flag} is required without a terminal"
        )));
    }
    // A back here means "go back to the question before this one", which the
    // caller recognises as a `None`. Letting the `Back` signal out instead ends
    // the whole run instead of stepping back one question.
    match ctx.ask_checked(label, default, check) {
        Ok(value) => Ok(Some(value)),
        Err(Signal::Back) => Ok(None),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
#[path = "setup_step_ask_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "setup_step_ask_provider_tests.rs"]
mod provider_tests;
