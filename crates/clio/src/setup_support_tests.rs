// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared test support for the `clio setup` suites:
//! a Docker runner stub and temp-directory helpers.
//!
//! ## Owns
//! - `StubDocker`, the recording `DockerRunner` test double
//! - Unique temp-directory creation for setup tests
//!
//! ## Does not own
//! - Any production behavior (this module is `#[cfg(test)]` only)
//!
//! ## Boundary
//!
//! Changes to the Docker runner seam may require changes here.
//! Changes to setup behavior should not require changes here.
//!
//! Keep this module focused on setup test doubles.

use crate::compose_docker::DockerRunner;
use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A `DockerRunner` stub: records the arguments and environment Docker would
/// have received, and lets a test choose probe and run outcomes.
pub(crate) struct StubDocker {
    calls: RefCell<Vec<Vec<String>>>,
    envs: RefCell<Vec<Vec<(String, String)>>>,
    probe_ok: bool,
    run_code: i32,
    fail_matching: Option<String>,
    /// When set, a `config` query answers with a resolved stack whose `extract`
    /// service mounts `models/` under this root, the shape a `local-rft` selection
    /// resolves to.
    extract_root: Option<PathBuf>,
}

impl StubDocker {
    /// Probe succeeds; `docker compose up` exits 0.
    pub(crate) fn ok() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            envs: RefCell::new(Vec::new()),
            probe_ok: true,
            run_code: 0,
            fail_matching: None,
            extract_root: None,
        }
    }

    /// Probe succeeds and the resolved stack holds an `extract` service whose model
    /// lives under `root/models`.
    pub(crate) fn with_extract_model(root: &Path) -> Self {
        Self {
            extract_root: Some(root.to_path_buf()),
            ..Self::ok()
        }
    }

    /// Probe succeeds; `docker compose up` exits non-zero.
    pub(crate) fn failing_run() -> Self {
        Self {
            run_code: 7,
            ..Self::ok()
        }
    }

    /// Every run succeeds except one carrying `needle` as an exact argument.
    ///
    /// The rotation call is the motivating case: `up` has to succeed so the
    /// probe can run, and `exec` has to fail so the recreate path is reachable.
    pub(crate) fn failing_when_arg(needle: &str) -> Self {
        Self {
            fail_matching: Some(needle.to_owned()),
            ..Self::ok()
        }
    }

    /// The exit code this invocation reports.
    fn code_for(&self, args: &[&str]) -> i32 {
        match &self.fail_matching {
            Some(needle) if args.contains(&needle.as_str()) => 7,
            _ => self.run_code,
        }
    }

    /// Every `run_inherited` argument vector, flattened in order.
    pub(crate) fn invocations(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }

    /// Whether any `docker compose up` invocation carried `arg` (for example a
    /// compose `--profile` value).
    pub(crate) fn ran_arg(&self, arg: &str) -> bool {
        self.calls
            .borrow()
            .iter()
            .flatten()
            .any(|candidate| candidate == arg)
    }

    /// The environment of the most recent environment-carrying run.
    pub(crate) fn last_env(&self) -> Vec<(String, String)> {
        self.envs.borrow().last().cloned().unwrap_or_default()
    }

    fn record(&self, args: &[&str]) {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|a| (*a).to_owned()).collect());
    }
}

impl DockerRunner for StubDocker {
    fn capture(&self, args: &[&str], _dir: &Path) -> io::Result<(i32, String)> {
        // A resolved-stack query answers with a resolved stack. The selections
        // these tests start normally hold no `extract` service, and an empty body is
        // a shape the engine never prints; a stub built with `with_extract_model`
        // answers with the extract service and its model instead.
        if args.contains(&"config") {
            let body = match &self.extract_root {
                Some(root) => crate::compose_docker::tests::extract_config(root),
                None => crate::compose_docker::tests::no_extract_config(),
            };
            return Ok((0, body));
        }
        Ok((i32::from(!self.probe_ok), String::new()))
    }

    fn run_inherited(&self, args: &[&str], _dir: &Path) -> io::Result<i32> {
        self.record(args);
        Ok(self.code_for(args))
    }

    fn run_inherited_env(
        &self,
        args: &[&str],
        _dir: &Path,
        env: &[(String, String)],
    ) -> io::Result<i32> {
        self.record(args);
        self.envs.borrow_mut().push(env.to_vec());
        Ok(self.code_for(args))
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The Clio encryption key every setup test seals under.
///
/// The deployment config holds API keys and a database URL, and those are sealed
/// on the way to disk, so a setup test needs a key. It must be a process key
/// rather than a per-thread one: nothing here runs on a spawned thread, but the
/// process cache is what a shipped build uses too, and a test must not depend on
/// which of the two it happens to reach.
const TEST_SEAL_KEY: [u8; clio_config::secret_key::KEY_LEN] =
    [0x6b; clio_config::secret_key::KEY_LEN];

/// Point every seal in this test process at the shared test key.
///
/// A test that seals a value must never reach the machine's real key store: the
/// operating system can prompt for access to the keychain, and a machine that
/// has both a keychain entry and a key file makes every sealing call fail with a
/// two-key conflict. Installing the process key here keeps the suite hermetic.
/// The call is idempotent ("first call wins"), so every fixture can call it.
pub(crate) fn ensure_test_key() {
    clio_config::secret_seal::test_key::install_process_key(TEST_SEAL_KEY);
}

/// A per-test scratch directory, removed when the guard drops.
///
/// Holding the guard is what makes the test own the directory: a finished test
/// leaves nothing behind under the shared temp root, and a panic mid-test still
/// removes it.
pub(crate) struct TempDir(PathBuf);

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh unique temp directory for one test, removed when the guard drops.
///
/// Installing the shared test key here keeps every setup suite off the machine's
/// real key store: without it, the first test that writes a credential to the
/// deployment file would ask the operating system for a Clio encryption key.
pub(crate) fn temp_dir(tag: &str) -> TempDir {
    ensure_test_key();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "am-setup-{tag}-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    TempDir(dir)
}

/// An injected environment lookup that points `CLIO_DEPLOYMENT_CONFIG` at `path`
/// and hides everything else (hermetic setup tests).
pub(crate) fn env_at(path: &Path) -> impl Fn(&str) -> Option<String> + '_ {
    move |key: &str| {
        if key == clio_config::DEPLOYMENT_CONFIG_ENV {
            Some(path.to_string_lossy().into_owned())
        } else {
            None
        }
    }
}

/// One scripted `clio setup` run's three outputs.
pub(crate) struct SetupRun {
    pub(crate) code: std::process::ExitCode,
    pub(crate) out: String,
    pub(crate) err: String,
}

/// A database probe that refuses and an endpoint probe that answers.
///
/// The default for a suite that is about what a run *wrote* rather than about
/// what a probe *said*. A refusal on a closed port is retried 30 times at 500 ms
/// because it carries no server error to prove it settled, so it costs about
/// fifteen seconds — and a managed run pays it twice, once for the post-start
/// test and once more after the rotation attempt.
pub(crate) fn failing_database_probe() -> crate::setup_test_seam::Probes {
    crate::setup_test_seam::Probes::new(
        |_address: &str| Err("connection refused".to_owned()),
        |url: &str, _| Ok(format!("{url} answered")),
    )
}

/// A database probe that answers and an endpoint probe that refuses.
///
/// The other half of [`failing_database_probe`]. A SQLite install has no
/// connection to test, so a local sidecar is the only thing that can fail, and it
/// is the only way to reach the post-failure question rather than the rotation
/// path a managed database takes.
pub(crate) fn failing_endpoint_probe() -> crate::setup_test_seam::Probes {
    crate::setup_test_seam::Probes::new(
        |address: &str| Ok(format!("connected to {address}")),
        |_url: &str, _| Err("the sidecar did not answer".to_owned()),
    )
}

/// A database probe that refuses once and then answers, with an endpoint probe
/// that always answers.
///
/// The rotation path is the one behaviour neither of the two probes above can
/// reach. Repairing a managed database is *refuse, then answer*: the first test
/// fails, Clio rotates the password, and the retest must pass or the run gives
/// up. A probe that always refuses never reaches the second test, and a probe
/// that always answers never reaches the first — so the repair can only be
/// observed with a probe that changes its mind once.
///
/// The refusal names the credentials on purpose. A refused socket is not a wrong
/// password, and the production code only bothers to rotate after a reason that
/// mentions them, so a reason like "connection refused" would skip the repair
/// this test is about — which is correct behaviour, and useless here.
///
/// The count lives behind an atomic because the probe is a plain closure and
/// cannot borrow. It is shared within this one run, so the ordering it depends on
/// is the order the run itself asks in, and nothing outside it can observe it.
pub(crate) fn fails_once_database_probe() -> crate::setup_test_seam::Probes {
    let asked = AtomicU64::new(0);
    crate::setup_test_seam::Probes::new(
        move |address: &str| {
            if asked.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("password authentication failed for user \"clio\"".to_owned())
            } else {
                Ok(format!("connected to {address}"))
            }
        },
        |url: &str, _| Ok(format!("{url} answered")),
    )
}

/// Run the real `setup_core` over injected streams, this test's environment, the
/// given Docker runner, and the given probes.
///
/// The key file is named inside the same directory as the config, so the key-store
/// marker a run records lands in the test's own temporary directory. Without it the
/// marker path resolves from the real environment, a run that applies an answer
/// writes a `key-source` file into the data directory of whoever is running the
/// suite, and every later run on that machine reads it and skips the question.
pub(crate) fn invoke_setup(
    args: &[String],
    input: &str,
    is_tty: bool,
    config: &Path,
    runner: &dyn DockerRunner,
    probes: &crate::setup_test_seam::Probes,
) -> SetupRun {
    // A run that reaches the fetch gets its bytes from the shared stub, so no call
    // site here needs to know that a model can be downloaded at all. A kilobyte is
    // enough to prove the file arrived and keeps the suite off the operator's disk.
    invoke_setup_with_fetcher(
        args,
        input,
        is_tty,
        config,
        runner,
        &crate::setup_extract_model::tests::StubFetcher::writing(1_024),
        probes,
    )
}

/// [`invoke_setup`] with the model fetcher chosen by the caller, for the runs that
/// are about the download itself.
pub(crate) fn invoke_setup_with_fetcher(
    args: &[String],
    input: &str,
    is_tty: bool,
    config: &Path,
    runner: &dyn DockerRunner,
    fetcher: &dyn crate::setup_extract_model::ModelFetcher,
    probes: &crate::setup_test_seam::Probes,
) -> SetupRun {
    let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let key_file = config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("clio.key")
        .to_string_lossy()
        .into_owned();
    let env = move |key: &str| match key {
        clio_config::DEPLOYMENT_CONFIG_ENV => Some(config.to_string_lossy().into_owned()),
        clio_config::KEY_FILE_ENV => Some(key_file.clone()),
        _ => None,
    };
    let code = crate::setup_cli::setup_core(
        args.to_vec(),
        &mut reader,
        &mut out,
        &mut err,
        is_tty,
        &env,
        runner,
        fetcher,
        probes,
    );
    SetupRun {
        code,
        out: String::from_utf8(out).expect("stdout is utf8"),
        err: String::from_utf8(err).expect("stderr is utf8"),
    }
}

/// The Compose test database the workspace Makefile exports for the suite.
///
/// The managed path has to dial something real for its post-start test, and this
/// is the same database every `clio-store` Postgres test opens. Tests pass
/// `--allow-untested`, so a machine without it still completes; the suite's
/// convention is that it is there.
pub(crate) const TEST_PG_HOST: &str = "127.0.0.1";
pub(crate) const TEST_PG_PORT: &str = "34310";
pub(crate) const TEST_PG_USER: &str = "clio";
pub(crate) const TEST_PG_NAME: &str = "clio";
pub(crate) const TEST_PG_PASSWORD: &str = "clio";

/// One item per `(id, subject)` pair, in a SQLite store at `path` so the setup
/// switch gate has something to count.
pub(crate) fn seed_sqlite_store(path: &Path, rows: &[(&str, &str)]) {
    use clio_store::{LocalDevKms, SqliteStore, Store as _};
    use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
    use std::sync::Arc;

    let store = SqliteStore::open(path, Arc::new(LocalDevKms::new())).expect("open the seed store");
    for (id, subject) in rows {
        let item = MemoryItem {
            id: (*id).to_owned(),
            bank_id: "setup-seed".to_owned(),
            subject_id: (*subject).to_owned(),
            kind: ItemKind::Semantic,
            category: Some(SemanticCategory::ToolConfig),
            episodic_type: None,
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: Some(0.6),
            admission_score: Some(0.5),
            source_ref: Some("seed".to_owned()),
            snapshot: Some(serde_json::json!({ "seed": id })),
            gist: Some("switch-gate seed".to_owned()),
            valid_from: "t0".to_owned(),
            valid_until: None,
            tx_from: "t0".to_owned(),
            tx_until: None,
            created_at: "t0".to_owned(),
            updated_at: "t0".to_owned(),
            context: None,
        };
        store.create_memory_item(&item, "actor").expect("seed item");
    }
}
