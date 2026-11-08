// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the two cases where the marker alone, and not an asset, is
//! the thing that is stale: a marker whose own recorded fields drifted while
//! every asset stayed current, and a package an earlier clio installed with no
//! marker at all. It also tests `--force` over such a package and the installer's
//! own leftover temp sibling.
//!
//! ## Owns
//! - The marker-only rewrite, the markerless adoption, the markerless refusal,
//!   and the forced repair of a markerless package, over a real filesystem and
//!   the real 18-asset payload
//! - The command-level exit code and the registration step the adoption lets run
//!
//! ## Does not own
//! - The pure decision matrix (`skill_install_plan_tests`)
//! - The marker, conflict, and no-op cases an unmarked payload already covers
//!   (`skill_install_marker_tests`)
//!
//! ## Boundary
//!
//! Test-only: every path lives under one `Scratch` dir in the system temp dir, so
//! no test touches the real home directory.

use super::test_support::*;
use super::*;
use crate::install_cli::run_with_skills;
use crate::install_cli_decide::Verb;
use crate::skill_install_plan::{ConflictReason, temp_sibling_name};
use crate::skill_marker::owned_files;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The embedded bytes of one asset, found by name.
fn embedded(name: &str) -> &'static str {
    SKILL_ASSETS
        .iter()
        .find(|(asset, _)| *asset == name)
        .expect("the asset is embedded")
        .1
}

/// Install the portable package, then remove the marker an earlier clio never
/// wrote, leaving the assets exactly as that earlier clio left them.
fn install_without_a_marker(label: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(label);
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install_portable(&env(&scratch.root), false).expect("the first install writes");
    fs::remove_file(dir.join(SKILL_MARKER_FILE)).expect("the marker can be removed");
    (scratch, dir)
}

#[test]
fn a_marker_whose_own_fields_drifted_is_rewritten_alone() {
    let scratch = Scratch::new("marker-drift");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install_portable(&env(&scratch.root), false).expect("the first install writes");
    let mut stale = read_marker(&dir);
    stale.package_version = "0.0.1-old".to_owned();
    stale.catalog_hash = "fnv1a64:0000000000000000".to_owned();
    fs::write(
        dir.join(SKILL_MARKER_FILE),
        stale.to_bytes().expect("the stale marker serializes"),
    )
    .expect("the stale marker can be written");
    let before = snapshot(&dir);

    let outcome = install_portable(&env(&scratch.root), false)
        .expect("a marker-only difference is an install, not a refusal");

    assert_eq!(
        written(&outcome)[0].written,
        [SKILL_MARKER_FILE],
        "a stale marker with current assets writes the marker and no asset"
    );
    let after = snapshot(&dir);
    for (name, text) in SKILL_ASSETS {
        assert_eq!(
            after.get(*name),
            before.get(*name),
            "{name} must keep its bytes and its modification time"
        );
        assert_eq!(
            fs::read(dir.join(*name)).expect("the asset is readable"),
            text.as_bytes(),
            "{name} must still be the embedded text"
        );
    }
    assert_ne!(
        after.get(SKILL_MARKER_FILE),
        before.get(SKILL_MARKER_FILE),
        "the marker's bytes or its modification time change"
    );
    let marker = read_marker(&dir);
    assert_eq!(marker.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(marker.catalog_hash, crate::skill_marker::catalog_hash());
}

#[test]
fn a_markerless_package_identical_to_the_payload_writes_only_the_marker() {
    let (scratch, dir) = install_without_a_marker("adopt-plan");
    let before = snapshot(&dir);

    let outcome =
        install_portable(&env(&scratch.root), false).expect("an identical package is adopted");

    assert_eq!(
        written(&outcome)[0].written,
        [SKILL_MARKER_FILE],
        "an adoption writes the marker and never an asset"
    );
    let after = snapshot(&dir);
    for (name, value) in &before {
        assert_eq!(
            after.get(name),
            Some(value),
            "{name} keeps its bytes and its modification time"
        );
    }
    assert_eq!(read_marker(&dir).files, owned_files(SKILL_ASSETS));
}

#[test]
fn a_markerless_package_identical_to_the_payload_lets_registration_run() {
    let (scratch, dir) = install_without_a_marker("adopt-register");
    let registered = Cell::new(false);

    let code = run_with_skills(
        Verb::Install,
        &args(&["--harness", "codex"]),
        &|| Ok(PathBuf::from("clio")),
        &|| env(&scratch.root),
        &|_: &HarnessRecord, _: &Path| {
            registered.set(true);
            Ok("registered".to_owned())
        },
        &|_: &HarnessRecord| panic!("uninstall must not run"),
        &|record: Option<&HarnessRecord>, env: &DirectWriteEnv, force: bool| {
            crate::skill_install::install_and_report(record.expect("a harness"), env, force)
        },
    );

    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "an adoption must not fail the step"
    );
    assert!(registered.get(), "registration runs after the adoption");
    assert_eq!(read_marker(&dir).files, owned_files(SKILL_ASSETS));
}

#[test]
fn a_markerless_package_with_an_edited_asset_is_refused() {
    let (scratch, dir) = install_without_a_marker("earlier-edit");
    fs::write(dir.join("references").join("01-retrieval.md"), b"my edit")
        .expect("the asset can be edited");
    let before = snapshot(&dir);

    let outcome = install(record("codex"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");

    assert_eq!(
        conflicts(&outcome)[0].reasons,
        [ConflictReason::EarlierInstall {
            path: "references/01-retrieval.md".to_owned()
        }],
        "a markerless package that differs is still refused"
    );
    assert_eq!(snapshot(&dir), before, "a refusal writes nothing");
    let text = portable_outcome_text(&outcome);
    assert!(
        text.contains("references/01-retrieval.md: installed by an earlier clio without a marker"),
        "{text}"
    );
    assert!(
        text.contains("nothing was written in any skill root"),
        "{text}"
    );
}

#[test]
fn a_markerless_package_with_an_extra_file_is_refused() {
    let (scratch, dir) = install_without_a_marker("earlier-extra");
    fs::write(dir.join("notes.md"), b"mine").expect("the extra file can be written");
    let before = snapshot(&dir);

    let outcome = install(record("codex"), &env(&scratch.root), false)
        .expect("a refusal is a decision, not a write failure");

    assert_eq!(
        conflicts(&outcome)[0].reasons,
        [ConflictReason::EarlierInstall {
            path: "notes.md".to_owned()
        }],
        "a file the payload does not carry keeps the package refused"
    );
    assert_eq!(snapshot(&dir), before, "a refusal writes nothing");
}

#[test]
fn force_on_a_markerless_package_replaces_owned_names_and_keeps_an_unrelated_file() {
    let (scratch, dir) = install_without_a_marker("earlier-force");
    let target = dir.join("references").join("01-retrieval.md");
    fs::write(&target, b"my edit").expect("the asset can be edited");
    let notes = dir.join("notes.md");
    fs::write(&notes, b"mine").expect("the unrelated file can be written");
    let notes_before = (
        fs::read(&notes).expect("the file is readable"),
        modified(&notes),
    );

    let outcome =
        install(record("codex"), &env(&scratch.root), true).expect("the forced run writes");

    assert_eq!(
        written(&outcome)[0].written,
        ["references/01-retrieval.md", SKILL_MARKER_FILE],
        "the forced run replaces the file whose name the package owns, then records it"
    );
    assert_eq!(
        fs::read(&target).expect("the asset is readable"),
        embedded("references/01-retrieval.md").as_bytes(),
        "the owned name holds the embedded bytes again"
    );
    assert_eq!(
        (
            fs::read(&notes).expect("the file is readable"),
            modified(&notes)
        ),
        notes_before,
        "the file the package does not own keeps its bytes and its time"
    );
    assert_eq!(written(&outcome)[0].preserved_foreign, ["notes.md"]);
    let text = portable_outcome_text(&outcome);
    assert!(
        text.contains("wrote references/01-retrieval.md, .clio-skill.json"),
        "the printed text names exactly what was written: {text}"
    );
    assert!(
        text.contains("kept unowned files in"),
        "the printed text names the file it kept: {text}"
    );
    assert_eq!(read_marker(&dir).files, owned_files(SKILL_ASSETS));
}

#[test]
fn a_leftover_temp_sibling_is_ignored_and_left_alone() {
    let scratch = Scratch::new("leftover-temp");
    let dir = package(&scratch.root, &[".agents", "skills"]);
    install_portable(&env(&scratch.root), false).expect("the first install writes");
    let leftover = dir.join(temp_sibling_name("SKILL.md", 999, 123, 0));
    fs::write(&leftover, b"half written").expect("the leftover temp sibling can be written");
    let before = snapshot(&dir);

    let outcome =
        install_portable(&env(&scratch.root), false).expect("a temp sibling refuses nothing");

    assert!(
        matches!(outcome, SkillOutcome::Unchanged { .. }),
        "this program's own temp sibling is not a package file: {outcome:?}"
    );
    assert_eq!(
        snapshot(&dir),
        before,
        "nothing is written and nothing is deleted"
    );
}
