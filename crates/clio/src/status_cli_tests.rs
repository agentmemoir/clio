// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio status` command core over injected probes:
//! healthy/degraded exit codes, `--output` selection, credential redaction,
//! and the no-write guarantee.
//!
//! ## Owns
//! - Report/exit tests for `status_with`/`build` over `FakeDeps`
//!
//! ## Does not own
//! - The probe helpers (`status_probe_tests`) or the usage paths
//!   (`status_cli_fault_tests`)
//!
//! ## Boundary
//!
//! Tests here break when `clio status` exit mapping changes; they never change it.
//!
//! Keep this module focused on the status command core.

use super::*;
use crate::status_report::ComposeView;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so `temp_dir` never returns a path
/// another call already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

enum OpenMode {
    Memory,
    Fail,
}

enum EmbedderMode {
    None,
    Fail,
}

/// What the injected key-location probe answers.
enum KeyMode {
    /// No key exists yet.
    None,
    /// A key exists at this fixed location (never a real store read).
    Location(String),
    /// The store could not be consulted.
    Fail(String),
}

struct FakeDeps {
    env: HashMap<String, String>,
    ports: Vec<u16>,
    compose: ComposeView,
    setup: SetupView,
    open: OpenMode,
    embedder_mode: EmbedderMode,
    key: KeyMode,
    cwd: Option<PathBuf>,
    timestamp: String,
}

impl Default for FakeDeps {
    fn default() -> Self {
        Self {
            env: HashMap::new(),
            ports: Vec::new(),
            compose: ComposeView::NotMaterialized,
            setup: build_setup(&clio_config::Runtime::with_env(&|_| None), None),
            open: OpenMode::Memory,
            embedder_mode: EmbedderMode::None,
            key: KeyMode::None,
            cwd: None,
            timestamp: "2026-01-01T00:00:00Z".to_owned(),
        }
    }
}

impl StatusDeps for FakeDeps {
    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn open_store(&self, _resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
        match self.open {
            OpenMode::Memory => Ok(Box::new(
                SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("memory store"),
            )),
            OpenMode::Fail => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "connect failed: postgres://clio:s3cretpw@127.0.0.1:1/clio",
            )),
        }
    }

    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError> {
        match self.embedder_mode {
            EmbedderMode::None => Ok(None),
            EmbedderMode::Fail => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.provider `bogus` is unknown",
            )),
        }
    }

    fn scan_ports(&self) -> Vec<u16> {
        self.ports.clone()
    }

    fn compose(&self, _root: &Path) -> ComposeView {
        self.compose.clone()
    }

    fn setup(&self) -> SetupView {
        self.setup.clone()
    }

    fn key_location(&self) -> Result<Option<String>, String> {
        match &self.key {
            KeyMode::None => Ok(None),
            KeyMode::Location(location) => Ok(Some(location.clone())),
            KeyMode::Fail(reason) => Err(reason.clone()),
        }
    }

    fn timestamp(&self) -> String {
        self.timestamp.clone()
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "am-status-cli-{tag}-{}-{}-{}",
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
fn diagnose_view_redacts_finding_messages() {
    let findings = vec![OpsFinding::new(
        "BACKEND_UNREACHABLE",
        clio_ops::Severity::Error,
        "store",
        "connect failed: postgres://clio:s3cretpw@127.0.0.1:34310/clio",
    )];
    let redacted = redact_findings(findings);
    assert_eq!(redacted.len(), 1);
    assert!(
        redacted[0]
            .message
            .contains("postgres://***@127.0.0.1:34310/clio"),
        "{}",
        redacted[0].message
    );
    assert!(!redacted[0].message.contains("s3cretpw"));
}

#[test]
fn healthy_memory_no_docker_exits_0() {
    let deps = FakeDeps::default();
    let (report, output) = build(&args(&[]), &deps).expect("ok build");
    assert!(report.ok, "in-memory store with no Docker is healthy");
    assert_eq!(output, Output::Text);
    assert!(report.ports.is_empty(), "{:?}", report.ports);
    assert_eq!(report.compose, ComposeView::NotMaterialized);
    assert_eq!(status_with(&args(&[]), &deps), ExitCode::SUCCESS);
}

#[test]
fn json_output_selected() {
    let deps = FakeDeps::default();
    let (_, output) = build(&args(&["--output", "json"]), &deps).expect("ok build");
    assert_eq!(output, Output::Json);
}

#[test]
fn short_output_alias_selects_the_same_modes() {
    let deps = FakeDeps::default();
    let (_, output) = build(&args(&["-o", "json"]), &deps).expect("ok build");
    assert_eq!(output, Output::Json);

    let (_, output) = build(&args(&["-o=text"]), &deps).expect("ok build");
    assert_eq!(output, Output::Text);
}

#[test]
fn unreachable_postgres_exits_1() {
    let deps = FakeDeps {
        open: OpenMode::Fail,
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("build still returns a report");
    assert!(!report.ok);
    assert!(!report.diagnose.backend_reachable);
    assert_eq!(report.diagnose.findings[0].code, "BACKEND_UNREACHABLE");
    // The store error carried a password; it must never surface.
    let error = report.diagnose.error.expect("error recorded");
    assert!(error.contains("postgres://***@127.0.0.1:1/clio"), "{error}");
    assert!(!error.contains("s3cretpw"), "{error}");
    assert_eq!(status_with(&args(&[]), &deps), ExitCode::from(1));
}

#[test]
fn embedder_failure_degrades() {
    let deps = FakeDeps {
        embedder_mode: EmbedderMode::Fail,
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert!(!report.ok);
    assert_eq!(report.diagnose.findings[0].code, "EMBEDDER_UNAVAILABLE");
    assert_eq!(status_with(&args(&[]), &deps), ExitCode::from(1));
}

#[test]
fn compose_unavailable_does_not_degrade() {
    let deps = FakeDeps {
        compose: ComposeView::Unavailable {
            reason: "docker CLI not found".to_owned(),
        },
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert!(report.ok, "compose state is informational");
}

#[test]
fn ports_and_db_userinfo_are_reported_redacted() {
    let deps = FakeDeps {
        ports: vec![34300, 34302],
        env: HashMap::from([(
            "CLIO_DATABASE_URL".to_owned(),
            "postgres://user:topsecret@db:5432/clio".to_owned(),
        )]),
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert_eq!(report.ports, vec![34300, 34302]);
    assert_eq!(report.database.backend, "postgres");
    assert_eq!(report.database.url, "postgres://***@db:5432/clio");
    assert!(!report.database.url.contains("topsecret"));
    assert_eq!(report.database.encryption, "encrypted: no, remote");
    let text = render_text(&report);
    assert!(
        text.contains("postgres://***@db:5432/clio (encrypted: no, remote)"),
        "{text}"
    );
    assert!(!text.contains("topsecret"), "{text}");
}

#[test]
fn database_encryption_and_key_location_are_reported() {
    let deps = FakeDeps {
        env: HashMap::from([(
            "CLIO_DATABASE_URL".to_owned(),
            "postgres://user:topsecret@db.example.com:5432/clio?sslmode=require".to_owned(),
        )]),
        key: KeyMode::Location("macOS Keychain".to_owned()),
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert_eq!(report.database.backend, "postgres");
    assert_eq!(
        report.database.url,
        "postgres://***@db.example.com:5432/clio?sslmode=require"
    );
    assert!(!report.database.url.contains("topsecret"));
    assert_eq!(report.database.encryption, "encrypted, remote");
    assert_eq!(
        report.database.key_location.as_deref(),
        Some("macOS Keychain")
    );
    assert_eq!(report.database.key_error, None);
    let text = render_text(&report);
    assert!(
        text.contains(
            "postgres://***@db.example.com:5432/clio?sslmode=require (encrypted, remote)"
        ),
        "{text}"
    );
    assert!(text.contains("encryption key: macOS Keychain"), "{text}");
    let json = to_json(&report);
    assert!(
        json.contains("\"encryption\":\"encrypted, remote\""),
        "{json}"
    );
    assert!(
        json.contains("\"key_location\":\"macOS Keychain\""),
        "{json}"
    );
    assert!(!json.contains("topsecret"), "{json}");
}

#[test]
fn a_key_read_failure_is_reported_and_its_reason_is_redacted() {
    let deps = FakeDeps {
        key: KeyMode::Fail("keystore locked: postgres://user:pw@db:5432/clio".to_owned()),
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert_eq!(report.database.key_location, None);
    let reason = report.database.key_error.clone().expect("reason recorded");
    assert!(reason.contains("keystore locked"), "{reason}");
    assert!(!reason.contains("user:pw"), "{reason}");
    let text = render_text(&report);
    assert!(
        text.contains(
            "encryption key: could not be read \
             (keystore locked: postgres://***@db:5432/clio)"
        ),
        "{text}"
    );
}

#[test]
fn no_key_line_when_no_key_exists() {
    let deps = FakeDeps::default();
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert_eq!(report.database.key_location, None);
    assert_eq!(report.database.key_error, None);
    let text = render_text(&report);
    assert!(!text.contains("encryption key:"), "{text}");
}

#[test]
fn the_key_line_names_the_location_never_the_key_bytes() {
    let dir = temp_dir("key-bytes");
    let key_path = dir.join("clio.key");
    std::fs::write(&key_path, b"secret-key-bytes-0123456789abcdef").expect("key file");
    let location = clio_config::KeyLocation::File(key_path).to_string();
    let deps = FakeDeps {
        key: KeyMode::Location(location.clone()),
        ..FakeDeps::default()
    };
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert_eq!(
        report.database.key_location.as_deref(),
        Some(location.as_str())
    );
    let text = render_text(&report);
    assert!(
        text.contains(&format!("encryption key: {location}")),
        "{text}"
    );
    assert!(!text.contains("secret-key-bytes"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sqlite_memory_flag_resolves() {
    let deps = FakeDeps::default();
    let (report, _) = build(
        &args(&["--backend", "sqlite", "--db", "sqlite::memory:"]),
        &deps,
    )
    .expect("ok build");
    assert_eq!(report.database.backend, "sqlite");
    // FakeDeps ignores the resolved target, but the resolver result is visible.
    let resolved = clio_config::db_path::resolve_database(
        &|key: &str| deps.env(key),
        Some("sqlite::memory:"),
        Some("sqlite"),
    );
    assert_eq!(resolved.url, "sqlite::memory:");
}

#[test]
fn status_does_not_write_config_or_store() {
    let dir = temp_dir("no-write");
    let path = dir.join("deployment.json");
    let content =
        r#"{"embed.provider":"tei","credentials.embed_api_key":"sk-planted-secret-9999"}"#;
    std::fs::write(&path, content).expect("write config");
    let rt = clio_config::Runtime::with_deployment_file(&path);
    let deps = FakeDeps {
        setup: build_setup(&rt, Some(&path)),
        ..FakeDeps::default()
    };
    let before: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    let (report, _) = build(&args(&[]), &deps).expect("ok build");
    assert!(report.ok);
    assert_eq!(std::fs::read_to_string(&path).expect("read back"), content);
    let after: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        before, after,
        "status creates no files (no backup, no migration)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
