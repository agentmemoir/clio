// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one finished run's results: the answers or the
//! signal that stopped it, the preview, the verdict, and both captured streams.
//!
//! ## Owns
//! - `Run`, the value a driven run hands back
//! - The accessors a suite reads it through
//!
//! ## Does not own
//! - Building and driving a run (see `setup_preview_fixtures`)
//! - The answers themselves (see `setup_steps`)
//! - The preview's shape (see `setup_report`)
//!
//! ## Boundary
//!
//! Changes to what a run hands back should normally require changes here. Changes
//! to how a run is driven should not require changes here.
//!
//! Keep this module focused on the result of a run.

#[cfg(test)]
#[path = "setup_preview_run_tests.rs"]
mod tests;

use crate::cli_style::Palette;
use crate::setup_cli_flows::Ctx;
use crate::setup_defaults::Defaults;
use crate::setup_preview as preview;
use crate::setup_render::Signal;
use crate::setup_report::{self as report, PreviewView};
use crate::setup_review::{self as review, Verdict};
use crate::setup_steps::{self as steps, Answers};
use crate::setup_test_seam::{Probes, scripted_secrets};

/// One run's results.
pub(crate) struct Run {
    /// The answers, or the signal that stopped the run.
    pub(crate) outcome: Result<Answers, Signal>,
    /// The preview, when the run finished.
    pub(crate) view: Option<PreviewView>,
    /// What the operator chose at the review prompt, when the loop ran.
    pub(crate) verdict: Option<Verdict>,
    /// Everything the run wrote to stdout.
    pub(crate) out: String,
    /// Everything the run wrote to stderr.
    pub(crate) err: String,
}

impl Run {
    /// The collected answers.
    ///
    /// # Panics
    /// If the run did not finish. Read [`Run::outcome`] when that is expected.
    pub(crate) fn answers(&self) -> &Answers {
        self.outcome
            .as_ref()
            .expect("the wizard finished; read Run::outcome when it need not")
    }

    /// The signal the run stopped on.
    #[must_use]
    pub(crate) fn signal(&self) -> Option<&Signal> {
        self.outcome.as_ref().err()
    }

    /// The preview.
    ///
    /// # Panics
    /// If the run did not finish.
    pub(crate) fn view(&self) -> &PreviewView {
        self.view
            .as_ref()
            .expect("the wizard finished, so a preview was built")
    }

    /// Every string the preview would print, one per line.
    ///
    /// Reading the rendered form rather than the view struct is what keeps a
    /// preview assertion about what an operator sees.
    pub(crate) fn preview_text(&self) -> String {
        let view = self.view();
        let mut out: Vec<u8> = Vec::new();
        crate::setup_report::preview(&mut out, Palette::new(false), view)
            .expect("the preview writes to a Vec");
        String::from_utf8(out).expect("utf8")
    }
}

impl crate::setup_preview_fixtures::Harness {
    /// Ask every question, then stop at the preview.
    pub(crate) fn drive(mut self) -> Run {
        let env = self.env();
        let defaults = Defaults::from_effective(&self.effective);
        let probes = std::mem::replace(&mut self.probes, Probes::ok());
        let will_back_up = self.will_back_up();
        let mut reader = std::io::Cursor::new(std::mem::take(&mut self.input).into_bytes());
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut ctx = Ctx::new(
            &self.flags,
            &mut reader,
            &mut out,
            &mut err,
            self.interactive,
            Palette::new(false),
            &probes,
            defaults,
            self.sqlite_path.clone(),
            self.docker.clone(),
            &env,
            false,
        )
        .with_secrets(scripted_secrets(std::mem::take(&mut self.secrets)));
        let preflight = self.preflight();
        let outcome = steps::drive(&mut ctx, preflight);
        let current = &self.effective;
        let view = outcome.as_ref().ok().map(|answers| {
            preview::view(
                answers,
                &|path| report::value_at(current, path),
                will_back_up,
                &env,
            )
        });
        Run {
            outcome,
            view,
            verdict: None,
            out: String::from_utf8(out).expect("utf8"),
            err: String::from_utf8(err).expect("utf8"),
        }
    }

    /// Ask every question, then run the review loop to apply or quit.
    pub(crate) fn review(mut self) -> Run {
        let env = self.env();
        let defaults = Defaults::from_effective(&self.effective);
        let probes = std::mem::replace(&mut self.probes, Probes::ok());
        let will_back_up = self.will_back_up();
        let mut reader = std::io::Cursor::new(std::mem::take(&mut self.input).into_bytes());
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut ctx = Ctx::new(
            &self.flags,
            &mut reader,
            &mut out,
            &mut err,
            self.interactive,
            Palette::new(false),
            &probes,
            defaults,
            self.sqlite_path.clone(),
            self.docker.clone(),
            &env,
            false,
        )
        .with_secrets(scripted_secrets(std::mem::take(&mut self.secrets)));
        let preflight = self.preflight();
        let effective = self.effective.clone();
        let store = self.store;
        let mut verdict = None;
        let outcome = steps::drive(&mut ctx, preflight).and_then(|answers| {
            review::review(
                &mut ctx,
                answers,
                &|path| report::value_at(&effective, path),
                will_back_up.as_deref(),
                store,
            )
            .map(|(answers, chosen)| {
                verdict = Some(chosen);
                answers
            })
        });
        let view = outcome.as_ref().ok().map(|answers| {
            preview::view(
                answers,
                &|path| report::value_at(&effective, path),
                will_back_up.clone(),
                &env,
            )
        });
        Run {
            outcome,
            view,
            verdict,
            out: String::from_utf8(out).expect("utf8"),
            err: String::from_utf8(err).expect("utf8"),
        }
    }
}
