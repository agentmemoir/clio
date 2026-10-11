// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Failure-mode and dispatcher tests for `clio compose`: precondition
//! failures that must leave no Docker calls or partial files behind, exit-code
//! propagation, and selection-resolution errors. Companion to
//! `compose_cli_tests.rs` (which owns the happy-path coverage).

use super::tests::{flags, last_call, temp_root, write_model};
use super::*;
use crate::compose_docker::tests::RecordingRunner;

#[test]
fn down_without_a_materialized_root_fails() {
    let root = temp_root("down-empty");
    let runner = RecordingRunner::succeeding();
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(code, ExitCode::from(1));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn down_docker_failure_propagates() {
    let root = temp_root("down-fail");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let failing = RecordingRunner::failing(1);
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap())]),
        &failing,
        ComposeTarget::MacArm64,
    );
    assert_eq!(code, ExitCode::from(1));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn down_services_flag_overrides_the_marker() {
    let root = temp_root("down-override");
    write_model(&root);
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "local-rft")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    let call = last_call(&runner);
    assert!(
        !call.contains(&"--profile".to_string()),
        "an explicit --services must override the marker: {call:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extract_without_gguf_fails_before_docker() {
    let root = temp_root("gguf-missing");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[
            ("dir", root.to_str().unwrap()),
            ("services", "postgres,extract"),
        ]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "a missing extract model must fail the start, not warn and continue"
    );
    assert!(
        compose_env::read_marker(&root).is_none(),
        "a failed start records no selection for `down` to reuse"
    );
    assert!(
        runner
            .recorded()
            .iter()
            .all(|call| !call.iter().any(|arg| arg == "up")),
        "no Docker start may happen while the model is missing: {:?}",
        runner.recorded()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prepare_root_reports_the_missing_extract_model() {
    let root = temp_root("gguf-message");
    // The process-level tests can only read the exit code, so the message itself
    // is asserted here, where `prepare_root` returns it.
    let err = prepare_root(&root, ComposeTarget::MacArm64, Stack::LocalRft)
        .expect_err("a missing model must be reported, not warned about");
    assert!(err.contains(compose_assets::GGUF_FILE_NAME), "{err}");
    assert!(
        err.contains("hf download"),
        "the error must name the command that fetches the model: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn resolve_root_refuses_non_directory_paths() {
    let parent = temp_root("refuse");
    let file = parent.join("not-a-dir");
    std::fs::write(&file, "x").expect("create file");
    let err = resolve_root(file.to_str()).expect_err("a plain file must be refused");
    assert!(err.contains("not a directory"), "{err}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn resolve_root_creates_a_missing_directory() {
    let parent = temp_root("create-me");
    let target = parent.join("nested/inner");
    let resolved = resolve_root(target.to_str()).expect("missing dir is created");
    assert_eq!(resolved, target);
    assert!(target.is_dir());
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn compose_dispatcher_prints_help_and_rejects_unknown_subcommands() {
    assert_eq!(compose(["help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(compose(["--help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(compose(["bogus".to_string()]), ExitCode::from(2));
    assert_eq!(
        compose([] as [String; 0]),
        ExitCode::from(2),
        "a missing subcommand must fail with the unknown-subcommand code"
    );
}

#[test]
fn effective_embed_model_is_never_empty() {
    let model = effective_embed_model();
    assert!(!model.is_empty(), "{model:?}");
}

#[test]
fn read_answer_reads_one_prompt_line() {
    let mut input = std::io::Cursor::new("3\n".to_string());
    assert_eq!(read_answer(&mut input), Some("3\n".to_string()));
    let mut empty = std::io::Cursor::new(String::new());
    assert_eq!(read_answer(&mut empty), None, "EOF means no answer");
}

#[test]
fn prompt_tty_false_is_non_interactive_and_true_reads_input() {
    let mut silent = std::io::Cursor::new(String::new());
    assert_eq!(prompt_tty(false, &mut silent), None, "non-TTY never reads");
    let mut tty = std::io::Cursor::new("2\n".to_string());
    assert_eq!(prompt_tty(true, &mut tty), Some("2\n".to_string()));
}

/// `read_prompt_line` reads the process stdin, so its "no answer" contract can
/// only be asserted when stdin is NOT a terminal: on a TTY the read would block
/// the whole test run, and a TTY answer is not `None` anyway. The
/// non-interactive gate/CI path still exercises it here.
#[test]
fn read_prompt_line_is_none_when_stdin_is_not_a_tty() {
    if std::io::stdin().is_terminal() {
        return;
    }
    assert_eq!(read_prompt_line(), None);
}

#[test]
fn resolve_root_uses_the_injected_cwd_when_no_dir_is_given() {
    let dir = temp_root("injected-cwd");
    let resolved = resolve_root_with(None, || Ok(dir.clone())).expect("injected cwd is used");
    assert_eq!(resolved, dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_root_reports_a_failing_cwd_lookup() {
    let err = resolve_root_with(None, || {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cwd unavailable",
        ))
    })
    .expect_err("a failing cwd lookup must be reported, not panic");
    assert!(err.contains("cannot read cwd"), "{err}");
}

#[test]
fn resolve_root_reports_a_root_that_cannot_be_created() {
    let parent = temp_root("cannot-create");
    let file = parent.join("plain-file");
    std::fs::write(&file, "x").expect("create file");
    let nested = file.join("child");
    let err = resolve_root(nested.to_str()).expect_err("must not create a directory under a file");
    assert!(err.contains("cannot create"), "{err}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn prompt_free_form_answer_is_parsed_as_service_list() {
    let root = temp_root("prompt-freform");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
        &|| Some("postgres,embed".to_string()),
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(last_call(&runner).contains(&"local-retrieval".to_string()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn up_and_down_fail_when_the_dir_is_not_a_directory() {
    let parent = temp_root("dir-file");
    let file = parent.join("target");
    std::fs::write(&file, "x").expect("create file");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", file.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "up must refuse a non-directory --dir"
    );
    let code = compose_down(
        &flags(&[("dir", file.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "down must refuse a non-directory --dir"
    );
    assert!(
        runner
            .recorded()
            .iter()
            .all(|call| !call.iter().any(|arg| arg == "up" || arg == "down")),
        "no inherited docker invocation may happen after a precondition failure"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn down_fails_on_an_unparseable_recorded_selection() {
    let root = temp_root("down-bad-marker");
    std::fs::write(root.join("compose.yml"), "x").expect("materialize-like marker");
    std::fs::write(root.join(compose_env::MARKER_FILE), "not-a-service\n").expect("bad marker");
    let runner = RecordingRunner::succeeding();
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(code, ExitCode::from(1));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn down_fails_without_a_recorded_selection_or_services() {
    let root = temp_root("down-no-marker");
    std::fs::write(root.join("compose.yml"), "x").expect("materialize-like file");
    let runner = RecordingRunner::succeeding();
    let code = compose_down(
        &flags(&[("dir", root.to_str().unwrap())]),
        &runner,
        ComposeTarget::MacArm64,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "no marker and no --services must fail closed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn docker_spawn_failure_is_reported() {
    struct ErrRunner;
    impl DockerRunner for ErrRunner {
        fn capture(&self, _args: &[&str], _dir: &Path) -> std::io::Result<(i32, String)> {
            Ok((0, String::new()))
        }
        fn run_inherited(&self, _args: &[&str], _dir: &Path) -> std::io::Result<i32> {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "spawn failed",
            ))
        }
        fn run_inherited_env(
            &self,
            _args: &[&str],
            _dir: &Path,
            _env: &[(String, String)],
        ) -> std::io::Result<i32> {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "spawn failed",
            ))
        }
    }
    let root = temp_root("spawn-fail");
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &ErrRunner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(code, ExitCode::from(1));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn up_fails_when_the_selection_marker_cannot_be_written() {
    let root = temp_root("up-marker-fail");
    std::fs::create_dir(root.join(compose_env::MARKER_FILE)).expect("block the marker path");
    let runner = RecordingRunner::succeeding();
    let code = compose_up(
        &flags(&[("dir", root.to_str().unwrap()), ("services", "postgres")]),
        &runner,
        ComposeTarget::MacArm64,
        &|| None,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "a blocked marker must fail before Docker runs"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_undeclared_compose_flag_is_a_usage_error() {
    assert_eq!(
        compose(["up".to_string(), "--bogus".to_string()]),
        ExitCode::from(2),
        "an unknown flag fails closed before Docker is touched"
    );
}
