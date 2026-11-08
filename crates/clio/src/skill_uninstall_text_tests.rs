// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill remover's text: the removed, retained, and no-op outcome lines, the refusal for a foreign installer, the refusal for every local reason, and the plain failure message for an IO error.
//!
//! ## Owns
//! - Every assertion over `outcome_text` and `failure_message`, built from literal values rather than a filesystem
//!
//! ## Does not own
//! - The text implementation under test (`skill_uninstall_text`)
//! - The decision to remove, retain, or refuse a directory (`skill_uninstall`)
//!
//! ## Boundary
//!
//! Test-only. Every input is a constructed value, so no test captures stdout, touches a file, or reads the host environment.
//!
//! Keep this module focused on the uninstall text, with literal inputs only.

use super::{failure_message, outcome_text};
use crate::skill_install_plan::ConflictReason;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall::{RefusalReason, RemovalOutcome, UninstallError};
use std::collections::BTreeSet;
use std::path::PathBuf;

const PACKAGE_DIR: &str = "/home/me/.claude/skills/clio-mcp";
const CLOSING: &str = "nothing was removed; clio removes only a package whose marker records installer clio, the payload digest, and every file's bytes";

fn package_dir() -> PathBuf {
    PathBuf::from(PACKAGE_DIR)
}

fn refused(reasons: Vec<RefusalReason>) -> String {
    failure_message(&UninstallError::Refused {
        dir: package_dir(),
        reasons,
    })
}

fn header(count: usize) -> String {
    format!(
        "refused to remove the clio-mcp skill package because {count} recorded facts do not match:"
    )
}

#[test]
fn a_removed_outcome_names_the_directory_the_file_count_and_the_directory_removal() {
    let outcome = RemovalOutcome::Removed {
        dir: package_dir(),
        files: vec![String::from("SKILL.md"), String::from("references/a.md")],
        removed_dir: true,
    };
    assert_eq!(
        outcome_text(&[outcome]),
        "removed the clio-mcp skill package from /home/me/.claude/skills/clio-mcp; removed 2 files and the marker; removed the package directory"
    );
}

#[test]
fn a_removed_outcome_leaves_out_the_directory_sentence_when_the_directory_stays() {
    let outcome = RemovalOutcome::Removed {
        dir: package_dir(),
        files: vec![String::from("SKILL.md")],
        removed_dir: false,
    };
    assert_eq!(
        outcome_text(&[outcome]),
        "removed the clio-mcp skill package from /home/me/.claude/skills/clio-mcp; removed 1 file and the marker"
    );
}

#[test]
fn a_retained_outcome_names_one_remaining_harness() {
    let outcome = RemovalOutcome::Retained {
        dir: package_dir(),
        remaining: BTreeSet::from([String::from("claude")]),
    };
    assert_eq!(
        outcome_text(&[outcome]),
        "retained the clio-mcp skill package in /home/me/.claude/skills/clio-mcp; still referenced by: claude"
    );
}

#[test]
fn a_retained_outcome_names_every_remaining_harness_in_order() {
    let outcome = RemovalOutcome::Retained {
        dir: package_dir(),
        remaining: BTreeSet::from([String::from("hermes"), String::from("codex")]),
    };
    assert_eq!(
        outcome_text(&[outcome]),
        "retained the clio-mcp skill package in /home/me/.claude/skills/clio-mcp; still referenced by: codex, hermes"
    );
}

#[test]
fn a_no_op_outcome_says_nothing_was_there() {
    let outcome = RemovalOutcome::NoOp { dir: package_dir() };
    assert_eq!(
        outcome_text(&[outcome]),
        "no clio-mcp skill package in /home/me/.claude/skills/clio-mcp; nothing to remove"
    );
}

#[test]
fn a_foreign_installer_refusal_names_the_recorded_program_and_not_clio() {
    let text = refused(vec![RefusalReason::ForeignInstaller {
        recorded: String::from("other-tool"),
    }]);
    assert_eq!(
        text,
        "refused to remove /home/me/.claude/skills/clio-mcp: its marker records installer `other-tool`, not `clio`"
    );
}

#[test]
fn every_local_refusal_reason_prints_its_path_and_its_own_words() {
    let cases = [
        (
            RefusalReason::Conflict(ConflictReason::MissingOwned {
                path: String::from("references/a.md"),
            }),
            "missing owned file",
        ),
        (
            RefusalReason::Conflict(ConflictReason::LocalEdit {
                path: String::from("SKILL.md"),
            }),
            "local edit",
        ),
        (
            RefusalReason::Conflict(ConflictReason::UnexpectedFile {
                path: String::from("notes.txt"),
            }),
            "unexpected file",
        ),
        (
            RefusalReason::Conflict(ConflictReason::EarlierInstall {
                path: String::from("SKILL.md"),
            }),
            "installed by an earlier clio without a marker",
        ),
        (
            RefusalReason::Conflict(ConflictReason::UnreadableMarker {
                path: String::from(".clio-skill.json"),
            }),
            "unreadable marker",
        ),
        (
            RefusalReason::Symlink {
                path: String::from("references/a.md"),
            },
            "symbolic link",
        ),
    ];
    for (reason, words) in cases {
        let line = format!("    {}: {words}", reason.path());
        assert_eq!(
            refused(vec![reason]),
            format!("{}\n{line}\n{CLOSING}", header(1))
        );
    }
}

#[test]
fn a_payload_digest_refusal_names_both_digests() {
    let text = refused(vec![RefusalReason::PayloadDigest {
        recorded: String::from("recorded-digest"),
        found: String::from("found-digest"),
    }]);
    assert_eq!(
        text,
        format!(
            "{}\n    {SKILL_MARKER_FILE}: payload digest differs (recorded recorded-digest, found found-digest)\n{CLOSING}",
            header(1)
        )
    );
}

#[test]
fn several_reasons_count_each_one_and_keep_each_line_and_the_closing_sentence() {
    let text = refused(vec![
        RefusalReason::Conflict(ConflictReason::LocalEdit {
            path: String::from("SKILL.md"),
        }),
        RefusalReason::Conflict(ConflictReason::MissingOwned {
            path: String::from("references/a.md"),
        }),
        RefusalReason::Symlink {
            path: String::from("references/b.md"),
        },
    ]);
    assert!(text.starts_with(&header(3)), "{text}");
    assert!(text.contains("\n    SKILL.md: local edit\n"), "{text}");
    assert!(
        text.contains("\n    references/a.md: missing owned file\n"),
        "{text}"
    );
    assert!(
        text.contains("\n    references/b.md: symbolic link\n"),
        "{text}"
    );
    assert!(text.ends_with(CLOSING), "{text}");
}

#[test]
fn an_io_failure_names_the_clio_mcp_package_and_the_detail() {
    let error = UninstallError::Io {
        detail: String::from("failed to remove /home/me/.claude/skills/clio-mcp: denied"),
    };
    assert_eq!(
        failure_message(&error),
        "could not remove the clio-mcp skill package: failed to remove /home/me/.claude/skills/clio-mcp: denied"
    );
}
