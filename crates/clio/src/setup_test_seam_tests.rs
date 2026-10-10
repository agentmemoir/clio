// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the injectable probe seam: that a scripted outcome is what
//! the wizard sees, with no server involved.
//!
//! ## Owns
//! - A scripted database outcome reaching the caller
//! - A scripted endpoint outcome reaching the caller, with its key
//!
//! ## Does not own
//! - The real probes, which stage 6 builds
//!
//! ## Boundary
//!
//! Tests here break when the seam stops passing a caller's answer through;
//! they never change the implementation.
//!
//! Keep this module focused on the seam.

use super::*;

#[test]
fn a_scripted_database_success_reaches_the_caller() {
    let probes = Probes::new(
        |address: &str| Ok(format!("scripted: {address}")),
        |_, _| Ok("answered".to_owned()),
    );
    assert_eq!(
        probes.test_database("postgres://h/db").expect("scripted"),
        "scripted: postgres://h/db"
    );
}

#[test]
fn a_scripted_database_refusal_reaches_the_caller() {
    let probes = Probes::new(
        |_: &str| Err("connection refused".to_owned()),
        |_, _| Ok("answered".to_owned()),
    );
    let error = probes
        .test_database("postgres://h/db")
        .expect_err("scripted");
    assert_eq!(error, "connection refused");
}

#[test]
fn a_scripted_endpoint_sees_its_key() {
    let probes = Probes::new(
        |_: &str| Ok("ok".to_owned()),
        |url, key| Ok(format!("{url}|{}", key.unwrap_or("none"))),
    );
    assert_eq!(
        probes
            .test_endpoint("https://api.example.com", Some("sk-test"))
            .expect("scripted"),
        "https://api.example.com|sk-test"
    );
}

#[test]
fn a_scripted_endpoint_reports_no_key_as_none() {
    let probes = Probes::new(
        |_: &str| Ok("ok".to_owned()),
        |url, key| Ok(format!("{url}|{}", key.unwrap_or("none"))),
    );
    assert_eq!(
        probes
            .test_endpoint("http://127.0.0.1:8080", None)
            .expect("scripted"),
        "http://127.0.0.1:8080|none"
    );
}

#[test]
fn a_scripted_endpoint_refusal_reaches_the_caller() {
    let probes = Probes::new(
        |_: &str| Ok("ok".to_owned()),
        |_, _| Err("connection refused".to_owned()),
    );
    let error = probes
        .test_endpoint("https://api.example.com", None)
        .expect_err("scripted");
    assert_eq!(error, "connection refused");
}
