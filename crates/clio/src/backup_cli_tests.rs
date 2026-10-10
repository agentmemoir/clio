// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio backup` surface: directory creation and
//! permissions, the env override, the writability probe, group help, and the
//! top-level dispatch.
//!
//! ## Owns
//! - Behavior and view tests for the `backup` verbs and help surfaces
//!
//! ## Does not own
//! - The command implementation under test (`backup_cli`)
//! - Directory resolution behavior (see `clio-config` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, run_backup};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

use serde_json::Value;

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat").mode() & 0o777
}

/// Root ignores mode bits, so the unwritable-directory assertion only runs for
/// non-root users. `/proc/self/status` exists on Linux; where it is absent the
/// assertion runs anyway.
fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .map(|uids| uids.split_whitespace().next() == Some("0"))
        })
        .unwrap_or(false)
}

#[test]
fn cli_t10_backup_dir_creates_the_fallback_dir_owner_only() {
    let scratch = Scratch::new("t10");
    let cwd = scratch.path().to_path_buf();
    let (code, out, err) = run_backup(&["dir"], &[], Ok(cwd.clone()), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let created = cwd.join("clio-backups");
    assert!(created.is_dir());
    assert!(out.contains(&created.display().to_string()), "{out}");
    assert!(out.contains("current-directory fallback"), "{out}");
    assert!(out.contains("created"), "{out}");
    assert!(out.contains("writable"), "{out}");
    assert!(!out.contains("not writable"), "{out}");
    #[cfg(unix)]
    assert_eq!(mode_of(&created), 0o700);
    // A second run finds the directory and leaves its mode alone.
    let (code, out, _) = run_backup(&["dir"], &[], Ok(cwd), true);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("existed"), "{out}");
    #[cfg(unix)]
    assert_eq!(mode_of(&created), 0o700);
}

#[test]
fn cli_t10b_env_override_respected_and_existing_mode_untouched() {
    let scratch = Scratch::new("t10b");
    let vault = scratch.path().join("vault");
    std::fs::create_dir_all(&vault).expect("pre-create vault");
    #[cfg(unix)]
    set_mode(&vault, 0o755);
    let override_str = vault.display().to_string();
    let env = [("CLIO_BACKUP_DIR", override_str.as_str())];
    let (code, out, err) = run_backup(&["dir"], &env, Ok(scratch.path().to_path_buf()), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(
        payload["tier"],
        json!("CLIO_BACKUP_DIR override"),
        "{payload}"
    );
    assert_eq!(
        payload["backup_dir"],
        json!(vault.display().to_string()),
        "{payload}"
    );
    assert_eq!(payload["created"], json!(false), "{payload}");
    assert_eq!(payload["writable"], json!(true), "{payload}");
    #[cfg(unix)]
    assert_eq!(mode_of(&vault), 0o755);
}

#[test]
fn cli_t10c_unwritable_directory_reports_not_writable() {
    if running_as_root() {
        return;
    }
    let scratch = Scratch::new("t10c");
    let vault = scratch.path().join("vault");
    std::fs::create_dir_all(&vault).expect("pre-create vault");
    #[cfg(unix)]
    set_mode(&vault, 0o555);
    let override_str = vault.display().to_string();
    let env = [("CLIO_BACKUP_DIR", override_str.as_str())];
    let cwd = Ok(scratch.path().to_path_buf());
    let (code, out, err) = run_backup(&["dir"], &env, cwd.clone(), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("not writable"), "{out}");
    let (code, out, _) = run_backup(&["dir"], &env, cwd, false);
    assert_eq!(code, ExitCode::SUCCESS);
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["writable"], json!(false), "{payload}");
    #[cfg(unix)]
    set_mode(&vault, 0o700);
}

#[test]
fn cli_t10d_create_failure_is_an_internal_error() {
    let scratch = Scratch::new("t10d");
    let blocked = scratch.path().join("vault");
    std::fs::write(&blocked, b"not a directory").expect("write blocker file");
    let override_str = blocked.display().to_string();
    let env = [("CLIO_BACKUP_DIR", override_str.as_str())];
    let (code, _, err) = run_backup(&["dir"], &env, Ok(scratch.path().to_path_buf()), false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("cannot create backup directory"), "{err}");
    assert!(err.contains("vault"), "{err}");
}

#[test]
fn cli_t10e_current_directory_failure_is_an_internal_error() {
    let error = current_dir_error(&std::io::Error::other("boom"));
    assert_eq!(error.code(), "internal");
    let (code, _, err) = run_backup(&["dir"], &[], Err(error), false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("current directory"), "{err}");
}

#[test]
fn cli_t11_no_subcommand_prints_the_group_and_exits_2() {
    let scratch = Scratch::new("t11");
    let (code, out, _) = run_backup(&[], &[], Ok(scratch.path().to_path_buf()), true);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.contains("usage: clio backup <subcommand>"), "{out}");
    assert!(out.contains("clio backup dir"), "{out}");
    assert!(out.contains("(no MCP tool)"), "{out}");
}

#[test]
fn cli_t11b_unknown_subcommand_suggests_dir() {
    let scratch = Scratch::new("t11b");
    let cwd = Ok(scratch.path().to_path_buf());
    let (code, _, err) = run_backup(&["dirt"], &[], cwd.clone(), false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("did you mean `clio backup dir`?"), "{err}");
    let (code, _, err) = run_backup(&["bogus"], &[], cwd, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("try `clio backup dir`"), "{err}");
}

#[test]
fn cli_t12_json_view_matches_the_text_view() {
    let scratch = Scratch::new("t12");
    let cwd = scratch.path().to_path_buf();
    let expected = cwd.join("clio-backups").display().to_string();
    let (code, out, err) = run_backup(&["dir", "--output", "json"], &[], Ok(cwd.clone()), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["backup_dir"], json!(expected), "{payload}");
    assert_eq!(
        payload["tier"],
        json!("current-directory fallback"),
        "{payload}"
    );
    assert_eq!(payload["created"], json!(true), "{payload}");
    assert_eq!(payload["writable"], json!(true), "{payload}");
    // The piped default is JSON, same payload without an explicit flag.
    let (code, out, _) = run_backup(&["dir"], &[], Ok(cwd.clone()), false);
    assert_eq!(code, ExitCode::SUCCESS);
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(payload["backup_dir"], json!(expected), "{payload}");
    // The TTY default is one text line with the same facts.
    let (code, text, _) = run_backup(&["dir"], &[], Ok(cwd), true);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(text.contains(&format!("backup dir {expected}")), "{text}");
    assert!(text.contains("existed"), "{text}");
}

#[test]
fn cli_t12b_bad_flags_and_extra_args_fail_closed() {
    let scratch = Scratch::new("t12b");
    let cwd = Ok(scratch.path().to_path_buf());
    let (code, _, err) = run_backup(&["dir", "--frobnicate"], &[], cwd.clone(), false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(!err.is_empty(), "{err:?}");
    let (code, _, err) = run_backup(&["dir", "--output", "yaml"], &[], cwd.clone(), false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run_backup(&["dir", "extra"], &[], cwd, false);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

#[test]
fn cli_t13_no_database_is_needed_or_opened() {
    let scratch = Scratch::new("t13");
    let data = scratch.path().join("data");
    let data_str = data.display().to_string();
    let env = [
        ("CLIO_DATA_DIR", data_str.as_str()),
        ("CLIO_DATABASE_URL", "postgres://127.0.0.1:9/nosuchdb"),
    ];
    let (code, out, err) = run_backup(&["dir"], &env, Ok(scratch.path().to_path_buf()), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(data.join("backups").is_dir());
    let payload: Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(
        payload["tier"],
        json!("data directory default"),
        "{payload}"
    );
    assert_eq!(payload["created"], json!(true), "{payload}");
    assert_eq!(payload["writable"], json!(true), "{payload}");
}

#[test]
fn cli_t14_group_help_marks_unbound_subcommands() {
    let help = cli_help::group_help("backup", &["dir"]);
    assert!(help.contains("clio backup dir   (no MCP tool)"), "{help}");
    assert!(!help.contains("->"), "{help}");
    // Bound subcommands keep the arrow rendering, and the sync group's two
    // CLI-only printing verbs are marked instead: they bind no MCP tool by the
    // reviewed exception, which is why the group is no longer arrow-only.
    let subs = cli_help::group_subcommands("sync").unwrap_or_default();
    let help = cli_help::group_help("sync", subs);
    assert!(help.contains("clio sync status   -> sync_status"), "{help}");
    assert!(help.contains("clio sync keygen   (no MCP tool)"), "{help}");
    assert!(
        help.contains("clio sync key export   (no MCP tool)"),
        "{help}"
    );
    assert!(
        help.contains("clio sync key status   -> sync_key_status"),
        "{help}"
    );
}

#[test]
fn cli_t15_dispatch_routes_the_backup_group() {
    // The real entry with no subcommand prints the group and exits 2. Running
    // `backup dir` through the real entry would use the process cwd, which
    // tests cannot isolate safely, so the injected-`run` tests own that path.
    assert_eq!(crate::run(vec!["backup".to_owned()]), ExitCode::from(2));
    assert_eq!(
        crate::run(vec!["backup".to_owned(), "--help".to_owned()]),
        ExitCode::SUCCESS
    );
}
