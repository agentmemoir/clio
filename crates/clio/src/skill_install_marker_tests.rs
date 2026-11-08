// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the provenance contract of the installer over the real
//! 18-asset payload and a real filesystem: the marker a first install writes, the
//! no-op a second install must be, the refusal a bad package earns, and `--force`.
//!
//! ## Owns
//! - The marker-and-outcome assertions: contents, digest, exact modification
//!   times, refusal reasons, and forced rewrites
//! - The `snapshot` helper and the command-level exit code a refusal produces
//!
//! ## Does not own
//! - The installer and text under test (`skill_install`, `skill_install_text`)
//! - The pure decision matrix (`skill_install_plan_tests`) and the per-harness
//!   root mapping (`skill_install_roots_tests`)
//!
//! ## Boundary
//!
//! Test-only: every path lives under one `Scratch` dir in the system temp dir,
//! so no test touches the real home directory.

use super::test_support::*;
use super::*;
use crate::install_cli::run_with_skills;
use crate::install_cli_decide::Verb;
use crate::skill_install_plan::ConflictReason;
use crate::skill_marker::{owned_files, payload_digest};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

#[test]
fn two_identical_installs_leave_every_asset_and_the_marker_unchanged() {
    let scratch = Scratch::new("identical");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    let first = install_portable(&env(&scratch.root), false).expect("the first install writes");
    assert_eq!(
        written(&first).len(),
        1,
        "the portable install writes one root"
    );
    let before = snapshot(&dir);
    assert_eq!(
        before.len(),
        SKILL_ASSETS.len() + 1,
        "the package holds the assets and the marker"
    );

    let second = install_portable(&env(&scratch.root), false).expect("the second install is safe");
    assert!(
        matches!(second, SkillOutcome::Unchanged { .. }),
        "an identical second install is a no-op: {second:?}"
    );
    assert_eq!(
        snapshot(&dir),
        before,
        "no asset byte, no marker byte, and no modification time may change"
    );
}

#[test]
fn the_first_install_writes_the_marker_last_from_the_embedded_assets() {
    let scratch = Scratch::new("marker-last");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    let outcome = install_portable(&env(&scratch.root), false).expect("the assets install");

    assert_eq!(written(&outcome)[0].written, first_install_order());
    let marker = read_marker(&dir);
    assert_eq!(marker.installer, "clio");
    assert_eq!(marker.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(marker.catalog_hash, crate::skill_marker::catalog_hash());
    assert_eq!(marker.files, owned_files(SKILL_ASSETS));
    assert_eq!(marker.payload_digest, payload_digest(SKILL_ASSETS));

    let marker_time = modified(&dir.join(SKILL_MARKER_FILE));
    for (name, _) in SKILL_ASSETS {
        assert!(
            modified(&dir.join(name)) <= marker_time,
            "{name} must not be written after the marker"
        );
    }
}

#[test]
fn the_marker_payload_digest_excludes_the_marker_file() {
    let bytes = SkillMarker::from_assets(SKILL_ASSETS, &crate::skill_marker::catalog_hash())
        .to_bytes()
        .expect("the marker serializes");
    let text = std::str::from_utf8(&bytes).expect("the marker is UTF-8");
    let mut with_marker: Vec<(&str, &str)> = SKILL_ASSETS.to_vec();
    with_marker.push((SKILL_MARKER_FILE, text));

    assert_eq!(
        payload_digest(&with_marker),
        payload_digest(SKILL_ASSETS),
        "a payload that carries a marker digests as the payload without it"
    );
    assert_eq!(owned_files(&with_marker), owned_files(SKILL_ASSETS));
    assert!(
        !owned_files(SKILL_ASSETS).contains_key(SKILL_MARKER_FILE),
        "the marker is never one of the files it records"
    );

    let scratch = Scratch::new("digest");
    install_portable(&env(&scratch.root), false).expect("the assets install");
    let installed = read_marker(&package(&scratch.root, &[".agents", "skills"]));
    assert_eq!(installed.payload_digest, payload_digest(SKILL_ASSETS));
    assert_eq!(installed.files, owned_files(SKILL_ASSETS));
}

#[test]
fn a_locally_edited_asset_is_refused_by_default_and_changes_no_byte() {
    let scratch = Scratch::new("refuse-edit");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install(record("codex"), &env(&scratch.root), false).expect("the first install writes");
    let target = dir.join("references").join("01-retrieval.md");
    fs::write(&target, b"my edit").expect("the recorded asset can be edited");
    let before = snapshot(&dir);

    let outcome = install(record("codex"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");

    let reasons = &conflicts(&outcome)[0].reasons;
    assert_eq!(
        reasons,
        &[ConflictReason::LocalEdit {
            path: "references/01-retrieval.md".to_owned()
        }],
        "the local edit is the reason"
    );
    assert_eq!(snapshot(&dir), before, "a refusal writes nothing");
    let text = portable_outcome_text(&outcome);
    assert!(text.contains("references/01-retrieval.md"), "{text}");
    assert!(text.contains("local edit"), "{text}");
    assert!(
        text.contains("nothing was written in any skill root"),
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
}

#[test]
fn force_replaces_a_locally_edited_asset_and_keeps_an_unrelated_file() {
    let scratch = Scratch::new("force-edit");
    let skills = scratch.root.join(".agents").join("skills");
    install(record("codex"), &env(&scratch.root), false).expect("the first install writes");
    let target = skills
        .join(SKILL_DIR_NAME)
        .join("references")
        .join("01-retrieval.md");
    fs::write(&target, b"my edit").expect("the recorded asset can be edited");
    let keep = skills.join("keep.txt");
    fs::write(&keep, b"not ours").expect("the unrelated file can be written");
    let keep_before = (
        fs::read(&keep).expect("the file is readable"),
        modified(&keep),
    );

    let outcome =
        install(record("codex"), &env(&scratch.root), true).expect("the forced run writes");

    assert_eq!(
        written(&outcome)[0].written,
        ["references/01-retrieval.md"],
        "only the edited asset is rewritten"
    );
    let expected = SKILL_ASSETS
        .iter()
        .find(|(name, _)| *name == "references/01-retrieval.md")
        .expect("the reference is embedded")
        .1;
    assert_eq!(
        fs::read(&target).expect("the asset is readable"),
        expected.as_bytes(),
        "the forced run restores the embedded bytes"
    );
    assert_eq!(
        (
            fs::read(&keep).expect("the file is readable"),
            modified(&keep)
        ),
        keep_before,
        "a file beside the package directory is never touched"
    );
    let text = portable_outcome_text(&outcome);
    assert!(text.contains("wrote references/01-retrieval.md"), "{text}");
}

#[test]
fn an_unexpected_file_inside_the_package_is_refused_then_preserved_under_force() {
    let scratch = Scratch::new("unexpected");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install(record("codex"), &env(&scratch.root), false).expect("the first install writes");
    let notes = dir.join("notes.md");
    fs::write(&notes, b"mine").expect("the unexpected file can be written");
    let notes_before = (
        fs::read(&notes).expect("the file is readable"),
        modified(&notes),
    );
    let before = snapshot(&dir);

    let refused = install(record("codex"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");
    assert_eq!(
        conflicts(&refused)[0].reasons,
        [ConflictReason::UnexpectedFile {
            path: "notes.md".to_owned()
        }],
        "an unrecorded file inside the package is a conflict"
    );
    assert_eq!(snapshot(&dir), before, "the refusal writes nothing");
    let text = portable_outcome_text(&refused);
    assert!(text.contains("notes.md: unexpected file"), "{text}");

    let forced =
        install(record("codex"), &env(&scratch.root), true).expect("the forced run writes");
    assert_eq!(
        written(&forced)[0].preserved_foreign,
        ["notes.md"],
        "the forced run keeps and reports the unowned file"
    );
    assert!(
        written(&forced)[0].written.is_empty(),
        "every asset already matches, so only the report changes"
    );
    assert_eq!(
        (
            fs::read(&notes).expect("the file is readable"),
            modified(&notes)
        ),
        notes_before,
        "no path is deleted and the unowned file keeps its bytes and its time"
    );
    let text = portable_outcome_text(&forced);
    assert!(text.contains("notes.md"), "{text}");
    assert!(text.contains("kept unowned files"), "{text}");
    assert!(text.contains("nothing written"), "{text}");
}

#[test]
fn a_conflict_in_one_root_leaves_the_other_root_byte_and_mtime_identical() {
    let scratch = Scratch::new("two-roots");
    install(record("claude"), &env(&scratch.root), false).expect("the first install writes");
    let native = package(&scratch.root, &[".claude", "skills"]);
    let portable = package(&scratch.root, &[".agents", "skills"]);
    fs::write(
        native.join("references").join("01-retrieval.md"),
        b"my edit",
    )
    .expect("the native asset can be edited");
    let native_before = snapshot(&native);
    let portable_before = snapshot(&portable);

    let outcome = install(record("claude"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");

    assert_eq!(conflicts(&outcome).len(), 1, "one root refuses");
    assert_eq!(
        snapshot(&native),
        native_before,
        "the refused root is untouched"
    );
    assert_eq!(
        snapshot(&portable),
        portable_before,
        "a conflict in one root writes nothing in the other"
    );
    let text = outcome_text(record("claude"), &outcome);
    assert!(text.contains(&native.display().to_string()), "{text}");
    assert!(text.contains("local edit"), "{text}");
}

#[test]
fn a_refusal_in_both_roots_names_both_of_them() {
    let scratch = Scratch::new("two-conflicts");
    install(record("claude"), &env(&scratch.root), false).expect("the first install writes");
    let native = package(&scratch.root, &[".claude", "skills"]);
    let portable = package(&scratch.root, &[".agents", "skills"]);
    for dir in [&native, &portable] {
        fs::write(dir.join("references").join("01-retrieval.md"), b"my edit")
            .expect("both assets can be edited");
    }

    let outcome = install(record("claude"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");

    assert_eq!(conflicts(&outcome).len(), 2, "both roots refuse");
    let text = outcome_text(record("claude"), &outcome);
    assert!(text.contains("2 skill roots hold"), "{text}");
    assert!(text.contains(&native.display().to_string()), "{text}");
    assert!(text.contains(&portable.display().to_string()), "{text}");
}

#[test]
fn an_unreadable_marker_is_refused_even_with_force() {
    let scratch = Scratch::new("unreadable-marker");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install(record("codex"), &env(&scratch.root), false).expect("the first install writes");
    fs::write(dir.join(SKILL_MARKER_FILE), b"not a marker").expect("the marker can be corrupted");
    let before = snapshot(&dir);

    let outcome = install(record("codex"), &env(&scratch.root), true)
        .expect("a refusal is a decision, not a write failure");

    assert_eq!(
        conflicts(&outcome)[0].reasons,
        [ConflictReason::UnreadableMarker {
            path: SKILL_MARKER_FILE.to_owned()
        }],
        "an unreadable marker is never a silent overwrite"
    );
    assert_eq!(snapshot(&dir), before, "nothing is replaced");
    let text = portable_outcome_text(&outcome);
    assert!(text.contains("unreadable marker"), "{text}");
    assert!(text.contains("refused even with --force"), "{text}");
}

#[test]
fn a_written_outcome_names_its_files_and_a_no_op_claims_no_write() {
    let scratch = Scratch::new("truthful-text");
    let first = install_portable(&env(&scratch.root), false).expect("the first install writes");
    let first_text = portable_outcome_text(&first);
    assert!(
        first_text.contains("installed 18 skill assets into"),
        "{first_text}"
    );
    for name in ["SKILL.md", "references/01-retrieval.md", SKILL_MARKER_FILE] {
        assert!(
            first_text.contains(name),
            "the text must name {name}: {first_text}"
        );
    }

    let second = install_portable(&env(&scratch.root), false).expect("the second install is safe");
    let second_text = portable_outcome_text(&second);
    assert!(
        second_text.contains("unchanged, nothing written"),
        "{second_text}"
    );
    assert!(!second_text.contains("wrote"), "{second_text}");
}

#[test]
fn the_install_command_exits_non_zero_when_a_root_refuses() {
    let scratch = Scratch::new("refusal-exit");
    install(record("codex"), &env(&scratch.root), false).expect("the first install writes");
    let target = package(&scratch.root, &[".agents", "skills"])
        .join("references")
        .join("01-retrieval.md");
    fs::write(&target, b"my edit").expect("the recorded asset can be edited");

    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "codex"]),
        &|| Ok(PathBuf::from("clio")),
        &|| env(&scratch.root),
        &|_: &HarnessRecord, _: &Path| panic!("registration must not run after a refusal"),
        &|_: &HarnessRecord| panic!("uninstall must not run"),
        &|record: Option<&HarnessRecord>, env: &DirectWriteEnv, force: bool| {
            crate::skill_install::install_and_report(record.expect("a harness"), env, force)
        },
    );

    assert_eq!(code, ExitCode::from(1), "a refusal fails the command");
}
