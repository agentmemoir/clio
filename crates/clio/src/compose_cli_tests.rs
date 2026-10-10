// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! End-to-end tests for the `clio compose` command surface with a stubbed
//! Docker runner: materialization ordering, argument shapes, exit codes,
//! selection resolution, the no-partial-writes failure rule, and `down`
//! reversing `up`. No test starts a real container.

use super::*;
use crate::compose_docker::tests::RecordingRunner;

pub(crate) fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

pub(crate) fn flags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

pub(crate) fn last_call(runner: &RecordingRunner) -> Vec<String> {
    runner.recorded().last().expect("docker invoked").clone()
}

pub(crate) fn probe_calls(runner: &RecordingRunner) -> usize {
    runner
        .recorded()
        .iter()
        .filter(|call| call.first().map(String::as_str) == Some("compose"))
        .count()
}

#[test]
fn up_materializes_and_runs_the_selected_profile() {
    let root = temp_root("up-retrieval");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[
            ("dir", root.to_str().unwrap()),
            ("services", "postgres,embed,rerank"),
        ]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(
        probe_calls(&runner) >= 2,
        "probe must call compose version + docker info"
    );
    assert_eq!(
        last_call(&runner),
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "--profile",
            "local-retrieval",
            "up",
            "-d",
            "postgres",
            "embed",
            "rerank"
        ],
        "up must use the checked-in files, the arch overlay, the selected profile, and the explicit services"
    );
    for path in [
        "compose.yml",
        "compose.arm64.yml",
        "sql/001_core.sql",
        "sql/002_vectors_postgres.sql",
        "models/README.md",
        ".env",
    ] {
        assert!(root.join(path).exists(), "{path} must be materialized");
    }
    assert_eq!(
        compose_env::read_marker(&root),
        Some("local-retrieval".into())
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn up_with_explicit_postgres_only_runs_no_profile() {
    let root = temp_root("up-postgres");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let call = last_call(&runner);
    assert!(
        !call.contains(&"--profile".to_string()),
        "postgres-only starts no profile: {call:?}"
    );
    assert_eq!(
        call.last().map(String::as_str),
        Some("postgres"),
        "postgres must still be named explicitly: {call:?}"
    );
    assert_eq!(compose_env::read_marker(&root), Some("postgres".into()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn up_writes_no_password_anywhere() {
    let root = temp_root("up-no-password");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(
        runner.last_env().is_empty(),
        "the managed database has no password, so there is nothing to pass: {:?}",
        runner.last_env()
    );
    let env_file = std::fs::read_to_string(root.join(".env")).expect(".env written");
    assert!(
        !env_file.contains("POSTGRES_PASSWORD"),
        "no password line may exist in .env: {env_file}"
    );
    for line in [
        "CLIO_POSTGRES_USER=clio",
        "CLIO_POSTGRES_DB=clio",
        "CLIO_POSTGRES_PORT=34310",
    ] {
        assert!(env_file.contains(line), "missing `{line}` in .env");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn up_keeps_a_user_owned_env_file() {
    let root = temp_root("up-keep-env");
    std::fs::write(root.join(".env"), "CLIO_POSTGRES_USER=mine\n").expect("user env");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        std::fs::read_to_string(root.join(".env")).unwrap(),
        "CLIO_POSTGRES_USER=mine\n",
        "an existing user .env is never overwritten"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prompt_defaults_to_local_retrieval_on_empty_answer() {
    let root = temp_root("prompt-default");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
        &|| Some(String::new()),
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(last_call(&runner).contains(&"local-retrieval".to_string()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prompt_accepts_menu_answers() {
    let root = temp_root("prompt-menu");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
        &|| Some("3".to_string()),
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(compose_env::read_marker(&root), Some("local-rft".into()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn non_interactive_stdin_without_services_fails_closed() {
    let root = temp_root("non-interactive");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "no --services and no interactive stdin must be refused"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failed_probe_leaves_no_files() {
    struct DeadRunner;
    impl DockerRunner for DeadRunner {
        fn capture(&self, _args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "docker missing",
            ))
        }
        fn run_inherited(&self, _args: &[&str], _dir: &Path) -> std::io::Result<i32> {
            unreachable!()
        }
    }
    let root = temp_root("probe-fail");
    let code = compose_up(
        &flags(&[
            ("dir", root.to_str().unwrap()),
            ("services", "local-retrieval"),
        ]),
        &DeadRunner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::from(1));
    let entries: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        entries.is_empty(),
        "no files may be written when Docker is unreachable: {entries:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn docker_exit_status_propagates() {
    let root = temp_root("docker-fail");
    let runner = RecordingRunner::failing(125);
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(
        code,
        ExitCode::from(125),
        "docker's exit status must propagate"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn down_reverses_up_using_the_recorded_selection() {
    let root = temp_root("down");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "local-rft")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    runner.recorded().clear();
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        last_call(&runner),
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "--profile",
            "local-rft",
            "down"
        ],
        "down must re-use the recorded selection so profile services are removed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn down_is_idempotent_on_a_stopped_stack() {
    let root = temp_root("down-idempotent");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    for _ in 0..2 {
        let code = compose_down(
            &flags(&[("dir", root.to_str().unwrap())]),
            &runner,
            ComposeTarget::MacArm64,
        );
        assert_eq!(code, ExitCode::SUCCESS, "down must be re-runnable");
    }
    let _ = std::fs::remove_dir_all(&root);
}
