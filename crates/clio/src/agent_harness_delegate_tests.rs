// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the delegate runner's scenarios. Every test injects a
//! temp home directory and, for spawn tests, a stub `claude`, so no test
//! reads the host's real `PATH`, `HOME`, or Claude config. The stub records
//! the argument vector it received; the tests assert that vector, never
//! vendor prose. The stub lives in a directory that already ran a probe, or
//! under `NOEXEC_EXEC_DIR`, because a temp directory can be mounted noexec
//! and a stub there cannot run; a directory that refuses the probe is an
//! error, never a skip. Spawn tests need a Unix shell and exist only there.
//!
//! ## Owns
//! - The assertions over delegate outcomes and recorded arguments, for
//!   install and uninstall
//! - The shared stub and fixture harness lives in
//!   `agent_harness_delegate_test_support_tests`
//!
//! ## Does not own
//! - The delegate implementation under test (`agent_harness_delegate`)
//! - The harness registry the tests resolve records from (`agent_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::test_support_tests::{Fixture, expected_args, record};
use super::*;
use crate::agent_harness::{Mechanism, SERVER_NAME};
use std::fs;

#[test]
fn install_without_the_binary_names_it_and_prints_the_snippet() {
    let fixture = Fixture::new("binary-missing");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &fixture.empty_env())
        .expect_err("a missing binary must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "claude");
    assert_eq!(
        manual_snippet,
        record().manual_snippet(SERVER_NAME, &fixture.exe())
    );
}

#[test]
fn uninstall_without_the_binary_refuses_with_the_remove_snippet() {
    let fixture = Fixture::new("uninstall-binary-missing");
    let err = uninstall(record(), SERVER_NAME, &fixture.empty_env())
        .expect_err("a missing binary must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "claude");
    assert_eq!(manual_snippet, "claude mcp remove clio --scope user");
}

#[cfg(unix)]
#[test]
fn install_runs_the_registry_argv_and_reports_installed() {
    let mut fixture = Fixture::new("install-clean");
    fixture.write_config(&fixture.exe());
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let message = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect("a clean exit and a matching entry must install");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&record().add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn install_reports_already_installed_when_the_entry_survives_a_duplicate_exit() {
    let mut fixture = Fixture::new("install-duplicate");
    fixture.write_config(&fixture.exe());
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    let message = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect("an entry already present is a success");
    assert_eq!(message, "already installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&record().add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn install_verifies_an_entry_the_stub_wrote_itself() {
    let mut fixture = Fixture::new("install-stub-writes");
    let env = fixture
        .stub_env_vendor()
        .expect("the stub needs a directory that permits exec");
    let message = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect("an entry the stub wrote itself must verify");
    assert_eq!(message, "installed");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&record().add_argv(SERVER_NAME, &fixture.exe()))
    );
}

#[cfg(unix)]
#[test]
fn install_rewrites_a_registration_that_points_elsewhere() {
    let mut fixture = Fixture::new("install-rewrite");
    fixture.write_config(&fixture.root.join("old"));
    let env = fixture
        .stub_env_vendor()
        .expect("the stub needs a directory that permits exec");
    let message = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect("a stale registration must be removed and added again");
    assert_eq!(message, "installed");
    let mut both = expected_args(&record().add_argv(SERVER_NAME, &fixture.exe()));
    both.extend(expected_args(&record().remove_argv(SERVER_NAME)));
    both.extend(expected_args(
        &record().add_argv(SERVER_NAME, &fixture.exe()),
    ));
    assert_eq!(
        fixture.recorded_args(),
        both,
        "the stub ran the refused add, then remove, then add"
    );
}

#[cfg(unix)]
#[test]
fn install_refuses_a_zero_exit_when_no_entry_lands() {
    let mut fixture = Fixture::new("install-zero-lies");
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("the read-back must not trust a zero exit");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn install_reports_command_failed_when_a_duplicate_exit_leaves_no_entry() {
    let mut fixture = Fixture::new("install-duplicate-refused");
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("a refused duplicate add with no entry is a failure");
    let DelegateError::CommandFailed { exit_code, .. } = err else {
        panic!("expected CommandFailed, got {err:?}")
    };
    assert_eq!(exit_code, 1);
}

#[cfg(unix)]
#[test]
fn install_refuses_a_zero_exit_when_the_entry_points_elsewhere() {
    let mut fixture = Fixture::new("install-elsewhere");
    fixture.write_config(&fixture.root.join("other"));
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("an entry under another executable is not installed");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn install_refuses_a_malformed_config_without_panicking() {
    let mut fixture = Fixture::new("install-garbage");
    fixture.write_garbage_config();
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("a malformed config cannot be verified");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn install_lets_the_read_back_decide_an_unknown_exit_code() {
    let mut fixture = Fixture::new("install-unknown-exit");
    fixture.write_config(&fixture.exe());
    let env = fixture
        .stub_env(3)
        .expect("the stub needs a directory that permits exec");
    let message = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect("the entry is present, whatever the vendor said");
    assert_eq!(message, "already installed");
}

#[cfg(unix)]
#[test]
fn uninstall_runs_the_registry_remove_argv() {
    let mut fixture = Fixture::new("uninstall-argv");
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    uninstall(record(), SERVER_NAME, &env).expect("an absent entry is a success for uninstall");
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&record().remove_argv(SERVER_NAME))
    );
}

#[cfg(unix)]
#[test]
fn uninstall_reports_removed_after_a_clean_exit() {
    let mut fixture = Fixture::new("uninstall-clean");
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let message = uninstall(record(), SERVER_NAME, &env)
        .expect("an entry gone after a clean exit is removed");
    assert_eq!(message, "removed");
}

#[cfg(unix)]
#[test]
fn uninstall_reports_nothing_to_remove_when_the_vendor_refuses_an_absent_entry() {
    let mut fixture = Fixture::new("uninstall-absent");
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    let message = uninstall(record(), SERVER_NAME, &env)
        .expect("a missing-remove exit with no entry is a success");
    assert_eq!(message, "nothing to remove");
}

#[cfg(unix)]
#[test]
fn uninstall_refuses_a_zero_exit_when_the_entry_still_present() {
    let mut fixture = Fixture::new("uninstall-zero-lies");
    fixture.write_config(&fixture.exe());
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let err = uninstall(record(), SERVER_NAME, &env)
        .expect_err("the read-back must not trust a zero exit");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn uninstall_reports_command_failed_when_the_vendor_refuses_and_the_entry_stays() {
    let mut fixture = Fixture::new("uninstall-refused");
    fixture.write_config(&fixture.exe());
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    let err = uninstall(record(), SERVER_NAME, &env)
        .expect_err("a refused removal with the entry still present is a failure");
    let DelegateError::CommandFailed { exit_code, .. } = err else {
        panic!("expected CommandFailed, got {err:?}")
    };
    assert_eq!(exit_code, 1);
}

#[test]
fn production_env_names_the_config_file_after_the_record() {
    let env = DelegateEnv::production();
    let path = env.config_path(record());
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(record().config_file),
        "the real environment still uses the record's config file name"
    );
}

#[test]
fn lookup_on_path_misses_a_binary_no_host_ships() {
    assert_eq!(
        lookup_on_path("clio-no-such-binary-anywhere"),
        None,
        "a binary absent from every PATH entry is not found"
    );
}

#[cfg(unix)]
#[test]
fn is_runnable_follows_the_exec_bits_of_a_real_file() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("is-runnable");
    let script = fixture.root.join("runnable");
    fs::write(&script, "#!/bin/sh\n").expect("the script can be written");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
        .expect("the script can be made executable");
    assert!(is_runnable(&script), "an executable file is runnable");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644))
        .expect("the exec bits can be removed");
    assert!(
        !is_runnable(&script),
        "a file without exec bits is not runnable"
    );
    assert!(
        !is_runnable(&fixture.root.join("missing")),
        "a missing file is not runnable"
    );
}

#[test]
fn a_harness_without_a_vendor_command_refuses_without_spawning() {
    let fixture = Fixture::new("empty-argv");
    let unknown = HarnessRecord {
        name: "unknown-harness",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".unknown-harness.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    };
    let err = install(&unknown, SERVER_NAME, &fixture.exe(), &fixture.empty_env())
        .expect_err("a record with no vendor command must refuse");
    let DelegateError::VerificationFailed { detail } = err else {
        panic!("expected VerificationFailed, got {err:?}")
    };
    assert!(
        detail.contains("no vendor command"),
        "the refusal names the missing command: {detail}"
    );
}

#[test]
fn a_binary_that_resolves_but_cannot_run_refuses_with_the_snippet() {
    let fixture = Fixture::new("unspawnable");
    let stand_in = fixture.root.join("not-an-executable");
    fs::create_dir_all(&stand_in).expect("the stand-in path can be created");
    let env = fixture.env_with_lookup(move |_| Some(stand_in.clone()));
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("a lookup hit that cannot be spawned must refuse");
    let DelegateError::BinaryMissing {
        binary,
        manual_snippet,
    } = err
    else {
        panic!("expected BinaryMissing, got {err:?}")
    };
    assert_eq!(binary, "claude");
    assert_eq!(
        manual_snippet,
        record().manual_snippet(SERVER_NAME, &fixture.exe())
    );
}

#[cfg(unix)]
#[test]
fn install_refuses_when_the_config_file_cannot_be_read_at_all() {
    let mut fixture = Fixture::new("config-unreadable");
    let config = fixture.root.join("home").join(record().config_file);
    fs::create_dir_all(&config).expect("the config path can become a directory");
    let env = fixture
        .stub_env(0)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("an unreadable config cannot be verified");
    assert!(
        matches!(err, DelegateError::VerificationFailed { .. }),
        "{err:?}"
    );
}

#[cfg(unix)]
#[test]
fn install_reports_command_failed_when_a_stale_entry_survives_the_repair() {
    let mut fixture = Fixture::new("repair-remove-stuck");
    fixture.write_config(&fixture.root.join("old"));
    let env = fixture
        .stub_env(1)
        .expect("the stub needs a directory that permits exec");
    let err = install(record(), SERVER_NAME, &fixture.exe(), &env)
        .expect_err("a refused add under a foreign entry must attempt the repair");
    let DelegateError::CommandFailed { exit_code, .. } = err else {
        panic!("expected CommandFailed, got {err:?}")
    };
    assert_eq!(exit_code, 1);
    assert_eq!(
        fixture.recorded_args(),
        expected_args(&record().remove_argv(SERVER_NAME)),
        "the repair's last command was the stale remove"
    );
}
