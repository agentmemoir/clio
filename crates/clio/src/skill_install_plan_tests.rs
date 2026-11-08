// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the installation decision: the no-op for a current
//! installation, every conflict reason, the write `--force` produces, and the
//! package-directory-only scan the decision reads.
//!
//! ## Owns
//! - The `Scratch` unique temp directory guard and every assertion over
//!   `plan_install`, `scan_package_dir`, and the temp-sibling name
//!
//! ## Does not own
//! - The decision implementation under test (`skill_install_plan`)
//! - The marker under test (`skill_marker`) or the installer (`skill_install`)
//!
//! ## Boundary
//!
//! Test-only. Every path lives under one `Scratch` directory rooted in the
//! system temporary directory, so no test reads or writes a real skill root.

use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Nanoseconds plus a process-global counter: two calls never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// One test's own directory, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "clio-skill-plan-{label}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("the scratch directory can be created");
        Self { root }
    }

    fn package(&self) -> PathBuf {
        self.root.join("clio-mcp")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The incoming asset set every test plans against.
fn assets() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SKILL.md", "# clio-mcp\n"),
        ("references/01-retrieval.md", "retrieval\n"),
        ("references/02-write.md", "write\n"),
    ]
}

fn marker(assets: &[(&str, &str)]) -> SkillMarker {
    SkillMarker::from_assets(assets, "fnv1a64:0123456789abcdef")
}

/// The package directory holdings a complete installation of `assets` leaves.
fn on_disk(assets: &[(&str, &str)]) -> BTreeMap<String, String> {
    assets
        .iter()
        .map(|(name, text)| ((*name).to_owned(), sha256_hex(text.as_bytes())))
        .collect()
}

/// The marker file itself, as a package directory that already holds one lists it.
fn marker_entry() -> (String, String) {
    (
        SKILL_MARKER_FILE.to_owned(),
        sha256_hex(b"{\"installer\":\"clio\"}"),
    )
}

/// The paths of `assets` in path order, which is the order the plan lists them in.
fn every_asset(assets: &[(&str, &str)]) -> Vec<String> {
    let mut names: Vec<String> = assets.iter().map(|(name, _)| (*name).to_owned()).collect();
    names.sort();
    names
}

/// What a first installation of `assets` writes: every asset, then the marker.
fn first_install(assets: &[(&str, &str)]) -> Vec<String> {
    let mut names = every_asset(assets);
    names.push(SKILL_MARKER_FILE.to_owned());
    names
}

#[test]
fn an_identical_installation_is_a_no_op() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    let (path, hash) = marker_entry();
    disk.insert(path, hash);
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets), &disk, false),
        InstallPlan::NoOp
    );
}

#[test]
fn a_locally_edited_owned_file_is_a_conflict() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    disk.insert("SKILL.md".to_owned(), sha256_hex(b"my own router\n"));
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets), &disk, false),
        InstallPlan::Conflict {
            reasons: vec![ConflictReason::LocalEdit {
                path: "SKILL.md".to_owned()
            }]
        }
    );
}

#[test]
fn a_missing_owned_file_is_a_conflict() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    disk.remove("references/02-write.md");
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets), &disk, false),
        InstallPlan::Conflict {
            reasons: vec![ConflictReason::MissingOwned {
                path: "references/02-write.md".to_owned()
            }]
        }
    );
}

#[test]
fn an_unexpected_file_inside_the_package_is_a_conflict() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    disk.insert("notes.md".to_owned(), sha256_hex(b"mine"));
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets), &disk, false),
        InstallPlan::Conflict {
            reasons: vec![ConflictReason::UnexpectedFile {
                path: "notes.md".to_owned()
            }]
        }
    );
}

#[test]
fn a_markerless_package_identical_to_the_payload_adopts_the_marker() {
    let assets = assets();
    let disk = on_disk(&assets);
    assert_eq!(
        plan_install(None, &marker(&assets), &disk, false),
        InstallPlan::Write {
            files: vec![SKILL_MARKER_FILE.to_owned()],
            preserved_foreign: Vec::new()
        },
        "an earlier clio's package that already matches the payload writes the marker alone"
    );
}

#[test]
fn a_markerless_package_that_differs_keeps_refusing() {
    let assets = assets();
    let mut disk = on_disk(&assets);
    disk.insert("SKILL.md".to_owned(), sha256_hex(b"my own router\n"));
    assert_eq!(
        plan_install(None, &marker(&assets), &disk, false),
        InstallPlan::Conflict {
            reasons: vec![ConflictReason::EarlierInstall {
                path: "SKILL.md".to_owned()
            }]
        }
    );
}

#[test]
fn an_empty_package_without_a_marker_is_written_in_full() {
    let assets = assets();
    let disk = BTreeMap::new();
    assert_eq!(
        plan_install(None, &marker(&assets), &disk, false),
        InstallPlan::Write {
            files: first_install(&assets),
            preserved_foreign: Vec::new()
        }
    );
}

#[test]
fn a_newer_payload_is_written_over_an_unchanged_installation() {
    let installed = vec![
        ("SKILL.md", "# clio-mcp\n"),
        ("references/01-retrieval.md", "retrieval\n"),
    ];
    let recorded = marker(&installed);
    let disk = on_disk(&installed);
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets()), &disk, false),
        InstallPlan::Write {
            files: vec![
                "references/02-write.md".to_owned(),
                SKILL_MARKER_FILE.to_owned()
            ],
            preserved_foreign: Vec::new()
        }
    );
}

#[test]
fn force_turns_a_local_edit_into_a_write_that_preserves_foreign_files() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    disk.insert("SKILL.md".to_owned(), sha256_hex(b"my own router\n"));
    disk.insert("notes.md".to_owned(), sha256_hex(b"mine"));
    assert_eq!(
        plan_install(Some(&recorded), &marker(&assets), &disk, true),
        InstallPlan::Write {
            files: vec!["SKILL.md".to_owned()],
            preserved_foreign: vec!["notes.md".to_owned()]
        }
    );
}

#[test]
fn force_writes_over_an_unrecorded_package_and_keeps_its_other_files() {
    let assets = assets();
    let mut disk = on_disk(&assets);
    disk.insert("notes.md".to_owned(), sha256_hex(b"mine"));
    assert_eq!(
        plan_install(None, &marker(&assets), &disk, true),
        InstallPlan::Write {
            files: vec![SKILL_MARKER_FILE.to_owned()],
            preserved_foreign: vec!["notes.md".to_owned()]
        },
        "force records the package and keeps the file it does not own"
    );
}

#[test]
fn conflict_reasons_are_reported_in_path_order() {
    let assets = assets();
    let recorded = marker(&assets);
    let mut disk = on_disk(&assets);
    disk.remove("SKILL.md");
    disk.insert("AAA.md".to_owned(), sha256_hex(b"mine"));
    match plan_install(Some(&recorded), &marker(&assets), &disk, false) {
        InstallPlan::Conflict { reasons } => {
            let paths: Vec<&str> = reasons.iter().map(ConflictReason::path).collect();
            assert_eq!(paths, vec!["AAA.md", "SKILL.md"]);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn each_reason_names_the_file_it_is_about() {
    let reasons = [
        ConflictReason::LocalEdit {
            path: "a".to_owned(),
        },
        ConflictReason::MissingOwned {
            path: "b".to_owned(),
        },
        ConflictReason::UnexpectedFile {
            path: "c".to_owned(),
        },
        ConflictReason::EarlierInstall {
            path: "d".to_owned(),
        },
        ConflictReason::UnreadableMarker {
            path: "e".to_owned(),
        },
    ];
    let paths: Vec<&str> = reasons.iter().map(ConflictReason::path).collect();
    assert_eq!(paths, vec!["a", "b", "c", "d", "e"]);
}

#[test]
fn a_file_beside_the_package_directory_is_not_scanned() {
    let scratch = Scratch::new("parent");
    fs::create_dir_all(scratch.package()).expect("the package directory can be created");
    fs::write(scratch.root.join("other-skill.md"), b"not ours")
        .expect("the sibling file can be written");

    let disk = scan_package_dir(&scratch.package()).expect("the package directory scans");
    assert!(
        disk.is_empty(),
        "only the package directory is read: {disk:?}"
    );
    let assets = assets();
    assert_eq!(
        plan_install(None, &marker(&assets), &disk, false),
        InstallPlan::Write {
            files: first_install(&assets),
            preserved_foreign: Vec::new()
        }
    );
}

#[test]
fn the_scan_keys_each_file_by_its_relative_path_and_records_its_digest() {
    let scratch = Scratch::new("scan");
    let package = scratch.package();
    fs::create_dir_all(package.join("references")).expect("the reference directory can be created");
    fs::write(package.join("SKILL.md"), b"router").expect("the router can be written");
    fs::write(
        package.join("references").join("01-retrieval.md"),
        b"retrieval",
    )
    .expect("the reference can be written");

    let disk = scan_package_dir(&package).expect("the package directory scans");
    let keys: Vec<&str> = disk.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["SKILL.md", "references/01-retrieval.md"]);
    assert_eq!(disk.get("SKILL.md"), Some(&sha256_hex(b"router")));
    assert_eq!(
        disk.get("references/01-retrieval.md"),
        Some(&sha256_hex(b"retrieval"))
    );
}

#[test]
fn a_package_directory_that_does_not_exist_scans_as_empty() {
    let scratch = Scratch::new("absent");
    let disk = scan_package_dir(&scratch.package()).expect("a missing directory holds no files");
    assert!(disk.is_empty(), "{disk:?}");
}

#[test]
fn every_recorded_marker_field_is_compared_with_the_incoming_one() {
    let assets = assets();
    let disk = on_disk(&assets);
    let expected = marker(&assets);
    let drifted = [
        (
            "installer",
            SkillMarker {
                installer: "another-installer".to_owned(),
                ..expected.clone()
            },
        ),
        (
            "package_version",
            SkillMarker {
                package_version: "0.0.1-old".to_owned(),
                ..expected.clone()
            },
        ),
        (
            "catalog_hash",
            SkillMarker {
                catalog_hash: "fnv1a64:0000000000000000".to_owned(),
                ..expected.clone()
            },
        ),
        (
            "payload_digest",
            SkillMarker {
                payload_digest: "00".repeat(32),
                ..expected.clone()
            },
        ),
    ];
    for (field, recorded) in drifted {
        assert_eq!(
            plan_install(Some(&recorded), &expected, &disk, false),
            InstallPlan::Write {
                files: vec![SKILL_MARKER_FILE.to_owned()],
                preserved_foreign: Vec::new()
            },
            "a stale {field} with every asset unchanged rewrites only the marker"
        );
    }
}

#[test]
fn the_installer_temp_sibling_name_is_recognized_and_a_lookalike_is_not() {
    let name = temp_sibling_name("SKILL.md", 999, 123, 0);
    assert_eq!(name, ".SKILL.md.999.123.0.tmp");
    assert!(is_temp_sibling(&name), "{name}");
    for other in [
        "SKILL.md",
        "notes.tmp",
        ".SKILL.md.999.123.tmp",
        ".SKILL.md.x.123.0.tmp",
        "..999.123.0.tmp",
        ".SKILL.md.999.123.-1.tmp",
    ] {
        assert!(!is_temp_sibling(other), "{other} is not a temp sibling");
    }
}

#[test]
fn a_leftover_temp_sibling_is_not_scanned() {
    let scratch = Scratch::new("temp-sibling");
    let package = scratch.package();
    fs::create_dir_all(&package).expect("the package directory can be created");
    fs::write(package.join("SKILL.md"), b"router").expect("the router can be written");
    fs::write(
        package.join(temp_sibling_name("references", 999, 123, 0)),
        b"half written",
    )
    .expect("the leftover temp sibling can be written");

    let disk = scan_package_dir(&package).expect("the package directory scans");
    let keys: Vec<&str> = disk.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["SKILL.md"],
        "this program's own temp sibling is not a package file"
    );
}
