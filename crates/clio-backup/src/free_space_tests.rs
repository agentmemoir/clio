// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the verify pass's free-space gate:
//! the `df` report parser and every arm of the check that can be exercised
//! without a real filesystem.
//!
//! ## Owns
//! - The parser assertions and the injected `df` outcomes
//!
//! ## Does not own
//! - The call that runs `df` itself (see `free_space`)
//!
//! ## Boundary
//!
//! Keep this file focused on how the free-space gate answers.

use super::*;
use std::process::{ExitStatus, Output};

/// One `df -P -k` outcome.
fn output(code: i32, stdout: &str) -> Output {
    Output {
        status: exit_status(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    ExitStatus::from_raw(code)
}

#[cfg(not(unix))]
fn exit_status(code: i32) -> ExitStatus {
    let mut command = std::process::Command::new("cmd");
    command.arg("/C").arg(format!("exit {code}"));
    command.status().expect("the test host runs its own shell")
}

/// A POSIX report: header, then filesystem, 1024-blocks, used, available,
/// capacity, mount point.
const REPORT: &str = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/disk1 100000 40000 60000 40% /\n";

#[test]
fn a_posix_report_names_the_available_blocks() {
    assert_eq!(parse_available_kb(REPORT), Some(60_000));
}

#[test]
fn a_report_without_a_data_line_has_no_answer() {
    assert_eq!(
        parse_available_kb("Filesystem 1024-blocks Used Available\n"),
        None
    );
}

#[test]
fn a_report_with_a_non_numeric_available_column_has_no_answer() {
    let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk1 1 2 ? 3% /\n";
    assert_eq!(parse_available_kb(text), None);
}

#[test]
fn a_successful_df_answers_in_bytes() {
    let bytes = run_available_bytes(Path::new("/"), &|_| Ok(output(0, REPORT))).expect("an answer");
    assert_eq!(bytes, 60_000 * 1024);
}

#[test]
fn a_failing_df_is_an_internal_error_not_room() {
    let error = run_available_bytes(Path::new("/"), &|_| Ok(output(1, ""))).expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("`df` exited"), "{}", error.message);
}

#[test]
fn a_df_that_cannot_run_is_an_internal_error() {
    let error = run_available_bytes(Path::new("/"), &|_| {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
    })
    .expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("gone"), "{}", error.message);
}

#[test]
fn an_unreadable_df_report_is_an_internal_error() {
    let error = run_available_bytes(Path::new("/"), &|_| Ok(output(0, "garbage\n")))
        .expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
}

#[test]
fn no_payload_bytes_needs_no_check() {
    require_room(Path::new("/definitely/not/a/directory"), 0).expect("nothing to reserve");
}
