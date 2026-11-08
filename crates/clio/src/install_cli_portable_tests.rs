// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the portable-only install flow: a call with no
//! `--harness` writes the portable skill package and skips registration, an
//! unknown harness writes nothing, and a selected harness still registers.
//!
//! ## Owns
//! - The `Decision::Portable` routing and the registration skip
//! - The exit codes for a portable write failure and an unknown harness
//!
//! ## Does not own
//! - The shared parse-and-decide step and the delegate outcome mapping
//!   (`install_cli_tests`)
//! - The asset writing itself (`skill_install_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the no-harness and refusal flows.

use super::*;
use crate::agent_harness_targets::{DirectWriteEnv, OsLayout};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_owned()).collect()
}

fn fake_exe() -> PathBuf {
    PathBuf::from("clio")
}

fn never_install(_: &HarnessRecord, _: &Path) -> Result<String, DelegateError> {
    panic!("the install delegate must not run");
}

fn never_uninstall(_: &HarnessRecord) -> Result<String, DelegateError> {
    panic!("the uninstall delegate must not run");
}

/// A harmless environment for a test whose skill step ignores it. It names no
/// real home directory and touches no file.
fn unused_env() -> DirectWriteEnv {
    DirectWriteEnv::with(
        OsLayout::Linux,
        PathBuf::from("unused-home"),
        None,
        None,
        None,
    )
}

#[test]
fn install_with_no_harness_returns_a_portable_decision() {
    let decision = decide(Verb::Install, &args(&[])).expect("no --harness is the portable install");
    assert!(matches!(decision, Decision::Portable), "{decision:?}");
}

#[test]
fn uninstall_with_no_harness_is_a_usage_error_before_the_delegate_runs() {
    let code = run_with(
        Verb::Uninstall,
        &args(&[]),
        &|| panic!("uninstall must not resolve the executable"),
        &unused_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(2),
        "a missing --harness on uninstall must fail with the usage code"
    );
}

#[test]
fn uninstall_missing_harness_usage_error_lists_every_supported_harness() {
    let error = decide(Verb::Uninstall, &args(&[])).expect_err("uninstall requires --harness");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    let hint = error.hint().unwrap_or_default();
    assert!(
        hint.contains(
            "supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"
        ),
        "the hint must name the exact registry list: {hint:?}"
    );
}

#[test]
fn install_with_no_harness_writes_the_portable_package_and_skips_registration() {
    let calls = RefCell::new(Vec::new());
    let skill = |record: Option<&HarnessRecord>, _: &DirectWriteEnv| {
        calls
            .borrow_mut()
            .push(record.map_or("portable", |record| record.name));
        Ok(())
    };
    let code = run_with_skills(
        Verb::Install,
        &args(&[]),
        &|| panic!("the portable install must not resolve the executable"),
        &unused_env,
        &never_install,
        &never_uninstall,
        &skill,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(*calls.borrow(), vec!["portable"]);
}

#[test]
fn a_portable_skill_failure_exits_1_and_skips_registration() {
    let skill = |_: Option<&HarnessRecord>, _: &DirectWriteEnv| {
        Err("could not install the portable clio-mcp skill assets: blocked".to_owned())
    };
    let code = run_with_skills(
        Verb::Install,
        &args(&[]),
        &|| panic!("a failed portable install must not resolve the executable"),
        &unused_env,
        &never_install,
        &never_uninstall,
        &skill,
    );
    assert_eq!(code, ExitCode::from(1));
}

#[test]
fn an_unknown_harness_writes_no_skill_and_registers_nothing() {
    let skill = |_: Option<&HarnessRecord>, _: &DirectWriteEnv| {
        panic!("the skill step must not run for an unknown harness")
    };
    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "vim"]),
        &|| panic!("an unknown harness must fail before the executable lookup"),
        &unused_env,
        &never_install,
        &never_uninstall,
        &skill,
    );
    assert_eq!(code, ExitCode::from(2));
}

#[test]
fn a_selected_harness_runs_the_skill_step_then_registers() {
    let order = RefCell::new(Vec::new());
    let skill = |record: Option<&HarnessRecord>, _: &DirectWriteEnv| {
        let record = record.expect("a selected harness reaches the skill step");
        order.borrow_mut().push(format!("skill {}", record.name));
        Ok(())
    };
    let install = |record: &HarnessRecord, _: &Path| -> Result<String, DelegateError> {
        order.borrow_mut().push(format!("register {}", record.name));
        Ok("registered".to_owned())
    };
    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(fake_exe()),
        &unused_env,
        &install,
        &never_uninstall,
        &skill,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(*order.borrow(), vec!["skill claude", "register claude"]);
}
