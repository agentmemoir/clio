// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a probe's attempt and its outcome are reported
//! faithfully, in plain text when colour is off and with the probe's own words.
//!
//! ## Owns
//! - The `Testing … ok` line on success
//! - The `failed` line and the indented refusal on failure
//! - The report being handed back to the caller
//!
//! ## Does not own
//! - What a real probe does
//! - What the operator does about a failure
//!
//! ## Boundary
//!
//! Tests here break when the reported shape changes; they never change the
//! implementation.
//!
//! Keep this module focused on the reported output.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

/// A writer that hands its bytes to a shared cell, so a probe can read what has
/// already been written while `run` still holds the writer.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Shared {
    fn written(&self) -> String {
        String::from_utf8_lossy(&self.0.borrow()).into_owned()
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn plain() -> Palette {
    Palette::new(false)
}

#[test]
fn a_successful_probe_reports_ok() {
    let mut out = Vec::new();
    let report = run(&mut out, plain(), "the database", || {
        Ok("PostgreSQL 17".to_owned())
    })
    .expect("the probe succeeded");
    assert_eq!(
        String::from_utf8_lossy(&out),
        "Testing the database ... ok\n"
    );
    assert_eq!(report, "PostgreSQL 17");
}

#[test]
fn a_failed_probe_reports_the_reason_verbatim() {
    let mut out = Vec::new();
    let error = run(&mut out, plain(), "the database", || {
        Err("connection refused (os error 61)".to_owned())
    })
    .expect_err("the probe failed");
    assert_eq!(
        String::from_utf8_lossy(&out),
        "Testing the database ... failed\n  connection refused (os error 61)\n"
    );
    assert_eq!(error, "connection refused (os error 61)");
}

#[test]
fn a_refusal_never_prints_the_password_it_was_handed() {
    // The probe is given the connection string, and the most useful thing it can
    // say is which address it tried — which on the typed route carries the
    // password the operator just typed. This is the whole reason the reason is
    // redacted here rather than at each place that shows it: the same string is
    // printed by this function, and then again by the retry message, the blame
    // line, and the final refusal.
    let reason = "no probe is wired for postgres://clio:hunter2-secret@db.example.test:5432/clio";
    let mut out = Vec::new();
    let error = run(&mut out, plain(), "the database", || Err(reason.to_owned()))
        .expect_err("the probe failed");
    let written = String::from_utf8_lossy(&out).into_owned();
    assert!(!written.contains("hunter2-secret"), "{written}");
    assert!(!error.contains("hunter2-secret"), "{error}");
    // The address itself is what makes the message actionable, so it stays.
    assert!(written.contains("db.example.test:5432"), "{written}");
    assert!(error.contains("db.example.test:5432"), "{error}");
}

#[test]
fn a_success_detail_never_carries_the_password_either() {
    // The success string reaches the preview as a `Tested` line, and the address
    // is the one thing worth keeping in it.
    let mut out = Vec::new();
    let report = run(&mut out, plain(), "the database", || {
        Ok("connected to postgres://clio:hunter2-secret@db.example.test:5432/clio".to_owned())
    })
    .expect("the probe succeeded");
    assert!(!report.contains("hunter2-secret"), "{report}");
}

#[test]
fn the_attempt_is_written_before_the_probe_runs() {
    // A probe that takes a while must not leave the operator looking at an
    // empty line, so the attempt is flushed first and the probe can already
    // read it.
    let mut out = Shared::default();
    let reader = out.clone();
    run(&mut out, plain(), "the endpoint", move || {
        assert_eq!(reader.written(), "Testing the endpoint ...");
        Ok("answered".to_owned())
    })
    .expect("the probe succeeded");
    assert_eq!(out.written(), "Testing the endpoint ... ok\n");
}

#[test]
fn the_reported_name_is_the_one_it_was_given() {
    let mut out = Vec::new();
    run(&mut out, plain(), "embedding endpoint", || {
        Ok("answered".to_owned())
    })
    .expect("the probe succeeded");
    assert!(
        String::from_utf8_lossy(&out).contains("Testing embedding endpoint ..."),
        "{}",
        String::from_utf8_lossy(&out)
    );
}

#[test]
fn a_long_refusal_is_not_truncated() {
    let reason = "db error: FATAL: password authentication failed for user \"clio\" \
                  (SQLSTATE 28P01)";
    let mut out = Vec::new();
    run(&mut out, plain(), "the database", || Err(reason.to_owned()))
        .expect_err("the probe failed");
    assert!(
        String::from_utf8_lossy(&out).contains("SQLSTATE 28P01"),
        "the whole reason must reach the operator"
    );
}
