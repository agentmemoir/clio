// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the generated Compose `.env` and the selection marker file:
//! content rules (no online-service keys, every name carrying the prefix
//! `compose.yml` substitutes), owner-only permissions, the never-overwrite rule
//! for an existing user `.env`, and the marker roundtrip.

use super::*;

fn temp_root(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "am-compose-env-{name}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp root");
    dir
}

#[test]
fn env_contents_hold_compose_variables_only() {
    let contents = env_contents("BAAI/bge-small-en-v1.5");
    for line in [
        "CLIO_POSTGRES_USER=clio",
        "CLIO_POSTGRES_DB=clio",
        "CLIO_POSTGRES_PORT=34310",
        "CLIO_EMBED_MODEL=BAAI/bge-small-en-v1.5",
        &format!("CLIO_EXTRACT_GGUF={}", compose_assets::GGUF_FILE_NAME),
    ] {
        assert!(contents.contains(line), "missing `{line}` in generated env");
    }
    // No password: it is passed through the process environment for each `up`,
    // so the file can never disagree with the sealed copy in the config.
    assert!(
        !contents.contains("POSTGRES_PASSWORD"),
        "generated env must not carry the database password"
    );
    // No online-service API keys: no HF_TOKEN line, no key-shaped assignments.
    assert!(
        !contents.contains("HF_TOKEN"),
        "generated env must not carry HF_TOKEN"
    );
    assert!(
        !contents.contains("API_KEY"),
        "generated env must not carry API keys"
    );
    assert!(
        !contents.contains("DATABASE_URL"),
        "generated env is not the CLI config"
    );
}

#[test]
fn every_written_name_carries_the_prefix_compose_substitutes() {
    // The rule, asserted as a shape over the whole file rather than one name at
    // a time: compose reads the host side, so an unprefixed line would be
    // ignored silently and the container would come up on its own default. This
    // is a shape assertion — it reads the file's own text, nothing from the host.
    let contents = env_contents("BAAI/bge-small-en-v1.5");
    let mut written = Vec::new();
    for line in contents.lines() {
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        assert!(
            name.starts_with("CLIO_"),
            "`{name}` has no CLIO_ prefix, so compose.yml will not read it"
        );
        written.push(name.to_owned());
    }
    assert_eq!(
        written.len(),
        15,
        "one entry per name the generator writes, so a dropped or doubled name \
         is a failure here: {written:?}"
    );
}

#[test]
fn env_contents_use_the_effective_embed_model() {
    let contents = env_contents("jinaai/jina-embeddings-v4");
    assert!(
        contents.contains("CLIO_EMBED_MODEL=jinaai/jina-embeddings-v4"),
        "model must come from the config value"
    );
}

#[test]
fn write_env_is_owner_only_on_unix() {
    let root = temp_root("perms");
    write_env(&root, "CLIO_POSTGRES_PORT=34310\n").expect("write env");
    let path = root.join(".env");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "generated .env must be owner-only (0600)"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn existing_user_env_is_never_overwritten() {
    let root = temp_root("keep");
    std::fs::write(root.join(".env"), "CLIO_POSTGRES_PORT=39999\n").expect("user env");
    write_env(&root, "CLIO_POSTGRES_PORT=34310\n").expect("write must still succeed");
    let contents = std::fs::read_to_string(root.join(".env")).unwrap();
    assert_eq!(
        contents, "CLIO_POSTGRES_PORT=39999\n",
        "user .env must be preserved"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn marker_records_and_reads_the_selection() {
    let root = temp_root("marker");
    assert_eq!(read_marker(&root), None, "no marker before up");
    write_marker(&root, "local-rft").expect("write marker");
    assert_eq!(read_marker(&root), Some("local-rft".to_string()));
    write_marker(&root, "postgres").expect("overwrite marker");
    assert_eq!(read_marker(&root), Some("postgres".to_string()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn env_write_failure_is_reported() {
    let root = temp_root("env-fail");
    // A missing parent directory makes the write fail (the CLI would have
    // created the root earlier; this targets only the write error arm).
    let err = write_env(&root.join("missing-dir"), "CLIO_POSTGRES_PORT=34310\n")
        .expect_err("uncreatable .env path must fail");
    assert!(err.contains("cannot write .env"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn marker_write_failure_is_reported() {
    let root = temp_root("marker-fail");
    std::fs::create_dir(root.join(MARKER_FILE)).expect("block the marker path");
    let err = write_marker(&root, "postgres").expect_err("directory marker must fail");
    assert!(err.contains(MARKER_FILE), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn marker_write_refuses_a_symlink_and_leaves_the_target_unchanged() {
    let root = temp_root("marker-symlink-write");
    let victim = root.join("victim");
    std::fs::write(&victim, b"content this module must not rewrite\n").expect("plant the victim");
    std::os::unix::fs::symlink(&victim, root.join(MARKER_FILE)).expect("plant the marker symlink");

    let err = write_marker(&root, "postgres").expect_err("a symlinked marker must be refused");
    assert!(err.contains(MARKER_FILE), "{err}");
    assert_eq!(
        std::fs::read(&victim).expect("read the victim"),
        b"content this module must not rewrite\n",
        "the write must not reach the file the symlink named"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn marker_read_refuses_a_symlink_instead_of_trusting_the_forged_selection() {
    let root = temp_root("marker-symlink-read");
    let elsewhere = root.join("elsewhere");
    std::fs::write(&elsewhere, b"postgres\n").expect("plant the forged selection");
    std::os::unix::fs::symlink(&elsewhere, root.join(MARKER_FILE))
        .expect("plant the marker symlink");

    let err = read_marker_entry(&root).expect_err("a symlinked marker must be refused");
    assert!(
        err.contains(MARKER_FILE) && err.contains("symlink"),
        "the refusal must name the marker and what it found: {err}"
    );
    assert_eq!(
        read_marker(&root),
        None,
        "the forged selection must not be recorded as this root's selection"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn marker_read_refuses_a_named_pipe_by_the_descriptor_type() {
    let root = temp_root("marker-pipe");
    let path = root.join(MARKER_FILE);
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        path.as_path(),
        rustix::fs::Mode::from(0o600),
    )
    .expect("plant a named pipe at the marker name");

    let err = read_marker_entry(&root).expect_err("a named pipe is not a recordable marker");
    assert!(
        err.contains(MARKER_FILE) && err.contains("not a regular file"),
        "the refusal must name the marker and what it found: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn open_marker_refuses_a_named_pipe_itself() {
    let root = temp_root("marker-pipe-gate");
    let path = root.join(MARKER_FILE);
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        path.as_path(),
        rustix::fs::Mode::from(0o600),
    )
    .expect("plant a named pipe at the marker name");

    // Called directly, so the absence pre-check never runs: only the descriptor
    // check can refuse this. `NONBLOCK` is what keeps the open from waiting for
    // a writer, so the refusal is an error rather than a hang.
    let err = open_marker(&path, rustix::fs::OFlags::RDONLY, "read")
        .expect_err("the gate itself must refuse a named pipe");
    assert!(
        err.contains("not a regular file"),
        "pins the descriptor check: a fifo slipped past the name is refused by \
         the opened descriptor's type, not by the pre-check: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn open_marker_opens_without_following_a_symlink() {
    let root = temp_root("marker-symlink-gate");
    let target = root.join("target");
    std::fs::write(&target, b"pg\n").expect("plant the link target");
    let path = root.join(MARKER_FILE);
    std::os::unix::fs::symlink(&target, &path).expect("plant the marker symlink");
    assert!(
        std::fs::File::open(&path).is_ok(),
        "the link resolves to a readable regular file, so only no-follow refuses it"
    );

    let err = open_marker(&path, rustix::fs::OFlags::RDONLY, "read")
        .expect_err("the gate itself must refuse a symlink");
    assert!(
        err.contains(MARKER_FILE) && !err.contains("not a regular file"),
        "pins the NOFOLLOW flag: the open refuses the link name, it does not \
         follow it and then inspect the target: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn open_marker_refuses_a_hard_linked_marker() {
    let root = temp_root("marker-hardlink-gate");
    let shared = root.join("shared");
    std::fs::write(&shared, b"postgres\n").expect("plant the shared file");
    let path = root.join(MARKER_FILE);
    std::fs::hard_link(&shared, &path).expect("plant a hard link at the marker name");

    let err = open_marker(&path, rustix::fs::OFlags::RDONLY, "read")
        .expect_err("a marker another name shares must be refused");
    assert!(
        err.contains("hard links"),
        "pins the link count: a regular file with another name sharing it is \
         refused even though its descriptor type is regular: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn marker_read_has_no_selection_without_a_marker() {
    let root = temp_root("marker-absent");
    assert_eq!(
        read_marker_entry(&root),
        Ok(None),
        "a missing marker is no recorded selection, not a refusal"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn marker_read_has_no_selection_for_an_empty_marker() {
    let root = temp_root("marker-empty");
    std::fs::write(root.join(MARKER_FILE), b"").expect("plant an empty marker");
    assert_eq!(
        read_marker_entry(&root),
        Ok(None),
        "an empty regular marker records nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}
