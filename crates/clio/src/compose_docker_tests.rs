// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the `clio compose` Docker invocation boundary: target detection,
//! the service allowlist/profile mapping, argument shapes, and the daemon
//! probe. Docker is always stubbed; no test starts a container.

use super::*;

fn assert_probe_err(runner: &dyn DockerRunner, needle: &str) {
    let err = probe(runner).expect_err("probe must fail");
    assert!(err.contains(needle), "probe error was: {err}");
}

struct FailingCapture;
impl DockerRunner for FailingCapture {
    fn capture(&self, _args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no docker",
        ))
    }
    fn run_inherited(&self, _args: &[&str], _dir: &Path) -> std::io::Result<i32> {
        unreachable!()
    }
}

struct FailingPlugin {
    compose_ok: bool,
    info_ok: bool,
}
impl DockerRunner for FailingPlugin {
    fn capture(&self, args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
        let ok = if args.first() == Some(&"info") {
            self.compose_ok && self.info_ok
        } else {
            self.compose_ok
        };
        Ok((i32::from(!ok), String::new()))
    }
    fn run_inherited(&self, _args: &[&str], _dir: &Path) -> std::io::Result<i32> {
        unreachable!()
    }
}

#[test]
fn detect_supported_targets() {
    assert_eq!(
        ComposeTarget::detect("macos", "aarch64"),
        Ok(ComposeTarget::MacArm64)
    );
    assert_eq!(
        ComposeTarget::detect("linux", "x86_64"),
        Ok(ComposeTarget::LinuxAmd64)
    );
}

#[test]
fn detect_unsupported_fails_closed() {
    let err = ComposeTarget::detect("windows", "x86_64").expect_err("windows must fail");
    assert!(
        err.contains("macOS arm64") && err.contains("Linux x86_64"),
        "{err}"
    );
    assert!(
        ComposeTarget::detect("macos", "x86_64").is_err(),
        "mac x86 must fail"
    );
    assert!(
        ComposeTarget::detect("linux", "aarch64").is_err(),
        "linux arm must fail"
    );
}

#[test]
fn overlays_match_checked_in_files() {
    assert_eq!(ComposeTarget::MacArm64.overlay(), "compose.arm64.yml");
    assert_eq!(ComposeTarget::LinuxAmd64.overlay(), "compose.amd64.yml");
}

#[test]
fn probe_requires_cli_then_daemon() {
    let ok = RecordingRunner::succeeding();
    assert_eq!(probe(&ok), Ok(()));
    let no_cli = FailingCapture;
    assert_probe_err(&no_cli, "docker CLI not found");
    let no_daemon = FailingPlugin {
        compose_ok: true,
        info_ok: false,
    };
    assert_probe_err(&no_daemon, "docker daemon is not reachable");
    let no_plugin = FailingPlugin {
        compose_ok: false,
        info_ok: false,
    };
    assert_probe_err(&no_plugin, "compose plugin not available");
}

/// Records every Docker invocation so argument shapes and environments can be
/// asserted.
pub(crate) struct RecordingRunner {
    calls: std::cell::RefCell<Vec<Vec<String>>>,
    envs: std::cell::RefCell<Vec<Vec<(String, String)>>>,
    exit: i32,
    config: std::cell::RefCell<Option<String>>,
}
impl RecordingRunner {
    pub(crate) fn succeeding() -> Self {
        Self {
            calls: std::cell::RefCell::new(Vec::new()),
            envs: std::cell::RefCell::new(Vec::new()),
            exit: 0,
            config: std::cell::RefCell::new(None),
        }
    }
    pub(crate) fn failing(exit: i32) -> Self {
        Self {
            calls: std::cell::RefCell::new(Vec::new()),
            envs: std::cell::RefCell::new(Vec::new()),
            exit,
            config: std::cell::RefCell::new(None),
        }
    }
    /// Serve `body` as the output of a `docker compose config` call, so a test
    /// can hand the extract-model check a resolved stack it wrote itself.
    pub(crate) fn with_config(self, body: String) -> Self {
        *self.config.borrow_mut() = Some(body);
        self
    }
    pub(crate) fn recorded(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
    /// The environment of the most recent `run_inherited_env` call.
    pub(crate) fn last_env(&self) -> Vec<(String, String)> {
        self.envs.borrow().last().cloned().unwrap_or_default()
    }
    fn record(&self, args: &[&str]) {
        self.calls
            .borrow_mut()
            .push(args.iter().map(ToString::to_string).collect());
    }
}
impl DockerRunner for RecordingRunner {
    fn capture(&self, args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
        self.record(args);
        // Only a resolved-stack call carries a body. A probe call collects
        // output that is never read, and an empty body is the honest answer for
        // a stack the test did not describe.
        let body = if args.contains(&"config") {
            self.config
                .borrow()
                .clone()
                .unwrap_or_else(no_extract_config)
        } else {
            String::new()
        };
        Ok((0, body))
    }
    fn run_inherited(&self, args: &[&str], _dir: &Path) -> std::io::Result<i32> {
        self.record(args);
        Ok(self.exit)
    }
    fn run_inherited_env(
        &self,
        args: &[&str],
        _dir: &Path,
        env: &[(String, String)],
    ) -> std::io::Result<i32> {
        self.record(args);
        self.envs.borrow_mut().push(env.to_vec());
        Ok(self.exit)
    }
}

#[test]
fn service_allowlist_rejects_unknown() {
    let err = Stack::from_services(&["postgres", "nvidia"]).expect_err("unknown must fail");
    assert!(err.contains("unknown service `nvidia`"), "{err}");
    assert!(err.contains("postgres, embed, rerank, extract"), "{err}");
}

#[test]
fn service_lists_map_to_exact_allowlisted_stacks() {
    assert_eq!(
        Stack::from_services(&["postgres"]).unwrap(),
        Stack::PostgresOnly
    );
    assert_eq!(Stack::PostgresOnly.services(), vec!["postgres"]);
    assert_eq!(Stack::PostgresOnly.profile(), None);
    assert_eq!(
        Stack::from_services(&["postgres", "embed", "rerank"]).unwrap(),
        Stack::LocalRetrieval
    );
    assert_eq!(
        Stack::LocalRetrieval.services(),
        vec!["postgres", "embed", "rerank"]
    );
    assert_eq!(Stack::LocalRetrieval.profile(), Some("local-retrieval"));
    assert_eq!(
        Stack::from_services(&["postgres", "embed", "rerank", "extract"]).unwrap(),
        Stack::LocalRft
    );
    assert_eq!(
        Stack::LocalRft.services(),
        vec!["postgres", "embed", "rerank", "extract"]
    );
    assert_eq!(Stack::LocalRft.profile(), Some("local-rft"));
    assert!(Stack::LocalRft.needs_extract());
    assert!(
        !Stack::from_services(&["postgres", "embed"])
            .unwrap()
            .needs_extract(),
        "local-retrieval does not include extract"
    );
}

#[test]
fn a_subset_stays_a_subset() {
    let only_extract = Stack::from_services(&["postgres", "extract"]).unwrap();
    assert_eq!(only_extract.services(), vec!["postgres", "extract"]);
    assert_eq!(only_extract.profile(), Some("local-rft"));
    let only_embed = Stack::from_services(&["postgres", "embed"]).unwrap();
    assert_eq!(only_embed.services(), vec!["postgres", "embed"]);
    assert_eq!(only_embed.profile(), Some("local-retrieval"));
}

#[test]
fn an_empty_service_list_starts_nothing() {
    let empty = Stack::from_services(&[]).unwrap();
    let services = empty.services();
    assert!(services.is_empty(), "{services:?}");
    assert_eq!(empty.profile(), None);
    assert!(!empty.needs_extract());
}

#[test]
fn too_many_services_fail_closed() {
    let err = Stack::from_services(&["postgres", "embed", "rerank", "extract", "postgres"])
        .expect_err("more services than exist must fail");
    assert!(err.contains("at most 4 services"), "{err}");
}

#[test]
fn parse_selection_accepts_names_and_profiles() {
    assert_eq!(
        Stack::parse_selection("postgres").unwrap(),
        Stack::PostgresOnly
    );
    assert_eq!(
        Stack::parse_selection("postgres,embed, rerank").unwrap(),
        Stack::LocalRetrieval
    );
    assert_eq!(
        Stack::parse_selection("local-rft").unwrap(),
        Stack::LocalRft
    );
    assert_eq!(
        Stack::parse_selection("local-retrieval").unwrap(),
        Stack::LocalRetrieval
    );
    assert!(Stack::parse_selection("").is_err());
    assert!(Stack::parse_selection("free-form").is_err());
}

#[test]
fn compose_args_use_fixed_shapes() {
    let up = compose_args(ComposeTarget::MacArm64, Stack::LocalRft, "up", &["-d"]);
    assert_eq!(
        up,
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "--profile",
            "local-rft",
            "up",
            "-d",
            "postgres",
            "embed",
            "rerank",
            "extract"
        ],
        "up must name the explicit services after the flags"
    );
    let stop = compose_args(ComposeTarget::MacArm64, Stack::LocalRetrieval, "stop", &[]);
    assert_eq!(
        stop,
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "--profile",
            "local-retrieval",
            "stop",
            "postgres",
            "embed",
            "rerank"
        ],
        "stop must name exactly the started services"
    );
    let down = compose_args(ComposeTarget::LinuxAmd64, Stack::PostgresOnly, "down", &[]);
    assert_eq!(
        down,
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.amd64.yml",
            "down"
        ],
        "down keeps the profile shape and never names services"
    );
    let ps = compose_args(ComposeTarget::LinuxAmd64, Stack::LocalRft, "ps", &["-a"]);
    assert_eq!(
        ps,
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.amd64.yml",
            "--profile",
            "local-rft",
            "ps",
            "-a"
        ],
        "other verbs keep today's shape"
    );
}

#[test]
fn real_docker_runner_spawns_the_cli() {
    // Exercises the real spawn paths. The outcome depends on the host (docker
    // present or not); both outcomes execute the full function bodies.
    let _ = RealDocker.capture(&["--version"], Path::new("."));
    let _ = RealDocker.run_inherited(&["--version"], Path::new("."));
    let _ = RealDocker.run_inherited_env(&["--version"], Path::new("."), &[]);
}

/// A resolved stack with no `extract` service, which is what a selection
/// outside `local-rft` resolves to.
pub(crate) fn no_extract_config() -> String {
    serde_json::json!({ "services": { "postgres": {} } }).to_string()
}

/// A resolved stack naming `extract` with its model under `root/models`, the
/// shape `docker compose config --format json` produces, so a test can drive
/// the extract-model check without a real engine.
pub(crate) fn extract_config(root: &Path) -> String {
    extract_config_with(root, crate::compose_assets::GGUF_FILE_NAME)
}

/// [`extract_config`] for a stack that names `name` instead of the shipped
/// default, the way a `CLIO_EXTRACT_GGUF` override does.
pub(crate) fn extract_config_with(root: &Path, name: &str) -> String {
    serde_json::json!({
        "services": {
            "extract": {
                "environment": { "LLAMA_ARG_MODEL": format!("/models/{name}") },
                "volumes": [{
                    "type": "bind",
                    "source": root.join("models").to_string_lossy(),
                    "target": "/models",
                    "read_only": true,
                }],
            }
        }
    })
    .to_string()
}
