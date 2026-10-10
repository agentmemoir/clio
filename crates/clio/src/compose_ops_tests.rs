// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the apply path's Compose lifecycle calls: exact service naming,
//! the empty-list no-op, `stop` never being `down`, the rotation SQL on stdin,
//! and the volume-removing recreate path. A recording stub stands in for
//! Docker; no test starts a container.

use super::*;
use crate::compose_cli::tests::{last_call, temp_root};
use crate::compose_docker::tests::RecordingRunner;

fn start(
    runner: &RecordingRunner,
    root: &Path,
    services: &[&str],
    env: &[(String, String)],
) -> Result<(), String> {
    start_services(
        runner,
        ComposeTarget::MacArm64,
        root.to_str(),
        services,
        env,
    )
}

#[test]
fn an_empty_service_list_starts_nothing() {
    let root = temp_root("ops-start-empty");
    let runner = RecordingRunner::succeeding();
    start(&runner, &root, &[], &[]).expect("an empty list is a no-op, not an error");
    assert!(
        runner.recorded().is_empty(),
        "no Docker call at all for an empty list: {:?}",
        runner.recorded()
    );
    assert!(
        !root.join("compose.yml").exists(),
        "nothing is materialized when nothing starts"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_managed_run_starts_postgres_and_nothing_else() {
    let root = temp_root("ops-start-managed");
    let runner = RecordingRunner::succeeding();
    start(&runner, &root, &["postgres"], &[]).expect("start the managed database");
    assert_eq!(
        last_call(&runner),
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "up",
            "-d",
            "--wait",
            "--wait-timeout",
            super::HEALTH_WAIT_SECONDS,
            "postgres"
        ],
        "a hosted-models run must name only postgres"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_local_models_run_starts_exactly_the_named_services() {
    let root = temp_root("ops-start-local");
    let runner = RecordingRunner::succeeding();
    let env = vec![("CLIO_POSTGRES_PORT".to_owned(), "34310".to_owned())];
    start(&runner, &root, &["postgres", "embed", "rerank"], &env).expect("start");
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
            "--wait",
            "--wait-timeout",
            super::HEALTH_WAIT_SECONDS,
            "postgres",
            "embed",
            "rerank"
        ]
    );
    assert_eq!(
        runner.last_env(),
        env,
        "the chosen port must reach the child process, so the container and the stored address agree"
    );
    let env_file = std::fs::read_to_string(root.join(".env")).expect(".env written");
    assert!(
        !env_file.contains("POSTGRES_PASSWORD"),
        "no password belongs in .env: {env_file}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stop_uses_the_stop_verb_with_exactly_the_started_services() {
    let root = temp_root("ops-stop");
    let runner = RecordingRunner::succeeding();
    let services = ["postgres", "embed", "rerank"];
    start(&runner, &root, &services, &[]).expect("start");
    stop_services(&runner, ComposeTarget::MacArm64, root.to_str(), &services).expect("stop");
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
            "stop",
            "postgres",
            "embed",
            "rerank"
        ]
    );
    for call in runner.recorded() {
        assert!(
            !call.iter().any(|arg| arg == "down"),
            "stop must never run down: {call:?}"
        );
        assert!(
            !call.iter().any(|arg| arg == "-v" || arg == "--volumes"),
            "stop must never remove volumes: {call:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stop_maps_a_non_zero_exit_to_an_error() {
    let root = temp_root("ops-stop-fail");
    let runner = RecordingRunner::failing(1);
    let err = stop_services(
        &runner,
        ComposeTarget::MacArm64,
        root.to_str(),
        &["postgres"],
    )
    .expect_err("a failed stop must be reported");
    assert!(err.contains("stop"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn recreate_runs_down_with_volume_removal() {
    let root = temp_root("ops-recreate");
    let runner = RecordingRunner::succeeding();
    recreate_volume(
        &runner,
        ComposeTarget::MacArm64,
        root.to_str(),
        &["postgres"],
    )
    .expect("recreate");
    assert_eq!(
        last_call(&runner),
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "down",
            "-v"
        ],
        "the recreate path is the one place the volume is removed"
    );
    let _ = std::fs::remove_dir_all(&root);
}
