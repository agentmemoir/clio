// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the generated Compose `.env` and the selection marker file:
//! content rules (no online-service keys, every name carrying the prefix
//! `compose.yml` substitutes), owner-only permissions, the never-overwrite rule
//! for an existing user `.env`, reading the GGUF name back out of one, and the
//! marker roundtrip.

use super::*;

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("am-compose-env-{name}-{}", std::process::id()));
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
fn extract_gguf_name_defaults_and_reads_an_existing_env() {
    let root = temp_root("gguf-name");
    assert_eq!(
        extract_gguf_name(&root),
        compose_assets::GGUF_FILE_NAME,
        "no .env means the documented default"
    );
    std::fs::write(
        root.join(".env"),
        "# generated\nCLIO_POSTGRES_PORT=34310\nCLIO_EXTRACT_GGUF=custom.gguf\n",
    )
    .expect("write .env");
    assert_eq!(extract_gguf_name(&root), "custom.gguf");
    // Quotes and surrounding space are accepted the way compose's own `.env`
    // parser accepts them, so a quoted name is still found.
    std::fs::write(
        root.join(".env"),
        "CLIO_EXTRACT_GGUF=\"spaced name.gguf\"\n",
    )
    .expect("write");
    assert_eq!(extract_gguf_name(&root), "spaced name.gguf");
    // An empty value falls back rather than yielding an empty file name.
    std::fs::write(root.join(".env"), "CLIO_EXTRACT_GGUF=\n").expect("write");
    assert_eq!(
        extract_gguf_name(&root),
        compose_assets::GGUF_FILE_NAME,
        "an empty value must not become the name"
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
