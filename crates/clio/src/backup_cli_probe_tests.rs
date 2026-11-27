// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio backup dir` writability probe: that it claims a
//! fresh name, and that an entry already sitting at the name it picks is left
//! exactly as it was found.
//!
//! ## Owns
//! - Behavior tests for the exclusive claim the probe makes on its own name
//!
//! ## Does not own
//! - The probe's place in the `backup dir` command (see `backup_cli_tests`)
//! - Directory creation and resolution (see `backup_cli` and `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::setup_support_tests::temp_dir;

const PLANTED: &str = "a file this probe must not touch\n";

/// A name under `dir` shaped like the one the probe builds, so a test plants an
/// entry at exactly the path the probe is handed.
fn probe_shaped(dir: &Path, tag: &str) -> PathBuf {
    dir.join(format!(
        ".clio-backup-dir-probe-{}-{tag}",
        std::process::id()
    ))
}

#[test]
fn probe_claims_a_fresh_name_and_removes_it() {
    let dir = temp_dir("probe-fresh");
    let probe = probe_shaped(&dir, "fresh");
    assert!(
        claim_probe_name(&probe),
        "a writable directory accepts the probe"
    );
    assert!(!probe.exists(), "the probe leaves no entry behind");
}

#[test]
fn probe_reports_not_writable_and_leaves_an_existing_file_intact() {
    let dir = temp_dir("probe-file");
    let probe = probe_shaped(&dir, "file");
    std::fs::write(&probe, PLANTED).expect("plant a file at the probe name");
    assert!(
        !claim_probe_name(&probe),
        "a taken name is reported as not writable"
    );
    assert_eq!(
        std::fs::read_to_string(&probe).expect("read the planted file"),
        PLANTED,
        "the planted file keeps its contents"
    );
}

#[cfg(unix)]
#[test]
fn probe_reports_not_writable_and_never_follows_a_planted_symlink() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("probe-link");
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, PLANTED).expect("write the link target");
    let probe = probe_shaped(&dir, "link");
    symlink(&victim, &probe).expect("plant a link at the probe name");
    assert!(
        !claim_probe_name(&probe),
        "a name held by a symlink is reported as not writable"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).expect("read the link target"),
        PLANTED,
        "the file behind the link keeps its contents"
    );
    assert!(
        std::fs::symlink_metadata(&probe).is_ok(),
        "the planted link is left in place"
    );
}
