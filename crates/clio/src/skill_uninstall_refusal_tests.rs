// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests every fact that makes the skill remover refuse a package: a foreign installer, an edited file, a missing file, an extra file, a marker path outside the package, a payload digest that does not match, a symlinked package directory, a symlinked recorded asset, a symlinked marker, and a symlinked skill root.
//!
//! ## Owns
//! - Assertions over the refusal reasons and the refusal text
//!
//! ## Does not own
//! - The remover under test (`skill_uninstall`)
//! - The scratch home and marker helpers (`skill_uninstall_test_support`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the system temporary directory, so no test touches the real home directory.
//!
//! Keep this module focused on the remover's refusal cases.

use super::*;
use crate::agent_harness::resolve;
use crate::skill_install::install;
use crate::skill_install_plan::ConflictReason;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall_test_support::{
    Scratch, env, first_asset, native_package, portable_package, read_marker, refused, write_marker,
};
use crate::skill_uninstall_text::failure_message;
use std::fs;

fn claude() -> &'static HarnessRecord {
    resolve("claude").expect("claude is supported")
}

fn write_package(env: &DirectWriteEnv) {
    install(claude(), env, false).expect("the skill package installs");
}

#[test]
fn refuses_a_foreign_installer() {
    let scratch = Scratch::new("foreign");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let mut marker = read_marker(&native);
    marker.installer = String::from("other-tool");
    write_marker(&native, &marker);

    let error = uninstall_skill_only(Some(claude()), &env).expect_err("a foreign marker refuses");

    let (dir, reasons) = refused(error);
    assert_eq!(dir, native);
    assert_eq!(
        reasons,
        vec![RefusalReason::ForeignInstaller {
            recorded: String::from("other-tool"),
        }]
    );
    assert!(native.join("SKILL.md").is_file(), "nothing is removed");
    let text = failure_message(&UninstallError::Refused { dir, reasons });
    assert!(
        text.contains("its marker records installer `other-tool`, not `clio`"),
        "{text}"
    );
}

#[test]
fn refuses_an_edited_file() {
    let scratch = Scratch::new("edited");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let asset = first_asset(&native);
    fs::write(native.join(&asset), b"edited").expect("the asset is edited");

    let (dir, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("an edit refuses"));

    assert_eq!(dir, native);
    assert_eq!(
        reasons,
        vec![RefusalReason::Conflict(ConflictReason::LocalEdit {
            path: asset,
        })]
    );
}

#[test]
fn refuses_a_missing_file() {
    let scratch = Scratch::new("missing");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let asset = first_asset(&native);
    fs::remove_file(native.join(&asset)).expect("the asset is removed");

    let (_, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a missing file refuses"));

    assert_eq!(
        reasons,
        vec![RefusalReason::Conflict(ConflictReason::MissingOwned {
            path: asset,
        })]
    );
}

#[test]
fn refuses_an_unexpected_file() {
    let scratch = Scratch::new("extra");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    fs::write(native.join("extra.txt"), b"extra").expect("the extra file is written");

    let (_, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("an extra file refuses"));

    assert_eq!(
        reasons,
        vec![RefusalReason::Conflict(ConflictReason::UnexpectedFile {
            path: String::from("extra.txt"),
        })]
    );
}

#[test]
fn refuses_a_payload_digest_mismatch() {
    let scratch = Scratch::new("digest");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let mut marker = read_marker(&native);
    marker.payload_digest = String::from("not-the-recorded-digest");
    write_marker(&native, &marker);

    let (dir, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a digest mismatch refuses"));

    assert_eq!(dir, native);
    match reasons.as_slice() {
        [RefusalReason::PayloadDigest { recorded, found }] => {
            assert_eq!(recorded, "not-the-recorded-digest");
            assert_ne!(found, recorded);
        }
        other => panic!("expected one digest reason, got {other:?}"),
    }
    let text = failure_message(&UninstallError::Refused { dir, reasons });
    assert!(text.contains("payload digest differs"), "{text}");
    assert!(text.contains("nothing was removed"), "{text}");
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_package_directory() {
    let scratch = Scratch::new("dir-link");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let copy = scratch.root.join("native-copy");
    fs::rename(&native, &copy).expect("the package moves aside");
    std::os::unix::fs::symlink(&copy, &native).expect("the package path becomes a link");

    let (dir, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a link refuses"));

    assert_eq!(dir, native);
    assert!(matches!(
        reasons.as_slice(),
        [RefusalReason::Symlink { .. }]
    ));
    assert!(copy.join("SKILL.md").is_file(), "the target is untouched");
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_recorded_asset() {
    let scratch = Scratch::new("asset-link");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let asset = first_asset(&native);
    let copy = scratch.root.join("asset-copy");
    fs::rename(native.join(&asset), &copy).expect("the asset moves aside");
    std::os::unix::fs::symlink(&copy, native.join(&asset)).expect("the asset becomes a link");

    let (_, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a link refuses"));

    assert_eq!(reasons, vec![RefusalReason::Symlink { path: asset }]);
    assert!(copy.is_file(), "the target is untouched");
}

#[test]
fn refuses_a_marker_that_names_a_path_outside_the_package() {
    let scratch = Scratch::new("outside-key");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let outside = scratch
        .root
        .join(".claude")
        .join("skills")
        .join("sibling")
        .join("SKILL.md");
    fs::create_dir_all(outside.parent().expect("the sibling has a parent"))
        .expect("the sibling directory is created");
    fs::write(&outside, b"outside").expect("the sibling is written");
    let mut value = serde_json::to_value(read_marker(&native)).expect("the marker is a JSON value");
    let files = value["files"]
        .as_object_mut()
        .expect("the marker records a file map");
    files.insert(String::from("/etc/passwd"), serde_json::json!("outside"));
    files.insert(
        String::from("../sibling/SKILL.md"),
        serde_json::json!("outside"),
    );
    let bytes = serde_json::to_vec(&value).expect("the marker serializes");
    fs::write(native.join(SKILL_MARKER_FILE), bytes).expect("the marker is written");

    let (dir, reasons) = refused(
        uninstall_skill_only(Some(claude()), &env).expect_err("a path outside the package refuses"),
    );

    assert_eq!(dir, native);
    assert_eq!(
        reasons,
        vec![RefusalReason::Conflict(ConflictReason::UnreadableMarker {
            path: String::from(SKILL_MARKER_FILE),
        })]
    );
    assert!(outside.is_file(), "a parent-relative key touches nothing");
    assert!(
        std::path::Path::new("/etc/passwd").exists(),
        "an absolute key touches nothing"
    );
    assert!(
        native.join("SKILL.md").is_file(),
        "the package is untouched"
    );
    assert!(
        portable_package(&scratch.root)
            .join(SKILL_MARKER_FILE)
            .is_file(),
        "the second root is untouched"
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_marker() {
    let scratch = Scratch::new("marker-link");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let copy = scratch.root.join("marker-copy.json");
    fs::rename(native.join(SKILL_MARKER_FILE), &copy).expect("the marker moves aside");
    std::os::unix::fs::symlink(&copy, native.join(SKILL_MARKER_FILE))
        .expect("the marker path becomes a link");

    let (dir, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a marker link refuses"));

    assert_eq!(dir, native);
    assert_eq!(
        reasons,
        vec![RefusalReason::Symlink {
            path: String::from(SKILL_MARKER_FILE),
        }]
    );
    assert!(copy.is_file(), "the marker target is untouched");
    assert!(native.join("SKILL.md").is_file(), "nothing is removed");
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_skill_root() {
    let scratch = Scratch::new("root-link");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    let claude_dir = scratch.root.join(".claude");
    let real = scratch.root.join("real-claude");
    fs::rename(&claude_dir, &real).expect("the skill root moves aside");
    std::os::unix::fs::symlink(&real, &claude_dir).expect("the skill root becomes a link");

    let (dir, reasons) = refused(
        uninstall_skill_only(Some(claude()), &env).expect_err("a linked skill root refuses"),
    );

    assert_eq!(dir, native);
    assert_eq!(
        reasons,
        vec![RefusalReason::Symlink {
            path: claude_dir.display().to_string(),
        }]
    );
    assert!(
        real.join("skills")
            .join("clio-mcp")
            .join("SKILL.md")
            .is_file(),
        "the package is untouched"
    );
}

#[test]
fn an_unreadable_marker_refuses() {
    let scratch = Scratch::new("bad-marker");
    let env = env(&scratch.root);
    write_package(&env);
    let native = native_package(&scratch.root);
    fs::write(native.join(SKILL_MARKER_FILE), b"not a marker").expect("the marker is replaced");

    let (_, reasons) =
        refused(uninstall_skill_only(Some(claude()), &env).expect_err("a bad marker refuses"));

    assert_eq!(
        reasons,
        vec![RefusalReason::Conflict(ConflictReason::UnreadableMarker {
            path: String::from(SKILL_MARKER_FILE),
        })]
    );
}
