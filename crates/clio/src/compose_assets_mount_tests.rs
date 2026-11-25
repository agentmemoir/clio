// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the materialized `models/` README and the bind-mount source check.
//! All run in per-test temp directories; nothing outside them is touched.

use super::*;

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

/// The README materialized into `models/` has to be true for both paths:
/// `clio setup` downloads the missing model itself, so setup needs no manual
/// step, while the compose and Makefile starts still need the file present and
/// print how to fetch it. A README that presents the fetch as always manual
/// sends the operator to do work the tool already does.
#[test]
fn models_readme_covers_the_automatic_and_the_manual_path() {
    let readme = models_readme();
    // The shipped name, spelled exactly as the container opens it.
    assert!(readme.contains(GGUF_FILE_NAME), "{readme}");
    // The automatic path: setup fetches the missing file before it writes.
    assert!(readme.contains("`clio setup` fetches it"), "{readme}");
    assert!(
        readme.contains("setup downloads the model from the Hugging Face Hub"),
        "{readme}"
    );
    // The two ways a machine without the Hub completes the same step.
    assert!(readme.contains("CLIO_EXTRACT_MODEL_URL"), "{readme}");
    assert!(readme.contains("CLIO_EXTRACT_MODEL_FILE"), "{readme}");
    // The manual path survives for the compose and Makefile starts, and names the
    // one repository the download itself uses.
    assert!(
        readme.contains(&format!("hf download {GGUF_REPO}")),
        "{readme}"
    );
    assert!(
        readme.contains(&format!("{GGUF_FILE_NAME} --local-dir")),
        "{readme}"
    );
    assert!(
        readme.contains("refuse to start the extract service while the file is absent"),
        "{readme}"
    );
    // It no longer asks for the fetch as a step every start needs.
    assert!(
        !readme.contains("place it here before selecting"),
        "the README must not present the fetch as always manual: {readme}"
    );
    // The file the operator reads is this text, byte for byte.
    let root = temp_root("readme-text");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    assert_eq!(
        std::fs::read_to_string(root.join("models/README.md")).unwrap(),
        readme
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn bind_mount_check_names_the_missing_source() {
    let root = temp_root("bindmounts");
    // No materialization at all: every source is missing.
    let err = verify_bind_mounts(&root).expect_err("empty root must fail");
    assert!(err.contains("sql/001_core.sql"), "{err}");
    assert!(
        err.contains(&root.display().to_string()),
        "error must name the root: {err}"
    );
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    assert_eq!(
        verify_bind_mounts(&root),
        Ok(()),
        "materialized set satisfies the mounts"
    );
    // Deleting a mount source after materialization is caught.
    std::fs::remove_file(root.join("sql/002_vectors_postgres.sql")).expect("delete mount");
    let err = verify_bind_mounts(&root).expect_err("deleted mount must fail");
    assert!(err.contains("sql/002_vectors_postgres.sql"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Only `extract` mounts `models/`, and the check that reads that mount lives
/// in `compose_model`, where it tests the file inside the directory. Requiring
/// the directory here is what let Docker's own auto-created empty one satisfy
/// the guard for every selection.
#[test]
fn the_init_script_check_does_not_require_the_model_directory() {
    let root = temp_root("no-models-dir");
    materialize(&root, ComposeTarget::MacArm64).expect("materialize");
    assert!(
        root.join("models").is_dir(),
        "materialize writes the models/ README, so the directory a directory-level check would \
         trust is there to begin with"
    );
    std::fs::remove_dir_all(root.join("models")).expect("delete models/");
    assert_eq!(
        verify_bind_mounts(&root),
        Ok(()),
        "models/ is mounted by extract alone, so this check must not require it"
    );
    let _ = std::fs::remove_dir_all(&root);
}
