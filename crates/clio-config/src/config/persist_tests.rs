// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of deployment-overlay load, default path
//! resolution, owner-only atomic save, and backup-before-replace.
//!
//! ## Owns
//! - Persistence behavior under test for `config::persist`
//!
//! ## Does not own
//! - Production persistence logic under test
//! - Config merge order or tool handlers
//!
//! ## Boundary
//!
//! Changes to deployment durability should normally update tests here.
//! Changes to unrelated config subsystems should not require changes here.
//!
//! Keep this module focused on deployment-overlay persistence tests.

use super::*;
use crate::Runtime;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn tmp(name: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-{name}-{n}"))
}

#[test]
fn default_deployment_path_is_loaded_and_can_be_disabled() {
    // A temp HOME stands in for the operator's real config dir.
    let home = std::env::temp_dir().join(format!(
        "am-home-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("t")
            .as_nanos()
    ));
    std::fs::create_dir_all(home.join(".config").join("clio")).expect("mkdir");
    std::fs::write(
        home.join(".config").join("clio").join("deployment.json"),
        r#"{"embed.provider":"openai"}"#,
    )
    .expect("write");

    let loaded = Runtime::with_env(&|key: &str| match key {
        "HOME" => Some(home.to_string_lossy().into_owned()),
        _ => None,
    });
    assert_eq!(
        loaded.deployment_field("embed.provider"),
        Some(&json!("openai")),
        "default path under HOME/.config/clio is loaded without CLIO_DEPLOYMENT_CONFIG"
    );

    // An explicitly empty CLIO_DEPLOYMENT_CONFIG disables the default.
    let disabled = Runtime::with_env(&|key: &str| match key {
        "HOME" => Some(home.to_string_lossy().into_owned()),
        "CLIO_DEPLOYMENT_CONFIG" => Some(String::new()),
        _ => None,
    });
    assert_eq!(disabled.deployment_field("embed.provider"), None);

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn load_missing_is_empty() {
    let path = tmp("missing.json");
    let overlay = load_overlay_file(&path).expect("missing");
    assert!(overlay.is_empty(), "a missing overlay file loads as empty");
}

#[test]
fn load_flat_and_nested_and_errors() {
    let flat = tmp("flat.json");
    fs::write(&flat, r#"{"log.level":"warn"}"#).expect("write");
    let o = load_overlay_file(&flat).expect("flat");
    assert_eq!(o.get("log.level"), Some(&json!("warn")));

    let nested = tmp("nested.json");
    fs::write(&nested, r#"{"log":{"level":"error"}}"#).expect("write");
    let o = load_overlay_file(&nested).expect("nested");
    assert_eq!(o.get("log.level"), Some(&json!("error")));

    let bad = tmp("bad.json");
    fs::write(&bad, "not-json").expect("write");
    assert_eq!(
        load_overlay_file(&bad).expect_err("parse").code,
        ErrorCode::ConfigCorrupt
    );

    let arr = tmp("arr.json");
    fs::write(&arr, "[1]").expect("write");
    assert_eq!(
        load_overlay_file(&arr).expect_err("obj").code,
        ErrorCode::ConfigCorrupt
    );

    // Directory path cannot be read as a file.
    let dir = tmp("dir");
    fs::create_dir_all(&dir).expect("dir");
    assert_eq!(
        load_overlay_file(&dir).expect_err("read").code,
        ErrorCode::ConfigCorrupt
    );
}

#[test]
fn save_creates_parents_and_write_errors() {
    let path = tmp("nested/dir/cfg.json");
    let mut overlay = Overlay::new();
    overlay.insert("backend".into(), json!("sqlite"));
    save_overlay_file(&path, &overlay).expect("save");
    assert!(path.exists());

    // Parent is a file → create_dir_all fails.
    let blocker = tmp("blocker");
    fs::write(&blocker, "x").expect("blocker");
    let under = blocker.join("child.json");
    let err = save_overlay_file(&under, &overlay).expect_err("mkdir");
    assert_eq!(err.code, ErrorCode::Internal);

    // Write to a directory path fails.
    let dir = tmp("outdir");
    fs::create_dir_all(&dir).expect("dir");
    let err = save_overlay_file(&dir, &overlay).expect_err("write");
    assert_eq!(err.code, ErrorCode::Internal);

    // Empty parent path skips create_dir_all (relative filename).
    let cwd = tmp("cwd");
    fs::create_dir_all(&cwd).expect("cwd");
    let prev = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("cd");
    save_overlay_file(Path::new("local.json"), &overlay).expect("relative");
    std::env::set_current_dir(prev).expect("cd back");
}

#[test]
fn save_is_owner_only_and_round_trips() {
    let path = tmp("priv/cfg.json");
    let mut overlay = Overlay::new();
    overlay.insert("credentials.embed_api_key".into(), json!("sk-secret-123"));
    // A credential path is sealed on the way out, so the save needs a key. The
    // key is installed for this thread only, so no test here can reach the
    // machine's real key store.
    crate::secret_seal::test_key::use_key([0x77; crate::secret_key::KEY_LEN]);
    save_overlay_file(&path, &overlay).expect("save");
    let reloaded = load_overlay_file(&path).expect("reload");
    assert_eq!(
        reloaded.get("credentials.embed_api_key"),
        Some(&json!("sk-secret-123"))
    );
    // No leftover temp file beside the target.
    assert!(!temp_sibling(&path).exists());
    // Temp siblings are unique per write so concurrent saves never clobber
    // each other before the rename.
    let first = temp_sibling(&path);
    let second = temp_sibling(&path);
    assert_ne!(first, second);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "config must be owner-only");
    }
}

#[test]
fn backup_reports_a_directory_creation_failure() {
    let dir = tmp("bak-blocked");
    fs::create_dir_all(&dir).expect("dir");
    let cfg = dir.join("deployment.json");
    fs::write(&cfg, "{}").expect("write");
    // A file where the backups directory should be blocks create_dir_all.
    fs::write(dir.join(BACKUP_DIR_NAME), "not a directory").expect("blocker");
    let err = backup_config_file(&cfg).expect_err("mkdir");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.contains("failed to create backup dir"),
        "{}",
        err.message
    );
}

#[test]
fn backup_copies_owner_only_and_skips_missing() {
    let path = tmp("bak/cfg.json");
    assert_eq!(backup_config_file(&path).expect("missing"), None);

    let mut overlay = Overlay::new();
    overlay.insert("backend".into(), json!("postgres"));
    save_overlay_file(&path, &overlay).expect("save");
    let backup = backup_config_file(&path).expect("backup").expect("some");
    assert!(backup.starts_with(path.parent().expect("parent").join(BACKUP_DIR_NAME)));
    assert_eq!(
        fs::read_to_string(&backup).expect("read"),
        fs::read_to_string(&path).expect("read")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&backup).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "backup must be owner-only");
    }
    // Unreadable source file surfaces an error naming the path.
    let dir = tmp("bak-dir");
    fs::create_dir_all(&dir).expect("dir");
    assert_eq!(
        backup_config_file(&dir).expect_err("read").code,
        ErrorCode::Internal
    );
}
