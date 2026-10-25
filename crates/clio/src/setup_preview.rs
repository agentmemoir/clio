// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for building the preview's read model from the
//! answers, and for the rules that decide which entries are new, unchanged, or
//! removed.
//!
//! ## Owns
//! - `planned_entries`, the one vector stage 6 will write and the preview shows
//! - `watched_paths`, the fields a run owns whether or not it writes them
//! - `local_services`, the containers a run will start
//! - `display`, the display form and the second column of every planned value
//! - The block layout: which sections appear, and in what order
//!
//! ## Does not own
//! - The database block's expansion of the address into its parts (see
//!   `setup_preview_db`)
//! - The change model and the layout (see `setup_report`)
//! - Producing the answers (see `setup_steps`)
//! - Writing the config (see `setup_config`)
//!
//! ## Boundary
//!
//! Changes to what the preview shows should normally require changes here.
//! Changes to how it is drawn, or to how the address is read apart, should not.
//!
//! Keep this module focused on assembling the preview's contents.
//!
//! # Why a secret is masked before the renderer sees it
//!
//! A preview gets pasted into issues. So a supplied secret shows `(set)` and
//! never its characters, and the plaintext never enters a value the renderer
//! could print. Masking happens here and in `setup_preview_db`, where it is
//! still known what the value is, rather than in the renderer, which cannot
//! tell a secret from any other string.

use crate::setup_preview_db as db;
use crate::setup_report::{self as report, Block, Change, ChangeState, PreviewView, Tested};
use crate::setup_services::{Service, ServiceRole};
use crate::setup_steps::Answers;
use serde_json::Value;

/// One planned value as the preview shows it: the display form, and the second
/// column.
pub(crate) type Displayed = (String, Option<String>);

/// The config entries the answers would write.
///
/// Built from the same `entries()` vectors stage 6 applies, in the same order. A
/// preview built from a parallel summary is a summary that eventually disagrees
/// with what gets written, and the disagreement is invisible until the config is
/// wrong.
#[must_use]
pub(crate) fn planned_entries(answers: &Answers) -> Vec<(String, Value)> {
    let mut entries = Vec::new();
    if let Some(database) = &answers.database {
        entries.extend(database.plan.entries());
    }
    for (name, service, provider) in [
        ("embed", &answers.embed, "openai"),
        ("rerank", &answers.rerank, "cohere"),
    ] {
        let Some(service) = service else { continue };
        entries.extend(service.entries(name, provider));
    }
    entries
}

/// Every watched path the answers own, whether or not they write it.
///
/// The preview needs the full set so a field that is *not* written shows as
/// removed rather than silently absent. That is what makes switching reranking
/// off visible instead of leaving a stale endpoint behind.
#[must_use]
pub(crate) fn watched_paths(answers: &Answers) -> Vec<&'static str> {
    let mut watched: Vec<&'static str> = db::watched().to_vec();
    watched.extend(
        answers
            .embed
            .as_ref()
            .map_or_else(Vec::new, |_| Service::watched("embed")),
    );
    watched.extend(
        answers
            .rerank
            .as_ref()
            .map_or_else(Vec::new, |_| Service::watched("rerank")),
    );
    watched
}

/// The Compose services the answers require started.
///
/// `postgres` appears only for the managed choice, and it appears because the
/// choice asks for it, not because it is Compose's default data plane. That
/// distinction is the whole silent-Postgres bug: `postgres` carries no
/// `profiles:` key, so a run that leaves it to the profile starts it anyway.
///
/// The Compose service names come from the role rather than from this list, so
/// the name the preview shows and the name the start command passes cannot be two
/// different strings.
#[must_use]
pub(crate) fn local_services(answers: &Answers) -> Vec<&'static str> {
    let mut services: Vec<&'static str> = Vec::new();
    if let Some(database) = &answers.database {
        services.extend(database.plan.database_services());
    }
    for (name, role) in [
        ("embed", answers.embed_role),
        ("rerank", answers.rerank_role),
        ("extract", answers.extract),
    ] {
        if let Some(service) = role.and_then(|role| role.compose_service(name)) {
            services.push(service);
        }
    }
    services
}

/// Build the whole preview from the answers.
///
/// `current` answers "what does the deployment file already set", one path at a
/// time. It is a lookup rather than the merged document because the merged
/// document carries every schema default: compared against it, a fresh install
/// would show every field as `unchanged` and an operator would agree to a write
/// they never saw.
///
/// `will_back_up` is the backup path the apply will take, or `None` on a first
/// run. It is passed in rather than derived from a path here, because whether a
/// file exists is a question for whoever is about to write it, not for a
/// renderer: probing the filesystem to decide what to print makes the preview
/// depend on a state it does not own.
///
/// `will_fetch` arrives the same way and for the same reason: whether the extract
/// model is already on disk is a question the caller answered, so this stays a
/// builder over the answers instead of a reader of the filesystem.
///
/// `env` is the run's own environment, the same one the apply records the store
/// choices through, so the key paths this prints are the ones the run will use.
#[must_use]
pub(crate) fn view(
    answers: &Answers,
    current: &dyn Fn(&str) -> Option<Value>,
    will_back_up: Option<String>,
    will_fetch: Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> PreviewView {
    let planned = planned_entries(answers);
    let watched = watched_paths(answers);
    let mut all = blocks(&planned, &watched, current);
    // The key stores are not config values, so no scope can pick them up. They
    // lead the preview anyway, because they are the first things the operator was
    // asked and the choices that cannot be taken back. The sync key's line sits
    // directly under the encryption key's when both were asked.
    let mut lead = 0;
    if let Some(source) = answers.key_source {
        all.insert(
            lead,
            Block {
                name: "Encryption key",
                changes: vec![Change {
                    path: "Kept in".to_owned(),
                    value: crate::setup_key_source::summary(source),
                    state: ChangeState::New,
                    note: Some("cannot be changed later".to_owned()),
                }],
            },
        );
        lead += 1;
    }
    if let Some(source) = answers.sync_key_source {
        all.insert(
            lead,
            Block {
                name: "Sync key",
                changes: vec![Change {
                    path: "Kept in".to_owned(),
                    value: crate::setup_sync_key_source::summary(source, env),
                    state: ChangeState::New,
                    note: Some("recorded once".to_owned()),
                }],
            },
        );
    }
    PreviewView {
        blocks: all,
        will_fetch,
        will_start: local_services(answers),
        will_remove: removals(&planned, &watched, current),
        will_back_up,
        tested: tested_lines(answers),
    }
}

/// The blocks, in the order the plan prints them.
///
/// A block appears only when it has something to say: either the run writes one of
/// its fields, or the config already holds one it would drop. An empty section is
/// omitted rather than drawn with nothing in it.
/// A preview section: its heading, and which config paths belong to it.
type Scope = (&'static str, fn(&str) -> bool);

fn blocks(
    planned: &[(String, Value)],
    watched: &[&'static str],
    current: &dyn Fn(&str) -> Option<Value>,
) -> Vec<Block> {
    let scopes: [Scope; 3] = [
        ("Database", db::is_database),
        ("Embedding", |path| {
            path.starts_with("embed") || path == "credentials.embed_api_key"
        }),
        ("Reranking", |path| {
            path.starts_with("rerank") || path == "credentials.rerank_api_key"
        }),
    ];
    scopes
        .into_iter()
        .filter_map(|(name, in_scope)| {
            let scope: Vec<&'static str> = watched
                .iter()
                .copied()
                .filter(|path| in_scope(path))
                .collect();
            let entries: Vec<(String, Value)> = planned
                .iter()
                .filter(|(path, _)| in_scope(path))
                .cloned()
                .collect();
            let holds_something = scope.iter().any(|path| current(path).is_some());
            if entries.is_empty() && !holds_something {
                return None;
            }
            let changes = if name == "Database" {
                db::block(&entries, &scope, current)
                    .unwrap_or_else(|| generic_block(&entries, &scope, current))
            } else {
                generic_block(&entries, &scope, current)
            };
            Some(Block { name, changes })
        })
        .collect()
}

/// Every scope's rows: the planned entries plus the watched ones being dropped.
fn generic_block(
    entries: &[(String, Value)],
    scope: &[&'static str],
    current: &dyn Fn(&str) -> Option<Value>,
) -> Vec<Change> {
    let masked: Vec<(String, Value)> = entries
        .iter()
        .map(|(path, value)| {
            let (shown, _) = display(path, value);
            (path.clone(), Value::String(shown))
        })
        .collect();
    let mut changes = report::changes(&masked, scope, current, &plain_display);
    for change in &mut changes {
        change.note = note_for(&change.path, entries);
    }
    changes
}

/// The display form for a secret whose length the caller must not report.
///
/// A preview gets pasted into issues, and a key's length is the provider's
/// business, so a secret is shown as present or absent and never measured.
pub(crate) fn secret_set() -> String {
    "(set)".to_owned()
}

/// A secret's length, never the secret.
///
/// Used where Clio itself chose the value and an operator can check it arrived.
pub(crate) fn secret_mask(value: &str) -> String {
    format!("({} characters)", value.chars().count())
}

/// The second column for one change, read from the entry it came from.
///
/// `None` for a value that needs no note. A secret gets `not shown`.
fn note_for(path: &str, entries: &[(String, Value)]) -> Option<String> {
    let (_, value) = entries.iter().find(|(planned, _)| planned == path)?;
    display(path, value).1
}

/// The display form of one planned value, with every secret masked.
///
/// A supplied secret shows `(set)` rather than its length: a key's length is the
/// provider's business, and a preview gets pasted into issues. The database
/// address is not rendered here at all — `setup_preview_db` expands it into its
/// parts, and shows a password row only when the address carries one.
#[must_use]
pub(crate) fn display(path: &str, value: &Value) -> Displayed {
    if clio_config::secret::path_is_secret(path) {
        let text = value.as_str().unwrap_or_default();
        let shown = if path.ends_with("_api_key") || text.is_empty() {
            secret_set()
        } else {
            secret_mask(text)
        };
        return (shown, Some("not shown".to_owned()));
    }
    match path {
        "embed.url" | "rerank.url" => {
            let url = value.as_str().unwrap_or_default();
            (format!("{url} ({})", service_note(url)), None)
        }
        other => (plain_display(other, value), None),
    }
}

/// `this machine only` for a loopback endpoint, `encrypted` for anything else.
fn service_note(url: &str) -> &'static str {
    match clio_types::net::http_target(url) {
        Ok(target) if clio_types::net::is_loopback_host(&target.host) => "this machine only",
        // A remote endpoint has to be `https`, because the key goes out as a
        // header on every request and `setup_validate::check_endpoint` refuses a
        // plain one. So "encrypted" is a fact about the address, not a hope.
        _ => "encrypted",
    }
}

/// The value as it is, for a path that holds nothing secret.
fn plain_display(path: &str, value: &Value) -> String {
    let _ = path;
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The paths the run will drop, named the way the preview names them.
fn removals(
    planned: &[(String, Value)],
    watched: &[&'static str],
    current: &dyn Fn(&str) -> Option<Value>,
) -> Vec<String> {
    watched
        .iter()
        .filter(|path| current(path).is_some())
        .filter(|path| !planned.iter().any(|(planned, _)| planned == **path))
        .map(|path| report::display_name(path))
        .collect()
}

/// One `Tested` line per probe the run performed.
///
/// A local service is not probed at this point, because nothing is started until
/// the operator applies — so its line says when it will be checked and is visibly
/// different from a tested one.
///
/// Every detail is redacted before it is printed. A probe is handed the address
/// and the most useful thing it can say is what it connected to, which on the
/// typed route contains the password the operator just typed. A preview gets
/// pasted into issues, so this uses the same redaction the store's own
/// diagnostics do.
fn tested_lines(answers: &Answers) -> Vec<Tested> {
    let redact = clio_ops::redact_credentials;
    let mut tested = Vec::new();
    if let Some(database) = &answers.database {
        tested.push(Tested {
            what: "database",
            detail: database
                .tested
                .as_deref()
                .map_or_else(|| "no connection was tested on this run".to_owned(), redact),
            was_tested: database.tested.is_some(),
        });
    }
    for (what, service) in [
        ("embedding", &answers.embed),
        ("reranking", &answers.rerank),
    ] {
        let Some(service) = service else { continue };
        if service.role == ServiceRole::Skip {
            continue;
        }
        let untested = || {
            if service.role == ServiceRole::Local {
                "Clio starts and checks this after you apply".to_owned()
            } else {
                "the endpoint was not tested".to_owned()
            }
        };
        tested.push(Tested {
            what,
            detail: service.tested.as_deref().map_or_else(untested, redact),
            was_tested: service.tested.is_some(),
        });
    }
    tested
}

#[cfg(test)]
#[path = "setup_preview_tests.rs"]
mod tests;
