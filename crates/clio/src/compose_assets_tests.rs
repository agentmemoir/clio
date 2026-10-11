// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for embedded-asset materialization: the materialized file set,
//! bind-mount existence checks, and the user-edit backup guard. All run in
//! per-test temp directories; nothing outside them is touched.

use super::*;

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

#[test]
fn materialize_writes_every_asset_for_the_target() {
    let root = temp_root("full-arm64");
    let written = materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    for path in [
        "compose.yml",
        "compose.arm64.yml",
        "sql/001_core.sql",
        "sql/002_vectors_postgres.sql",
        "models/README.md",
    ] {
        assert!(root.join(path).exists(), "{path} missing");
        assert!(written.contains(&path.to_string()), "{path} not reported");
    }
    assert!(
        !root.join("compose.amd64.yml").exists(),
        "wrong overlay materialized"
    );
    // The materialized files match the checked-in files byte for byte.
    assert_eq!(
        std::fs::read_to_string(root.join("compose.yml")).unwrap(),
        BASE_COMPOSE
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn materialize_amd64_overlay_for_linux_target() {
    let root = temp_root("amd64");
    materialize(&root, ComposeTarget::LinuxAmd64).expect("materialize");
    assert!(root.join("compose.amd64.yml").exists());
    assert!(!root.join("compose.arm64.yml").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn user_edited_compose_file_is_backed_up_before_overwrite() {
    let root = temp_root("backup");
    materialize(&root, ComposeTarget::MacArm64).expect("first materialize");
    let compose = root.join("compose.yml");
    std::fs::write(&compose, "# hand edit\nservices: {}\n").expect("user edit");
    materialize(&root, ComposeTarget::MacArm64).expect("second materialize");
    // The edited content was saved next to the file before the overwrite.
    let backups: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("compose.yml.bak-"))
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "expected exactly one backup, got {backups:?}"
    );
    let backup = root.join(&backups[0]);
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        "# hand edit\nservices: {}\n",
        "backup must hold the user's edit"
    );
    assert_eq!(
        std::fs::read_to_string(&compose).unwrap(),
        BASE_COMPOSE,
        "materialized file is restored to the embedded content"
    );
    // A third run over the identical embedded content creates no new backup.
    materialize(&root, ComposeTarget::MacArm64).expect("third materialize");
    let backup_count = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("compose.yml.bak-")
        })
        .count();
    assert_eq!(
        backup_count, 1,
        "unchanged content must not create a backup"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_backup_is_reported_not_silently_overwritten() {
    let root = temp_root("backup-fail");
    materialize(&root, ComposeTarget::MacArm64).expect("first materialize");
    std::fs::write(root.join("compose.yml"), "# hand edit\n").expect("user edit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555))
            .expect("make the root read-only");
        let err = materialize(&root, ComposeTarget::MacArm64)
            .expect_err("a failed backup must fail the materialize");
        assert!(err.contains("cannot back up"), "{err}");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("restore permissions");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prerequisites_name_the_missing_bind_mount() {
    let root = temp_root("bindmounts");
    // No materialization at all: every source is missing.
    let err = verify_prerequisites(&root, Stack::PostgresOnly, GGUF_FILE_NAME)
        .expect_err("empty root must fail");
    assert!(err.contains("sql/001_core.sql"), "{err}");
    assert!(
        err.contains(&root.display().to_string()),
        "error must name the root: {err}"
    );
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    assert_eq!(
        verify_prerequisites(&root, Stack::PostgresOnly, GGUF_FILE_NAME),
        Ok(()),
        "materialized set satisfies the mounts"
    );
    // Deleting a mount source after materialization is caught.
    std::fs::remove_file(root.join("sql/002_vectors_postgres.sql")).expect("delete mount");
    let err = verify_prerequisites(&root, Stack::PostgresOnly, GGUF_FILE_NAME)
        .expect_err("deleted mount must fail");
    assert!(err.contains("sql/002_vectors_postgres.sql"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The extract model is the reported defect: `models/` exists (materialize
/// writes a README into it) but the GGUF does not, so a directory-level check
/// passes and the container restarts forever.
#[test]
fn extract_without_its_model_file_fails() {
    let root = temp_root("extract-model");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    assert!(
        root.join("models").is_dir(),
        "materialize creates models/, the directory a directory-level check would trust"
    );
    let err = verify_prerequisites(&root, Stack::LocalRft, GGUF_FILE_NAME)
        .expect_err("extract without its model must fail");
    assert!(err.contains(GGUF_FILE_NAME), "{err}");
    assert!(
        err.contains("hf download"),
        "the error must name the command that fetches the model: {err}"
    );
    // A directory named like the model is still not the model file.
    let impostor = root.join("models").join(GGUF_FILE_NAME);
    std::fs::create_dir(&impostor).expect("create a directory named like the model");
    let err = verify_prerequisites(&root, Stack::LocalRft, GGUF_FILE_NAME)
        .expect_err("a directory is not a model file");
    assert!(err.contains(GGUF_FILE_NAME), "{err}");
    std::fs::remove_dir(&impostor).expect("remove the impostor");
    std::fs::write(&impostor, b"stub").expect("write the model");
    assert_eq!(
        verify_prerequisites(&root, Stack::LocalRft, GGUF_FILE_NAME),
        Ok(()),
        "a present model file satisfies the extract prerequisite"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_model_is_checked_under_the_name_the_caller_passes() {
    let root = temp_root("extract-name");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    std::fs::write(root.join("models/custom.gguf"), b"stub").expect("write the model");
    // The configured name is the one that counts: the default is absent here, so
    // checking the default would refuse a stack the configuration makes correct.
    assert!(
        verify_prerequisites(&root, Stack::LocalRft, GGUF_FILE_NAME).is_err(),
        "the default name is absent, so the default check must fail"
    );
    assert_eq!(
        verify_prerequisites(&root, Stack::LocalRft, "custom.gguf"),
        Ok(())
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Only `extract` mounts `models/`, so no other stack requires it. Requiring
/// the directory is what let Docker's auto-created empty one pass the check.
#[test]
fn only_the_extract_stack_needs_the_model_directory() {
    let root = temp_root("extract-not-selected");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    std::fs::remove_dir_all(root.join("models")).expect("delete models/");
    assert_eq!(
        verify_prerequisites(&root, Stack::PostgresOnly, GGUF_FILE_NAME),
        Ok(()),
        "a stack without extract must not require models/"
    );
    assert_eq!(
        verify_prerequisites(&root, Stack::LocalRetrieval, GGUF_FILE_NAME),
        Ok(()),
        "a retrieval stack must not require models/"
    );
    assert!(
        verify_prerequisites(&root, Stack::LocalRft, GGUF_FILE_NAME).is_err(),
        "the extract stack still requires its model, and models/ is gone"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gguf_path_lands_in_models() {
    let root = Path::new("/tmp");
    assert_eq!(
        gguf_path(root, GGUF_FILE_NAME),
        root.join("models").join(GGUF_FILE_NAME)
    );
}

#[test]
fn materialize_reports_a_root_that_cannot_be_created() {
    let parent = temp_root("blocked-root");
    let blocker = parent.join("f");
    std::fs::write(&blocker, "x").expect("create blocker file");
    let err = materialize(&parent.join("f/sub"), ComposeTarget::MacArm64)
        .expect_err("a file in the path must fail");
    assert!(err.contains("cannot create"), "{err}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn materialize_reports_write_failures() {
    let root = temp_root("write-fail");
    std::fs::create_dir(root.join("compose.yml")).expect("block the compose.yml path");
    let err =
        materialize(&root, ComposeTarget::MacArm64).expect_err("a directory in the way must fail");
    assert!(err.contains("cannot write"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
