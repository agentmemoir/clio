// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the extract-model check: the host path it derives from a resolved
//! stack, the failures it reports, and the Docker call it makes. The engine is
//! stubbed with the resolved stack it would print, so no test starts a
//! container and no test depends on what is installed on the host.

use super::*;
use crate::compose_docker::tests::{RecordingRunner, extract_config, extract_config_with};

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-model-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

/// Place a stand-in model file where the resolved stack will look for it.
fn write_model(root: &Path, name: &str) {
    let models = root.join("models");
    std::fs::create_dir_all(&models).expect("create models");
    std::fs::write(models.join(name), b"stub").expect("write the model");
}

fn check(root: &Path, stack: Stack, runner: &dyn DockerRunner) -> Result<(), String> {
    verify_extract_model(root, ComposeTarget::MacArm64, stack, runner)
}

#[test]
fn a_stack_without_extract_needs_no_model() {
    let root = temp_root("no-extract");
    // The materialized root holds a `models/` directory and no model in it,
    // which is the state that used to pass a directory-level check.
    std::fs::create_dir_all(root.join("models")).expect("create models");
    let runner = RecordingRunner::succeeding();
    assert_eq!(
        check(&root, Stack::PostgresOnly, &runner),
        Ok(()),
        "a stack Compose resolves without extract has no model to require"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_query_uses_the_fixed_config_shape() {
    let root = temp_root("config-args");
    let runner = RecordingRunner::succeeding();
    check(&root, Stack::LocalRft, &runner).expect("no extract service in the stub stack");
    assert_eq!(
        runner.recorded().last().expect("docker invoked").clone(),
        vec![
            "compose",
            "-f",
            "compose.yml",
            "-f",
            "compose.arm64.yml",
            "--profile",
            "local-rft",
            "config",
            "--format",
            "json"
        ],
        "the check must ask Compose with the materialized files and no service list"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_model_the_engine_names_is_the_one_checked() {
    let root = temp_root("named");
    write_model(&root, "custom.gguf");
    let runner =
        RecordingRunner::succeeding().with_config(extract_config_with(&root, "custom.gguf"));
    assert_eq!(
        check(&root, Stack::LocalRft, &runner),
        Ok(()),
        "the name Compose resolves is the name that must exist"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The defect this check exists for: the engine names a file that is absent
/// while a file with the shipped default name is present. Reading the default
/// instead of the engine's answer would pass this stack.
#[test]
fn the_engine_answer_wins_over_another_name_on_disk() {
    let root = temp_root("engine-wins");
    write_model(&root, GGUF_FILE_NAME);
    let runner =
        RecordingRunner::succeeding().with_config(extract_config_with(&root, "absent.gguf"));
    let err = check(&root, Stack::LocalRft, &runner)
        .expect_err("the absent file Compose named must fail the check");
    assert!(err.contains("absent.gguf"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_model_names_the_shipped_fetch_command() {
    let root = temp_root("missing-default");
    let runner = RecordingRunner::succeeding().with_config(extract_config(&root));
    let err = check(&root, Stack::LocalRft, &runner).expect_err("a missing model must fail");
    assert!(err.contains(GGUF_FILE_NAME), "{err}");
    assert!(
        err.contains("hf download"),
        "the shipped model has a documented fetch command: {err}"
    );
    assert!(
        err.contains(&root.display().to_string()),
        "the error must name the host path: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_model_with_another_name_prints_no_fetch_command() {
    let root = temp_root("missing-custom");
    let runner =
        RecordingRunner::succeeding().with_config(extract_config_with(&root, "custom.gguf"));
    let err = check(&root, Stack::LocalRft, &runner).expect_err("a missing model must fail");
    assert!(err.contains("custom.gguf"), "{err}");
    assert!(
        !err.contains("hf download"),
        "the fetch command names a file the DevQuasar repository does not hold, so it must not \
         be suggested for another name: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_environment_list_shape_is_read_too() {
    let root = temp_root("env-list");
    write_model(&root, GGUF_FILE_NAME);
    // Some Compose versions write the resolved environment as a list of
    // NAME=value entries rather than as a map.
    let body = serde_json::json!({
        "services": {
            "extract": {
                "environment": [format!("LLAMA_ARG_MODEL=/models/{GGUF_FILE_NAME}"), "OTHER=1"],
                "volumes": [{ "type": "bind", "source": root.join("models").to_string_lossy(), "target": "/models" }],
            }
        }
    })
    .to_string();
    let runner = RecordingRunner::succeeding().with_config(body);
    assert_eq!(check(&root, Stack::LocalRft, &runner), Ok(()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_extract_service_without_a_model_argument_fails_closed() {
    let root = temp_root("no-arg");
    let body = serde_json::json!({ "services": { "extract": { "environment": {} } } }).to_string();
    let runner = RecordingRunner::succeeding().with_config(body);
    let err = check(&root, Stack::LocalRft, &runner)
        .expect_err("an unreadable extract service must not pass the check");
    assert!(err.contains(MODEL_ARG), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_extract_service_whose_model_is_not_mounted_fails_closed() {
    let root = temp_root("unmounted");
    let body = serde_json::json!({
        "services": {
            "extract": {
                "environment": { "LLAMA_ARG_MODEL": format!("/models/{GGUF_FILE_NAME}") },
                "volumes": [],
            }
        }
    })
    .to_string();
    let runner = RecordingRunner::succeeding().with_config(body);
    let err = check(&root, Stack::LocalRft, &runner)
        .expect_err("a model with no bind mount must not pass the check");
    assert!(err.contains("/models/"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn output_that_is_not_a_resolved_stack_fails_closed() {
    let root = temp_root("bad-json");
    let runner = RecordingRunner::succeeding().with_config("not a resolved stack".to_string());
    let err = check(&root, Stack::LocalRft, &runner).expect_err("unreadable output must fail");
    assert!(err.contains("JSON"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A runner whose window into the engine fails, for the two ways that happens.
struct BrokenEngine {
    code: i32,
    spawn_error: bool,
}
impl DockerRunner for BrokenEngine {
    fn capture(&self, _args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
        if self.spawn_error {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "docker missing",
            ));
        }
        Ok((self.code, String::new()))
    }
    fn run_inherited(&self, _args: &[&str], _dir: &Path) -> std::io::Result<i32> {
        unreachable!("the check never runs Docker with inherited stdio")
    }
}

#[test]
fn a_config_call_that_exits_non_zero_is_reported() {
    let root = temp_root("config-fails");
    let runner = BrokenEngine {
        code: 15,
        spawn_error: false,
    };
    let err = check(&root, Stack::LocalRft, &runner).expect_err("a refused config must fail");
    assert!(err.contains("15"), "{err}");
    assert!(
        err.contains(&root.display().to_string()),
        "the error must name where to look: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_config_call_that_cannot_start_is_reported() {
    let root = temp_root("config-spawn-fails");
    let runner = BrokenEngine {
        code: 0,
        spawn_error: true,
    };
    let err = check(&root, Stack::LocalRft, &runner).expect_err("an unreachable CLI must fail");
    assert!(err.contains("Compose"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
