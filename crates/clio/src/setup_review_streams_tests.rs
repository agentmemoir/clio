// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of what the last loop of a run does
//! when the terminal goes away: the two writes that carry the consequence notice
//! under the preview, and the read at the apply question.
//!
//! ## Owns
//! - The write and read error paths of `setup_review::review` under test
//!
//! ## Does not own
//! - What the preview shows (see `setup_preview_tests`)
//! - The amend and `switch` prompts themselves (see `setup_report_amend_tests`)
//! - The questions before the preview (see `setup_steps_tests`)
//!
//! ## Boundary
//!
//! Changes to how the last loop reports a dead terminal should normally update
//! tests here. Changes to the questions should not require changes here.
//!
//! Keep this module focused on the streams, not on the loop's decisions.

use super::*;
use crate::cli_style::Palette;
use crate::compose_docker::DockerStatus;
use crate::setup_preview_fixtures::{Harness, PASTED, SERVICES};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

/// The one string only the consequence notice carries.
///
/// The preview is written before the notice and does not contain it, so a writer
/// keyed on this text is still healthy when the wizard reaches the write under
/// test. A writer that failed everything would stop at the preview instead and
/// never get there.
const NOTICE: &str = "Nothing is deleted and nothing is copied";

/// The answers a complete run reaches the preview with.
fn answered() -> Answers {
    Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(format!("{PASTED}{SERVICES}"))
        .drive()
        .answers()
        .clone()
}

/// A store holding memories, so the consequence notice is drawn at all.
///
/// With an empty store there is no consequence to state, and the two writes under
/// test are never reached — so a scenario about them has to say the store is full.
fn full_store() -> StoreState {
    StoreState::NonEmpty {
        memories: 12,
        subjects: 3,
    }
}

/// Run the last loop over a reader and a writer the scenario chooses.
///
/// The context is built here rather than through the shared harness because that
/// harness owns a `Cursor` and a `Vec`, and neither can fail. Everything the loop
/// needs is passed in: a non-empty store, and no config file to back up.
fn last_loop<R: io::BufRead, W: Write>(
    reader: &mut R,
    out: &mut W,
    answers: Answers,
) -> Result<(Answers, Verdict), Signal> {
    let flags: HashMap<String, String> = HashMap::new();
    let mut err: Vec<u8> = Vec::new();
    let probes = crate::setup_test_seam::Probes::ok();
    let mut ctx = Ctx::new(
        &flags,
        reader,
        out,
        &mut err,
        true,
        Palette::new(false),
        &probes,
        crate::setup_defaults::Defaults::from_effective(&serde_json::json!({})),
        PathBuf::from("/nonexistent/clio-stage5/clio.db"),
        DockerStatus::Available,
        &crate::setup_preview_scripts::no_env,
        false,
    );
    review(&mut ctx, answers, &|_path: &str| None, None, full_store())
}

/// A writer that fails on the first write carrying [`NOTICE`].
struct FailsOnNotice;

impl Write for FailsOnNotice {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if String::from_utf8_lossy(buf).contains(NOTICE) {
            return Err(io::Error::other("the pipe closed"));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that lets the notice out and fails on the next write.
///
/// The notice and the blank line under it are two separate writes, so a terminal
/// that dies between them is a real thing that happens. The wizard has to report
/// it rather than draw half a sentence and carry on.
struct DiesAfterNotice {
    seen: Cell<bool>,
}

impl Write for DiesAfterNotice {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.seen.get() {
            return Err(io::Error::other("the pipe closed"));
        }
        if String::from_utf8_lossy(buf).contains(NOTICE) {
            self.seen.set(true);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A reader that refuses to read, because a `Cursor` cannot.
struct BrokenReader;

impl io::Read for BrokenReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("the terminal went away"))
    }
}

impl io::BufRead for BrokenReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Err(io::Error::other("the terminal went away"))
    }
    fn consume(&mut self, _amt: usize) {}
}

#[test]
fn a_terminal_that_dies_on_the_consequence_notice_is_reported() {
    // The notice is the last thing drawn before the apply question, so a pipe that
    // closes on it must stop the run rather than ask a question nobody can see.
    let mut reader = io::Cursor::new(Vec::new());
    let mut out = FailsOnNotice;
    let error = last_loop(&mut reader, &mut out, answered()).expect_err("a refusal");
    assert!(
        error.reason().contains("failed to write the preview"),
        "{error:?}"
    );
}

#[test]
fn a_terminal_that_dies_just_after_the_consequence_notice_is_reported() {
    // The blank line under the notice is a second write, so a terminal that dies
    // between the two is a failure the run has to notice on its own.
    let mut reader = io::Cursor::new(Vec::new());
    let mut out = DiesAfterNotice {
        seen: Cell::new(false),
    };
    let error = last_loop(&mut reader, &mut out, answered()).expect_err("a refusal");
    assert!(
        error.reason().contains("failed to write the preview"),
        "{error:?}"
    );
}

#[test]
fn a_terminal_that_goes_away_at_the_apply_question_is_reported() {
    // The writer is a `Vec` here, so the preview and the prompt are drawn and the
    // failure lands on the read — the one case where the operator pressed nothing
    // and the run still cannot continue. The message names the question and the
    // reason the pipe is gone, which is the part the operator can act on.
    let mut reader = BrokenReader;
    let mut out: Vec<u8> = Vec::new();
    let error = last_loop(&mut reader, &mut out, answered()).expect_err("a refusal");
    assert!(error.reason().contains("prompt failed"), "{error:?}");
    assert!(
        error.reason().contains("the terminal went away"),
        "{error:?}"
    );
}
