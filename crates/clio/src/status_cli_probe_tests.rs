// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio status` probe helpers: opening the selected
//! backend read path, reading/masking the setup view, the no-write guarantee,
//! and the tolerant Compose service-state probe.
//!
//! ## Owns
//! - `open_chosen`, `build_setup`, and `probe_compose` tests
//!
//! ## Does not own
//! - The report/exit tests (`status_cli_tests`) or the usage paths
//!   (`status_cli_fault_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a status probe changes; they never change it.
//!
//! Keep this module focused on the status probe helpers.

use super::*;
use crate::compose_docker::DockerRunner;
use crate::status_report::{ComposeView, ServiceState};
use serde_json::json;
use std::cell::RefCell;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so `temp_dir` never returns a path
/// another call already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "am-status-probe-{tag}-{}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn open_chosen_opens_memory_and_rejects_unknown_backend() {
    let memory = ResolvedDatabase {
        backend: "sqlite".to_owned(),
        url: "sqlite::memory:".to_owned(),
    };
    assert!(open_chosen(&memory).is_ok());

    let dir = temp_dir("open-file");
    let path = dir.join("clio.db");
    std::fs::write(&path, b"").expect("pre-create store file");
    let file = ResolvedDatabase {
        backend: "sqlite".to_owned(),
        url: path.to_string_lossy().into_owned(),
    };
    assert!(open_chosen(&file).is_ok());
    let _ = std::fs::remove_dir_all(&dir);

    let bad = ResolvedDatabase {
        backend: "redis".to_owned(),
        url: "redis://x".to_owned(),
    };
    let err = open_chosen(&bad).err().expect("unknown backend");
    assert!(err.message.contains("sqlite|postgres"), "{}", err.message);

    let pg = ResolvedDatabase {
        backend: "postgres".to_owned(),
        url: "postgres://nope:nope@127.0.0.1:1/none".to_owned(),
    };
    assert!(
        open_chosen(&pg).is_err(),
        "unreachable postgres is degraded"
    );
}

#[test]
fn open_chosen_missing_sqlite_file_degrades_without_creating() {
    let dir = temp_dir("missing-db");
    let path = dir.join("absent.db");
    let resolved = ResolvedDatabase {
        backend: "sqlite".to_owned(),
        url: path.to_string_lossy().into_owned(),
    };
    let err = open_chosen(&resolved)
        .err()
        .expect("a missing sqlite file must degrade, not bootstrap");
    assert!(err.message.contains("does not exist"), "{}", err.message);
    assert!(
        !path.exists(),
        "read-only status must not create the database file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_setup_masks_secret_and_reports_config() {
    let dir = temp_dir("setup");
    let path = dir.join("deployment.json");
    // The api key sits at a secret path, so the file must be written through
    // the save path: a hand-written plaintext file is refused by the loader,
    // and then nothing masks because nothing loads.
    crate::setup_support_tests::ensure_test_key();
    let mut seed = clio_config::Runtime::with_deployment_file(&path);
    for (key, value) in [
        ("embed.provider", "tei"),
        ("embed.url", "http://127.0.0.1:34311"),
        ("embed.model", "m"),
        ("credentials.embed_api_key", "sk-verysecretkey12345"),
    ] {
        let clio_config::error::ToolResult::Ok { .. } =
            seed.config_set(key, json!(value), clio_config::ConfigScope::Deployment)
        else {
            panic!("config_set {key}");
        };
    }
    let rt = clio_config::Runtime::with_deployment_file(&path);
    let view = build_setup(&rt, Some(&path));
    assert_eq!(
        view.deployment_config.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    let embed = view.embed.expect("embed set");
    assert_eq!(embed.provider, "tei");
    assert_eq!(embed.url, "http://127.0.0.1:34311");
    assert!(view.rerank.is_none());
    let masked = view
        .secrets
        .get("credentials.embed_api_key")
        .expect("masked");
    assert_eq!(masked, "****2345");
    assert!(!masked.contains("verysecret"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_setup_masks_secret_keys_outside_the_watched_allowlist() {
    let dir = temp_dir("setup-extra-secret");
    let path = dir.join("deployment.json");
    std::fs::write(
        &path,
        r#"{"extract.token":"tok-abcdefghijklmnop","embed.provider":"tei"}"#,
    )
    .expect("write config");
    let rt = clio_config::Runtime::with_deployment_file(&path);
    let view = build_setup(&rt, Some(&path));
    let masked = view.secrets.get("extract.token").expect("masked");
    assert_eq!(masked, "****mnop");
    assert!(
        !view
            .secrets
            .values()
            .any(|v| v.contains("abcdefghijklmnop")),
        "no discovered secret may survive masking"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_setup_absent_config_is_none() {
    let dir = temp_dir("no-config");
    let missing = dir.join("deployment.json");
    let rt = clio_config::Runtime::with_deployment_file(&missing);
    let view = build_setup(&rt, Some(&missing));
    assert!(
        view.deployment_config.is_none(),
        "absent config reports none"
    );
    assert!(view.embed.is_none() && view.rerank.is_none());
    assert!(view.secrets.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

struct FakeRunner {
    result: io::Result<(i32, String)>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeRunner {
    fn ok(text: &str) -> Self {
        Self {
            result: Ok((0, text.to_owned())),
            calls: RefCell::new(Vec::new()),
        }
    }
    fn exit(code: i32, text: &str) -> Self {
        Self {
            result: Ok((code, text.to_owned())),
            calls: RefCell::new(Vec::new()),
        }
    }
    fn missing() -> Self {
        Self {
            result: Err(io::Error::new(io::ErrorKind::NotFound, "no docker")),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl DockerRunner for FakeRunner {
    fn capture(&self, args: &[&str], _dir: &Path) -> io::Result<(i32, String)> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(ToString::to_string).collect());
        match &self.result {
            Ok((code, text)) => Ok((*code, text.clone())),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        }
    }
    fn run_inherited(&self, _args: &[&str], _dir: &Path) -> io::Result<i32> {
        unreachable!("status never runs an inherited Docker command")
    }
}

#[test]
fn probe_compose_not_materialized_skips_docker() {
    let dir = temp_dir("no-compose");
    let runner = FakeRunner::ok("[]");
    assert_eq!(
        probe_compose(&runner, &dir, "macos", "aarch64"),
        ComposeView::NotMaterialized
    );
    assert!(
        runner.calls.borrow().is_empty(),
        "no docker without a project"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn probe_compose_available_reads_service_state() {
    let dir = temp_dir("compose-ok");
    std::fs::write(dir.join("compose.yml"), "services: {}\n").expect("compose.yml");
    let runner = FakeRunner::ok(
        r#"[{"Service":"postgres","State":"running"},{"Service":"embed","State":"exited"}]"#,
    );
    let view = probe_compose(&runner, &dir, "macos", "aarch64");
    assert_eq!(
        view,
        ComposeView::Available {
            services: vec![
                ServiceState {
                    service: "embed".to_owned(),
                    state: "exited".to_owned()
                },
                ServiceState {
                    service: "postgres".to_owned(),
                    state: "running".to_owned()
                },
            ]
        }
    );
    let calls = runner.calls.borrow();
    let call = &calls[0];
    assert!(call.contains(&"ps".to_owned()), "{call:?}");
    assert!(call.contains(&"--format".to_owned()), "{call:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn probe_compose_states_and_failures() {
    let dir = temp_dir("compose-fail");
    std::fs::write(dir.join("compose.yml"), "services: {}\n").expect("compose.yml");

    let unsupported = probe_compose(&FakeRunner::ok("[]"), &dir, "windows", "x86_64");
    assert!(
        matches!(unsupported, ComposeView::Unavailable { ref reason } if reason.contains("unsupported platform")),
        "{unsupported:?}"
    );

    let nonzero = probe_compose(
        &FakeRunner::exit(1, "error: no such service: x\nmore"),
        &dir,
        "linux",
        "x86_64",
    );
    assert_eq!(
        nonzero,
        ComposeView::Unavailable {
            reason: "error: no such service: x more".to_owned()
        }
    );

    let blank = probe_compose(&FakeRunner::exit(1, "   "), &dir, "linux", "x86_64");
    assert_eq!(
        blank,
        ComposeView::Unavailable {
            reason: "docker compose ps failed".to_owned()
        }
    );

    let missing = probe_compose(&FakeRunner::missing(), &dir, "macos", "aarch64");
    assert!(
        matches!(missing, ComposeView::Unavailable { ref reason } if reason.contains("docker CLI not found")),
        "{missing:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
