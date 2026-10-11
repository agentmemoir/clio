// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asking Compose which model file the `extract`
//! service will open, and for refusing to start the stack when that file is not
//! on the host.
//!
//! ## Owns
//! - The `docker compose config` query and the reading of its resolved stack
//! - The mapping from the resolved model argument and bind mount to the host
//!   path the container will read
//! - The missing-model error, which names the file and, for the model the
//!   project ships a fetch command for, the command that fetches it
//!
//! ## Does not own
//! - Docker target detection, the probe, and argument shapes (see `compose_docker`)
//! - The `up` pipeline, its ordering, and exit-code mapping (see `compose_cli`)
//! - Embedded-asset materialization and the init-script check (see `compose_assets`)
//!
//! ## Boundary
//!
//! Changes to which Compose output is read, or how the host path is derived
//! from it, should normally require changes here. Changes to when the check
//! runs, or to which Docker arguments are used, should not.
//!
//! Keep this module focused on the extract model's host path.
//!
//! ## Why the engine is asked
//!
//! The model name is a Compose variable, so three things can set it: the process
//! environment, the project `.env`, and the default inside the compose file.
//! Compose resolves them in that order, and the environment wins over `.env`. A
//! second resolver here would have to repeat that order and the `.env` grammar
//! with it, and any disagreement would either pass a stack whose model is absent
//! or refuse one whose model is present. Asking Compose removes the second
//! resolver rather than trying to keep two of them in step.

use crate::compose_assets::GGUF_FILE_NAME;
use crate::compose_docker::{ComposeTarget, DockerRunner, Stack, compose_args};
use std::path::{Path, PathBuf};

/// The environment value `compose.yml` uses to name the model the extract
/// server loads, read from Compose's own resolved stack.
const MODEL_ARG: &str = "LLAMA_ARG_MODEL";

/// Verify that the model file the `extract` service will open is on the host.
///
/// Whether `extract` is in the stack is Compose's answer too: it resolves the
/// selected profile, so a stack without `extract` holds no model to check and
/// passes. Nothing here repeats the service list or the profile rules.
///
/// # Errors
/// Compose could not be asked or refused to resolve the stack; the resolved
/// stack no longer has the shape the check reads; or the model file is absent.
pub(crate) fn verify_extract_model(
    root: &Path,
    target: ComposeTarget,
    stack: Stack,
    runner: &dyn DockerRunner,
) -> Result<(), String> {
    let args = compose_args(target, stack, "config", &["--format", "json"]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, output) = runner
        .capture(&refs, root)
        .map_err(|e| format!("cannot ask Compose for the resolved stack: {e}"))?;
    if code != 0 {
        return Err(format!(
            "`docker compose config` exited with status {code}, so the extract model was not \
             checked; run it in {} to see why",
            root.display()
        ));
    }
    let Some(model) = host_model_path(&output)? else {
        return Ok(());
    };
    if model.is_file() {
        return Ok(());
    }
    Err(missing_model_error(&model))
}

/// The host path of the model the resolved stack will mount, or `None` when the
/// resolved stack holds no `extract` service.
///
/// # Errors
/// Output that is not a resolved stack, or an `extract` service whose model
/// argument or bind mount cannot be found. Both mean the files no longer have
/// the shape this check reads, and a green result on an unread shape would be
/// the silent pass the check exists to prevent.
fn host_model_path(config_json: &str) -> Result<Option<PathBuf>, String> {
    let config: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|e| format!("cannot read the resolved stack as JSON: {e}"))?;
    let Some(service) = config.get("services").and_then(|all| all.get("extract")) else {
        return Ok(None);
    };
    let container = environment_value(service, MODEL_ARG)
        .ok_or_else(|| format!("the resolved extract service has no {MODEL_ARG} value"))?;
    let host = bind_source(service, &container)
        .ok_or_else(|| format!("the resolved extract service binds nothing at {container}"))?;
    Ok(Some(host))
}

/// One environment value of a resolved service. Compose writes `environment`
/// as a map in some versions and as a list of `NAME=value` entries in others,
/// and both shapes carry the same value.
fn environment_value(service: &serde_json::Value, name: &str) -> Option<String> {
    match service.get("environment")? {
        serde_json::Value::Object(map) => map.get(name)?.as_str().map(str::to_owned),
        serde_json::Value::Array(entries) => entries
            .iter()
            .filter_map(serde_json::Value::as_str)
            .find_map(|entry| {
                entry
                    .strip_prefix(name)?
                    .strip_prefix('=')
                    .map(str::to_owned)
            }),
        _ => None,
    }
}

/// The host path behind a path inside the container: the bind mount whose
/// target is a parent of `container`, with the remainder re-joined. That is the
/// same join the runtime performs when it mounts the host directory.
fn bind_source(service: &serde_json::Value, container: &str) -> Option<PathBuf> {
    for volume in service.get("volumes")?.as_array()? {
        if volume.get("type").and_then(serde_json::Value::as_str) != Some("bind") {
            continue;
        }
        let (Some(source), Some(target)) = (
            volume.get("source").and_then(serde_json::Value::as_str),
            volume.get("target").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        if let Some(rest) = container.strip_prefix(target) {
            return Some(Path::new(source).join(rest.strip_prefix('/').unwrap_or(rest)));
        }
    }
    None
}

/// The failure the user sees. The fetch command is printed only for the model
/// the project ships a source for: for any other name it would name a file that
/// repository does not hold, which is worse than no suggestion at all.
fn missing_model_error(model: &Path) -> String {
    let name = model
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name == GGUF_FILE_NAME {
        return format!(
            "missing extract model {}; fetch it with `hf download \
             DevQuasar/numind.NuExtract-tiny-v1.5-GGUF {name} --local-dir ./models`",
            model.display()
        );
    }
    format!(
        "missing extract model {}; the stack names a model other than the shipped \
         {GGUF_FILE_NAME}, so place that file in the directory the compose file mounts",
        model.display()
    )
}

#[cfg(test)]
#[path = "compose_model_tests.rs"]
mod tests;
