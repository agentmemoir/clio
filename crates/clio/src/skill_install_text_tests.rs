// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the skill installer's text: the written, unchanged, and
//! refused outcome lines, the reason text for every conflict, the portable
//! reason line, and the plain failure messages.
//!
//! ## Owns
//! - Every assertion over `outcome_text`, `portable_outcome_text`, `refused`,
//!   and the two failure messages, built from literals rather than a filesystem
//!
//! ## Does not own
//! - The text implementation under test (`skill_install_text`)
//! - The write and no-op behaviour the text describes
//!   (`skill_install_marker_tests`)
//!
//! ## Boundary
//!
//! Test-only. Every input is a literal value, so no test captures stdout,
//! touches a file, or reads the host environment.

use super::*;
use crate::agent_harness;
use std::path::PathBuf;

fn record(name: &str) -> &'static HarnessRecord {
    agent_harness::resolve(name).expect("the harness is supported")
}

fn written(dir: &str, written: &[&str], preserved: &[&str]) -> SkillRootOutcome {
    SkillRootOutcome {
        dir: PathBuf::from(dir),
        written: written.iter().map(|name| (*name).to_owned()).collect(),
        preserved_foreign: preserved.iter().map(|name| (*name).to_owned()).collect(),
    }
}

#[test]
fn an_unchanged_outcome_says_unchanged_and_names_every_root() {
    let outcome = SkillOutcome::Unchanged {
        roots: vec![PathBuf::from("/a/clio-mcp"), PathBuf::from("/b/clio-mcp")],
    };
    let text = portable_outcome_text(&outcome);
    assert_eq!(
        text,
        "18 skill assets already current in /a/clio-mcp; unchanged, nothing written\n\
         18 skill assets already current in /b/clio-mcp; unchanged, nothing written"
    );
    assert!(!refused(&outcome));
}

#[test]
fn a_written_root_names_every_file_it_wrote() {
    let outcome = SkillOutcome::Written {
        roots: vec![written(
            "/a/clio-mcp",
            &["SKILL.md", ".clio-skill.json"],
            &[],
        )],
    };
    assert_eq!(
        portable_outcome_text(&outcome),
        "installed 18 skill assets into /a/clio-mcp; wrote SKILL.md, .clio-skill.json"
    );
    assert!(!refused(&outcome));
}

#[test]
fn a_written_root_with_nothing_to_write_says_it_is_current() {
    let outcome = SkillOutcome::Written {
        roots: vec![written("/a/clio-mcp", &[], &[])],
    };
    assert!(
        portable_outcome_text(&outcome).contains("unchanged, nothing written"),
        "an empty write list must not claim a write"
    );
}

#[test]
fn preserved_foreign_files_are_reported_on_their_own_line() {
    let outcome = SkillOutcome::Written {
        roots: vec![written(
            "/a/clio-mcp",
            &["SKILL.md"],
            &["notes.md", "todo.txt"],
        )],
    };
    assert_eq!(
        portable_outcome_text(&outcome),
        "installed 18 skill assets into /a/clio-mcp; wrote SKILL.md\n\
         kept unowned files in /a/clio-mcp: notes.md, todo.txt"
    );
}

#[test]
fn a_refusal_names_every_path_and_reason_with_the_force_note() {
    let outcome = SkillOutcome::Refused {
        conflicts: vec![
            RootConflict {
                dir: PathBuf::from("/a/clio-mcp"),
                reasons: vec![
                    ConflictReason::LocalEdit {
                        path: "SKILL.md".to_owned(),
                    },
                    ConflictReason::MissingOwned {
                        path: "references/01-retrieval.md".to_owned(),
                    },
                ],
            },
            RootConflict {
                dir: PathBuf::from("/b/clio-mcp"),
                reasons: vec![
                    ConflictReason::UnexpectedFile {
                        path: "notes.md".to_owned(),
                    },
                    ConflictReason::EarlierInstall {
                        path: "other.md".to_owned(),
                    },
                ],
            },
        ],
    };
    let text = portable_outcome_text(&outcome);
    assert!(refused(&outcome));
    assert!(text.contains("because 2 skill roots hold paths"), "{text}");
    assert!(text.contains("  /a/clio-mcp:"), "{text}");
    assert!(text.contains("  /b/clio-mcp:"), "{text}");
    for line in [
        "    SKILL.md: local edit",
        "    references/01-retrieval.md: missing owned file",
        "    notes.md: unexpected file",
        "    other.md: installed by an earlier clio without a marker",
    ] {
        assert!(
            text.contains(line),
            "the refusal must carry `{line}`: {text}"
        );
    }
    assert!(
        text.contains("nothing was written in any skill root"),
        "{text}"
    );
    assert!(
        text.contains("re-run with --force to replace the files the package owns"),
        "{text}"
    );
    assert!(
        text.contains("--force replaces the files the package owns and only those"),
        "{text}"
    );
    assert!(
        text.contains("never deletes a file it does not own"),
        "{text}"
    );
    assert!(text.contains("never deletes a skill root"), "{text}");
    assert!(!text.contains("refused even with --force"), "{text}");
}

#[test]
fn a_single_conflicting_root_is_phrased_in_the_singular() {
    let outcome = SkillOutcome::Refused {
        conflicts: vec![RootConflict {
            dir: PathBuf::from("/a/clio-mcp"),
            reasons: vec![ConflictReason::UnexpectedFile {
                path: "notes.md".to_owned(),
            }],
        }],
    };
    let text = portable_outcome_text(&outcome);
    assert!(text.contains("because 1 skill root holds paths"), "{text}");
}

#[test]
fn an_unreadable_marker_adds_the_note_that_force_does_not_help() {
    let outcome = SkillOutcome::Refused {
        conflicts: vec![RootConflict {
            dir: PathBuf::from("/a/clio-mcp"),
            reasons: vec![ConflictReason::UnreadableMarker {
                path: ".clio-skill.json".to_owned(),
            }],
        }],
    };
    let text = portable_outcome_text(&outcome);
    assert!(
        text.contains(".clio-skill.json: unreadable marker"),
        "{text}"
    );
    assert!(text.contains("refused even with --force"), "{text}");
}

#[test]
fn hermes_gets_the_portable_reason_and_another_harness_does_not() {
    let outcome = SkillOutcome::Unchanged {
        roots: vec![PathBuf::from("/a/clio-mcp")],
    };
    let hermes = outcome_text(record("hermes"), &outcome);
    assert!(
        hermes.contains("note: hermes has no documented native skill mechanism"),
        "{hermes}"
    );
    let claude = outcome_text(record("claude"), &outcome);
    assert!(!claude.contains("note:"), "{claude}");
    assert!(!claude.contains("loaded"), "{claude}");
}

#[test]
fn the_failure_messages_name_the_harness_or_the_portable_assets() {
    let io = WriteError::Io {
        detail: "failed to create /a/clio-mcp: denied".to_owned(),
    };
    assert_eq!(
        failure_message(record("claude"), &io),
        "could not install the claude skill assets: failed to create /a/clio-mcp: denied"
    );
    assert_eq!(
        portable_failure_message(&io),
        "could not install the portable clio-mcp skill assets: failed to create /a/clio-mcp: denied"
    );
    let unparseable = WriteError::Unparseable {
        path: PathBuf::from("/a/clio-mcp/SKILL.md"),
        snippet: "line 4: broken".to_owned(),
    };
    assert_eq!(
        failure_message(record("claude"), &unparseable),
        "could not install the claude skill assets: /a/clio-mcp/SKILL.md line 4: broken"
    );
    assert_eq!(
        portable_failure_message(&unparseable),
        "could not install the portable clio-mcp skill assets: /a/clio-mcp/SKILL.md line 4: broken"
    );
}
