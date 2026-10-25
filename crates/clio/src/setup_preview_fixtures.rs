// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared harness the wizard test suites
//! drive: a scripted context over injected streams, faked probes, and a scripted
//! masked reader.
//!
//! ## Owns
//! - `Harness`, the flags, streams, probes, and stored document for one run
//! - `Run`, what a run produced
//! - The faked probes a test can make succeed or fail
//!
//! ## Does not own
//! - The route scripts (see `setup_preview_scripts`, re-exported here)
//! - Any production behavior
//! - Any assertion: each suite asserts on what it scripted
//!
//! ## Boundary
//!
//! Changes to the context's constructor or to a seam signature should normally
//! require changes here. Changes to the order of questions should normally
//! require changes in `setup_preview_scripts` instead.
//!
//! Keep this module focused on driving a run.
//!
//! # Why this is `#[cfg(test)]`
//!
//! Every seam it uses exists so a test can script it. In a shipped build nothing
//! constructs a `Harness`, so compiling it would be dead weight in the binary the
//! user runs.
//!
//! # Why both entry points build the context themselves
//!
//! `Ctx` borrows the reader and both writers for the whole run, so it cannot sit
//! in a struct beside the buffers it borrows. `drive` and `review` therefore
//! each declare the same four locals and hand them over. The duplication is four
//! lines, and the alternative — a struct that owns the buffers and hands out
//! borrows — needs a self-referential type to keep `Ctx` alive across a call.

#![allow(
    dead_code,
    reason = "a shared harness is used by several suites, not all at once"
)]

use crate::compose_docker::DockerStatus;
use crate::setup_cli_flows::Ctx;
use crate::setup_key_source::FLAG as KEY_STORE_FLAG;
pub(crate) use crate::setup_preview_run::Run;
use crate::setup_review::StoreState;
use crate::setup_test_seam::Probes;
use clio_config::secret_key::KEY_FILE_ENV;
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

// The scripts are re-exported so every suite has one import path. They live in
// their own module because they are a statement about the questions a route asks,
// and that statement has to change with the questions rather than with the way a
// run is driven.
pub(crate) use crate::setup_preview_scripts::{
    ABSENT_CONFIG, ALL_LOCAL, EXISTING_CONFIG, MANAGED, NO_RERANK, PASTED, SERVICES, SQLITE, TYPED,
    TYPED_BY_FLAG, no_env,
};

/// One scripted setup run, with everything injected.
pub(crate) struct Harness {
    pub(crate) flags: HashMap<String, String>,
    pub(crate) input: String,
    pub(crate) secrets: Vec<String>,
    pub(crate) interactive: bool,
    pub(crate) probes: Probes,
    pub(crate) effective: Value,
    pub(crate) sqlite_path: PathBuf,
    pub(crate) config_exists: bool,
    pub(crate) preflight_fails: Option<String>,
    pub(crate) store: StoreState,
    pub(crate) docker: DockerStatus,
    pub(crate) key_file: Option<PathBuf>,
    /// The compose root a run checks the extract model under.
    ///
    /// A path that does not exist, so the presence check can only report the model
    /// as absent: a fixture that named a real directory would make the preview
    /// depend on the machine running the suite.
    pub(crate) compose_dir: Option<String>,
}

/// The compose root a harness checks the extract model under: a path that is not
/// there, so the check is about the code rather than about the machine.
pub(crate) const ABSENT_COMPOSE_ROOT: &str = "/nonexistent/clio-stage5/compose";

/// The key store a run uses when the test does not care which one.
///
/// The question is the first the wizard asks on a machine that has never chosen,
/// so leaving it in the scripts would put one extra line in front of every route
/// and make each script a count of questions rather than a statement about a
/// route. Answering it by flag removes that coupling; the question itself is
/// tested where it belongs, in `setup_key_source_tests`.
const DEFAULT_KEY_STORE: &str = "file";

impl Harness {
    /// A fresh install: no flags, no stored config, both probes succeeding.
    ///
    /// Docker is available, because the managed and local choices are the ones
    /// the scripts are written for and a refused choice would stop a run before
    /// the question the test is about. A test that needs Docker to be missing
    /// says so with [`Harness::without_docker`].
    #[must_use]
    pub(crate) fn fresh() -> Self {
        Self {
            flags: HashMap::from([(KEY_STORE_FLAG.to_owned(), DEFAULT_KEY_STORE.to_owned())]),
            input: String::new(),
            secrets: Vec::new(),
            interactive: true,
            probes: Probes::ok(),
            effective: json!({}),
            sqlite_path: PathBuf::from("/nonexistent/clio-stage5/clio.db"),
            config_exists: false,
            preflight_fails: None,
            store: StoreState::Unknown,
            docker: DockerStatus::Available,
            // A key file under a directory this harness creates, so the key-store
            // marker a run writes lands there rather than in the machine's real
            // data directory. `None` names no key file at all, which is the
            // one-time case where the question is asked.
            key_file: None,
            compose_dir: Some(ABSENT_COMPOSE_ROOT.to_owned()),
        }
    }

    /// The compose root this run reads the extract model's presence under.
    ///
    /// Set only by a test that is about that file: the default names a path that
    /// does not exist, so no other route depends on what is on the machine.
    #[must_use]
    pub(crate) fn with_compose_dir(mut self, dir: &str) -> Self {
        self.compose_dir = Some(dir.to_owned());
        self
    }

    /// A run whose key file is at `path`, and whose key-store marker therefore
    /// lands beside it.
    ///
    /// This is the seam that keeps a test from writing into the data directory of
    /// whoever is running it: without it the marker path resolves from the process
    /// environment, and a suite that applies an answer writes a real `key-source`
    /// file that then makes every later run skip the question.
    #[must_use]
    pub(crate) fn with_key_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.key_file = Some(path.into());
        self
    }

    /// The environment this run sees: its own key file, and nothing else.
    pub(crate) fn env(&self) -> impl Fn(&str) -> Option<String> + use<> {
        let key_file = self
            .key_file
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        move |key: &str| (key == KEY_FILE_ENV).then(|| key_file.clone()).flatten()
    }

    /// A run on a machine where Clio cannot start a container.
    ///
    /// `reason` is what the menus tell the operator, so a test can assert the
    /// advice rather than only that a choice was refused.
    #[must_use]
    pub(crate) fn without_docker(reason: &str) -> Self {
        Self::fresh().with_docker(DockerStatus::Unavailable(reason.to_owned()))
    }

    /// A run on a machine with the given Docker status.
    #[must_use]
    pub(crate) fn with_docker(mut self, status: DockerStatus) -> Self {
        self.docker = status;
        self
    }

    /// A run that is asked where the encryption key should be kept.
    ///
    /// The default answers the question by flag, so this drops that answer and
    /// leaves the question in the plan, which is where the key-store tests need
    /// it.
    #[must_use]
    pub(crate) fn asking_the_key_store(mut self) -> Self {
        self.flags.remove(KEY_STORE_FLAG);
        self
    }

    /// A non-interactive run, which never prompts.
    #[must_use]
    pub(crate) fn non_interactive(mut self) -> Self {
        self.interactive = false;
        self
    }

    /// Add one flag.
    #[must_use]
    pub(crate) fn flag(mut self, name: &str, value: &str) -> Self {
        self.flags.insert(name.to_owned(), value.to_owned());
        self
    }

    /// Add several flags at once.
    #[must_use]
    pub(crate) fn flags<const N: usize>(mut self, pairs: [(&str, &str); N]) -> Self {
        for (name, value) in pairs {
            self.flags.insert(name.to_owned(), value.to_owned());
        }
        self
    }

    /// The lines the operator types, one per question.
    ///
    /// Takes anything that reads as a string, so a test that builds a script
    /// from the route constants can hand over a `String` and one that has a
    /// literal can hand over a `&str`.
    #[must_use]
    pub(crate) fn typing(mut self, lines: impl AsRef<str>) -> Self {
        self.input.push_str(lines.as_ref());
        self
    }

    /// The secrets the masked reader hands out, in order.
    #[must_use]
    pub(crate) fn with_secrets(mut self, values: &[&str]) -> Self {
        self.secrets = values.iter().map(|value| (*value).to_owned()).collect();
        self
    }

    /// The secrets of a run, already owned.
    #[must_use]
    fn with_secrets_owned(mut self, values: Vec<String>) -> Self {
        self.secrets = values;
        self
    }

    /// The secrets of a pasted route: the pasted address, then whatever else.
    ///
    /// The address is a secret, so the pasted route reads it from the masked
    /// reader rather than from a typed line, and it is always the first value that
    /// reader hands out. Prepending it here means a test states its own secrets in
    /// the order the *rest* of the run wants them and does not have to remember
    /// that the address comes first.
    #[must_use]
    pub(crate) fn with_pasted_secrets(self, rest: &[&str]) -> Self {
        self.with_secrets_owned(crate::setup_preview_scripts::pasted_secrets(rest))
    }

    /// The effective document the run reads its defaults from.
    #[must_use]
    pub(crate) fn with_effective(mut self, document: Value) -> Self {
        self.effective = document;
        self
    }

    /// Pretend a deployment config already exists, so the backup line names it.
    #[must_use]
    pub(crate) fn with_existing_config(mut self) -> Self {
        self.config_exists = true;
        self
    }

    /// Pretend the store holds this much, so a `switch` confirmation is asked for.
    #[must_use]
    pub(crate) fn with_non_empty_store(mut self, memories: u64, subjects: u64) -> Self {
        self.store = StoreState::NonEmpty { memories, subjects };
        self
    }

    /// Make the database probe refuse with `reason`.
    #[must_use]
    pub(crate) fn database_fails(mut self, reason: &str) -> Self {
        let reason = reason.to_owned();
        self.probes = Probes::new(
            move |_address: &str| Err(reason.clone()),
            |url: &str, _| Ok(format!("{url} answered")),
        );
        self
    }

    /// Make the database probe refuse the first `count` times, then succeed.
    ///
    /// Counting rather than refusing always, so a test can observe the blame
    /// step-back: the refusal has to stop on its own for the step-back to be
    /// observable at all. The count lives in a [`Cell`] because the seam takes an
    /// `Fn`, and a closure that assigns to what it captured is not one.
    #[must_use]
    pub(crate) fn database_fails_times(mut self, count: usize, reason: &str) -> Self {
        let reason = reason.to_owned();
        let left = Cell::new(count);
        self.probes = Probes::new(
            move |address: &str| {
                if left.get() > 0 {
                    left.set(left.get() - 1);
                    return Err(reason.clone());
                }
                Ok(format!("connected to {address}"))
            },
            |url: &str, _| Ok(format!("{url} answered")),
        );
        self
    }

    /// Make the endpoint probe refuse with `reason`.
    #[must_use]
    pub(crate) fn endpoint_fails(mut self, reason: &str) -> Self {
        let reason = reason.to_owned();
        self.probes = Probes::new(
            |address: &str| Ok(format!("connected to {address}")),
            move |_url: &str, _| Err(reason.clone()),
        );
        self
    }

    /// Make the preflight refuse with `reason`, as a contradictory flag would.
    #[must_use]
    pub(crate) fn preflight_fails(mut self, reason: &str) -> Self {
        self.preflight_fails = Some(reason.to_owned());
        self
    }

    /// The backup path the apply would take, which is what the preview prints.
    ///
    /// A fact rather than a filesystem probe, so a test can say "there is a config
    /// to back up" without creating a file, and a first run prints the plan's
    /// phrase without depending on a path that happens not to exist.
    pub(crate) fn will_back_up(&self) -> Option<String> {
        self.config_exists.then(|| EXISTING_CONFIG.to_owned())
    }

    /// The preflight this run refuses with, if any.
    pub(crate) fn preflight(&self) -> impl FnOnce(&Ctx<'_>) -> Result<(), String> + use<> {
        let reason = self.preflight_fails.clone();
        move |_| match &reason {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }
    /// Run the wizard and require it to finish.
    ///
    /// The panic message carries both streams, because a scripted test that stops
    /// on an unexpected signal is otherwise very hard to read.
    pub(crate) fn run(self) -> Run {
        let run = self.drive();
        assert!(
            run.outcome.is_ok(),
            "the wizard did not finish.\n--- out ---\n{}\n--- err ---\n{}",
            run.out,
            run.err
        );
        run
    }

    /// Run the wizard and require it to stop with a refusal, returning the text.
    pub(crate) fn expect_refusal(self) -> String {
        let run = self.drive();
        match run.outcome {
            Ok(_) => panic!(
                "the wizard finished instead of refusing.\n--- out ---\n{}",
                run.out
            ),
            Err(signal) => signal.reason().to_owned(),
        }
    }
}
