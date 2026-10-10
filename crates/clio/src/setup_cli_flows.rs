// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared state one `clio setup` run needs:
//! the parsed flags, the three streams, the palette, the two probes, the masked
//! reader, the last-known defaults, and every prompt wrapper the flow modules
//! call.
//!
//! ## Owns
//! - `Ctx`: the run's injected state
//! - Flag access, so a flow never reaches into the map
//! - The prompt wrappers, turning a stream failure into a `Signal`
//! - Running the probes through `setup_connect`
//!
//! ## Does not own
//! - Which questions exist (see `setup_steps`)
//! - What a valid answer is (see `setup_validate`)
//! - Building the real probes (stage 6 passes them in; see `setup_test_seam`)
//!
//! ## Boundary
//!
//! Changes to the run's injected state or to a prompt wrapper should normally
//! require changes here. Changes to a specific question should not.
//!
//! Keep this module focused on plumbing one run together.
//!
//! # Why the streams are trait objects
//!
//! `setup_connect::run` takes `&mut dyn Write` so a caller cannot specialise it
//! per stream type, and a flow function that took `W: Write` would then have to
//! be generic too. Trait objects throughout keep every prompt helper taking the
//! same two parameters, which is what lets the flow modules stay small.
//!
//! # Why `b` and `q` are the only dialog keys
//!
//! A single letter is reserved because a free-text field that silently swallows
//! a legitimate one-character answer is worse than one that refuses it. The two
//! exceptions are named: the advanced `options` and `application_name` fields
//! read `b` as text, because any text is legitimate there.

use crate::cli_style::Palette;
use crate::compose_docker::DockerStatus;
use crate::setup_connect;
use crate::setup_defaults::Defaults;
use crate::setup_render::Signal;
use crate::setup_secrets::{SecretReader, production_reader};
use crate::setup_style;
use crate::setup_test_seam::Probes;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The injected state for one setup run.
pub(crate) struct Ctx<'a> {
    flags: &'a HashMap<String, String>,
    pub(crate) reader: &'a mut dyn BufRead,
    pub(crate) out: &'a mut dyn Write,
    pub(crate) err: &'a mut dyn Write,
    interactive: bool,
    pub(crate) palette: Palette,
    probes: &'a Probes,
    pub(crate) secrets: Box<SecretReader>,
    defaults: Defaults,
    sqlite_path: PathBuf,
    docker: DockerStatus,
    /// The operator's own environment, which is where a `CLIO_DATABASE_URL` and a
    /// `CLIO_KEY_FILE` come from.
    ///
    /// Injected rather than read from the process, for the same reason the streams
    /// are: the wizard offers the database the environment already names and writes
    /// a key-store marker beside the key the environment points at, so a test that
    /// inherited the machine's environment would assert a fact about the machine and
    /// would write into the machine's data directory.
    env: &'a dyn Fn(&str) -> Option<String>,
    pub(crate) terminal: bool,
    pub(crate) back_leaves_run: bool,
}

impl<'a> Ctx<'a> {
    /// Bind the flags, streams, and the pieces stage 6 supplies.
    ///
    /// One argument per decision the caller makes: grouping them behind a builder
    /// would move the caller's choices into the order it happened to call methods
    /// in, which is harder to read than a list.
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per decision the caller makes"
    )]
    pub(crate) fn new(
        flags: &'a HashMap<String, String>,
        reader: &'a mut dyn BufRead,
        out: &'a mut dyn Write,
        err: &'a mut dyn Write,
        interactive: bool,
        palette: Palette,
        probes: &'a Probes,
        defaults: Defaults,
        sqlite_path: PathBuf,
        docker: DockerStatus,
        env: &'a dyn Fn(&str) -> Option<String>,
        terminal: bool,
    ) -> Self {
        Self {
            flags,
            reader,
            out,
            err,
            interactive,
            palette,
            probes,
            secrets: production_reader(),
            defaults,
            sqlite_path,
            docker,
            env,
            terminal,
            // True until the first question is answered, because until then
            // there is nothing behind the operator to go back to. The question
            // layer clears it the moment a menu produces a value, which is the
            // only place that knows an answer arrived.
            back_leaves_run: true,
        }
    }

    /// Whether this machine can start the containers Clio needs.
    ///
    /// Asked once before the first question, so a menu can say whether a choice is
    /// available rather than letting the operator pick one and failing later.
    pub(crate) const fn docker(&self) -> &DockerStatus {
        &self.docker
    }

    /// Replace the masked reader with a scripted one.
    ///
    /// The only way a test reads a secret, and the reason no test opens a
    /// terminal. A production build leaves `rpassword` in place.
    #[cfg(test)]
    pub(crate) fn with_secrets(mut self, reader: Box<SecretReader>) -> Self {
        self.secrets = reader;
        self
    }

    /// The flag's value, or `None`.
    pub(crate) fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    /// Every value given for a repeatable flag, in the order they were passed.
    ///
    /// A repeatable flag is stored once with its values joined by a newline,
    /// because the flag parser hands back one `String` per name. Splitting it back
    /// here rather than in the parser keeps the parser's shape unchanged for the
    /// flags that are not repeatable.
    pub(crate) fn flag_values(&self, name: &str) -> Vec<&str> {
        self.flags
            .get(name)
            .map(|raw| raw.split('\n').collect())
            .unwrap_or_default()
    }

    /// Whether a flag was given at all.
    pub(crate) fn has_flag(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    /// Whether this run may prompt.
    pub(crate) const fn interactive(&self) -> bool {
        self.interactive
    }

    /// The operator's environment, for the paths and defaults it names.
    pub(crate) fn env(&self, key: &str) -> Option<String> {
        (self.env)(key)
    }

    /// The palette every styled string goes through.
    pub(crate) const fn palette(&self) -> Palette {
        self.palette
    }

    /// The writer, for a module that calls the render helpers directly.
    pub(crate) fn writer(&mut self) -> &mut dyn Write {
        self.out
    }

    /// Whether a back from the question being asked leaves the run.
    ///
    /// True only while nothing at all has been answered. A group that re-asks its
    /// own first question after a back reads this to restore the flag, because
    /// that question is genuinely the start of the run again.
    pub(crate) const fn back_leaves_run(&self) -> bool {
        self.back_leaves_run
    }

    /// Record whether a back from the question being asked leaves the run.
    pub(crate) const fn set_back_leaves_run(&mut self, leaves: bool) {
        self.back_leaves_run = leaves;
    }

    /// The last-known defaults, for the questions that offer one.
    pub(crate) fn defaults(&self) -> &Defaults {
        &self.defaults
    }

    /// Whether multi-host sync is in play for this run.
    ///
    /// The sync-key question exists only where sync is real. The fact comes
    /// from the effective `/sync/omit` pointer, never from a second question
    /// about the deployment shape.
    pub(crate) const fn sync_in_play(&self) -> bool {
        self.defaults.sync_in_play
    }

    /// The standard SQLite file path.
    pub(crate) fn standard_sqlite_path(&self) -> &Path {
        &self.sqlite_path
    }

    /// Print a dim section heading.
    pub(crate) fn section(&mut self, name: &str) {
        let painted = setup_style::section(self.palette, name);
        let _ = writeln!(self.out, "\n  {painted}");
    }

    /// Print a confirmation.
    pub(crate) fn confirm(&mut self, text: &str) {
        let painted = setup_style::confirm(self.palette, text);
        let _ = writeln!(self.out, "  {painted}");
    }

    /// Print a warning.
    pub(crate) fn warn(&mut self, text: &str) {
        let painted = setup_style::warn(self.palette, text);
        let _ = writeln!(self.out, "  {painted}");
    }

    /// Print a refusal in red.
    pub(crate) fn error(&mut self, text: &str) {
        let painted = setup_style::error(self.palette, text);
        let _ = writeln!(self.err, "  {painted}");
    }

    /// Run the database probe, printing the plan's `Testing …` line.
    ///
    /// # Errors
    /// Whatever the probe refused with, after it has been printed.
    pub(crate) fn test_database(&mut self, address: &str) -> Result<String, Signal> {
        let probes = self.probes;
        let writer: &mut dyn Write = self.out;
        setup_connect::run(writer, self.palette, "the connection", || {
            probes.test_database(address)
        })
        .map_err(Signal::Refused)
    }

    /// Run the endpoint probe, printing the plan's `Testing …` line.
    ///
    /// # Errors
    /// Whatever the probe refused with, after it has been printed.
    pub(crate) fn test_endpoint(
        &mut self,
        url: &str,
        api_key: Option<&str>,
    ) -> Result<String, Signal> {
        let probes = self.probes;
        let writer: &mut dyn Write = self.out;
        setup_connect::run(writer, self.palette, "the endpoint", || {
            probes.test_endpoint(url, api_key)
        })
        .map_err(Signal::Refused)
    }
}

#[cfg(test)]
#[path = "setup_cli_flows_tests.rs"]
mod tests;
