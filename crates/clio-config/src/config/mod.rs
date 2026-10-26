// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-process `Runtime` and `config_*` / `ranking_env_*` tools.
//!
//! ## Owns
//! - Effective configuration state and tool handlers
//! - Profile apply orchestration
//!
//! ## Does not own
//! - Process-env overlay wiring and bearer credential lookup (see `env`)
//! - Overlay flatten/set primitives (see `merge`)
//! - Path/value validation rules (see `validate`)
//! - Tool-name JSON dispatch (see `dispatch`)
//! - Ranking weight math (see `ranking`)
//!
//! ## Boundary
//!
//! Changes to config/ranking tool semantics on `Runtime` should normally require changes here.
//! Changes to JSON overlay helpers or profile catalog text should not require changes here.
//!
//! Keep this module focused on Runtime config/ranking tool orchestration.

mod cli_state;
mod config_ops;
mod dispatch;
mod env;
mod merge;
mod persist;
mod profile_ops;
mod retention_ops;
mod types;
mod validate;
mod validate_set;

#[cfg(test)]
mod tests;

pub use env::bearer_for_effective;
pub use persist::{
    DEPLOYMENT_CONFIG_ENV, DEPLOYMENT_CONFIG_FILE_NAME, backup_config_file, default_config_dir,
    resolve_deployment_path, restrict_dir, write_private_atomic,
};
pub use types::{
    ConfigGetData, ConfigProfileApplyData, ConfigProfilesData, ConfigSetData, ProfileSummary,
    RankingEnvSetData, RetentionMissionSummary, RetentionProfileGetData, RetentionProfileSetData,
};
pub use validate::{CONFIG_SCHEMA_VERSION, ConfigScope};

use crate::context::{Actor, BankDefaultPolicy, OpContext, resolve_bank};
use crate::error::{AmError, ErrorCode};
use crate::history_knobs::HistoryKnobs;
use crate::log::{self, LogLevel};
use crate::ranking::RankingEnv;
use crate::retention_store::{RetentionStore, resolve_retention_path};
use crate::secret::masked_clone;
use clio_types::HistoryPolicy;
use merge::{Overlay, apply_overlay, flatten_object, set_path, system_defaults};
use persist::{load_overlay_file, resolve_deployment_path_with, save_overlay_file};
use serde_json::Value;
use std::path::PathBuf;

/// Process runtime: effective config + config/ranking tool handlers.
#[derive(Debug)]
pub struct Runtime {
    defaults: Value,
    file_overlay: Overlay,
    env_overlay: Overlay,
    active_profile: Option<String>,
    profile_bundle: Overlay,
    profile_overlay: Overlay,
    deployment_overlay: Overlay,
    session_overlay: Overlay,
    ranking: RankingEnv,
    bank_policy: BankDefaultPolicy,
    repo_root: Option<String>,
    default_actor: Actor,
    deployment_path: Option<PathBuf>,
    cli_state_path: Option<PathBuf>,
    retention: RetentionStore,
    retention_path: Option<PathBuf>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    #[must_use]
    pub fn new() -> Self {
        Self::boot_with(
            &|key| std::env::var(key).ok(),
            resolve_deployment_path(None),
            resolve_retention_path(None),
            cli_state::resolve_cli_state_path(None),
        )
    }

    /// Boot with an injected environment lookup (tests / embedding).
    ///
    /// The same lookup drives the default database target, the process-env
    /// config overlay, and the default deployment config path, so tests never
    /// depend on the host env.
    #[must_use]
    pub fn with_env<F>(get: &F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::boot_with(
            get,
            resolve_deployment_path_with(None, get),
            resolve_retention_path(None),
            cli_state::resolve_cli_state_path_with(None, get),
        )
    }

    /// Test/embed helper: boot with an explicit deployment overlay path.
    #[must_use]
    pub fn with_deployment_file(path: impl Into<PathBuf>) -> Self {
        Self::with_env_and_deployment_file(&|key| std::env::var(key).ok(), path)
    }

    /// Boot with an injected environment lookup *and* an explicit deployment path.
    ///
    /// Both at once, because a caller that names a config path usually also needs
    /// to say what the environment says. Without this pair the only way to have
    /// both is to read the real environment, so a test that wants a chosen path
    /// silently inherits the machine's `CLIO_DATABASE_URL` and asserts a fact
    /// about the machine instead of about the code.
    #[must_use]
    pub fn with_env_and_deployment_file(
        get: &dyn Fn(&str) -> Option<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        // The lookup is borrowed, so it is re-borrowed through a closure to reach
        // `boot_with`, which takes it by a concrete type.
        Self::boot_with(&|key| get(key), Some(path.into()), None, None)
    }

    fn boot_with<F>(
        get: &F,
        deployment_path: Option<PathBuf>,
        retention_path: Option<PathBuf>,
        cli_state_path: Option<PathBuf>,
    ) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut rt = Self {
            defaults: system_defaults(get),
            file_overlay: Overlay::new(),
            env_overlay: Overlay::new(),
            active_profile: None,
            profile_bundle: Overlay::new(),
            profile_overlay: Overlay::new(),
            deployment_overlay: Overlay::new(),
            session_overlay: Overlay::new(),
            ranking: RankingEnv::system_defaults(),
            bank_policy: BankDefaultPolicy::Required,
            repo_root: None,
            default_actor: Actor::Agent,
            deployment_path,
            cli_state_path,
            retention: RetentionStore::default(),
            retention_path,
        };
        rt.finish_boot(get);
        rt
    }

    fn finish_boot<F>(&mut self, get: &F)
    where
        F: Fn(&str) -> Option<String>,
    {
        self.apply_process_env(&mut |key| get(key));
        if let Some(path) = self.deployment_path.clone()
            && let Ok(overlay) = load_overlay_file(&path)
        {
            self.deployment_overlay = overlay;
        }
        if let Some(path) = self.retention_path.clone() {
            self.retention = RetentionStore::load_lossy(&path);
        }
        self.load_cli_state();
        self.sync_log_level();
    }

    /// Override the durable deployment config path (tests / embedding).
    pub fn set_deployment_path(&mut self, path: impl Into<PathBuf>) {
        self.deployment_path = Some(path.into());
    }

    /// Drop the durable deployment path so deployment-scoped sets stay in-memory only.
    pub fn clear_deployment_path(&mut self) {
        self.deployment_path = None;
    }

    /// Reload deployment overlay from [`Self::set_deployment_path`] / env.
    ///
    /// # Errors
    /// Corrupt deployment file.
    pub fn reload_deployment_overlay(&mut self) -> Result<(), AmError> {
        if let Some(path) = self.deployment_path.clone() {
            self.deployment_overlay = load_overlay_file(&path)?;
            self.sync_log_level();
        }
        Ok(())
    }

    /// Persist the current in-memory deployment overlay to its resolved path
    /// (no-op when no path is set), writing exactly once.
    ///
    /// # Errors
    /// I/O failures creating parents or writing the file.
    pub fn flush_deployment_overlay(&mut self) -> Result<(), AmError> {
        if let Some(path) = self.deployment_path.clone() {
            save_overlay_file(&path, &self.deployment_overlay)?;
        }
        Ok(())
    }

    /// Raw value the durable deployment overlay sets for `path`, or `None` when
    /// the overlay does not set it (callers mask before display).
    #[must_use]
    pub fn deployment_field(&self, path: &str) -> Option<&Value> {
        self.deployment_overlay.get(path)
    }

    /// Every flattened dot-path the durable deployment overlay sets, so a caller
    /// can discover (and mask) secrets it does not already know by name.
    pub fn deployment_keys(&self) -> impl Iterator<Item = &str> {
        self.deployment_overlay.keys().map(String::as_str)
    }

    /// Drop the durable deployment overlay fields named by dot-path, returning
    /// how many were present and removed.
    pub fn remove_deployment_fields(&mut self, paths: &[&str]) -> usize {
        paths
            .iter()
            .filter(|path| self.deployment_overlay.remove(**path).is_some())
            .count()
    }

    /// Set optional repo root used by per-repository bank policy.
    pub fn set_repo_root(&mut self, root: impl Into<String>) {
        self.repo_root = Some(root.into());
    }

    /// Merge a JSON object file layer.
    ///
    /// # Errors
    /// Corrupt / non-object roots.
    pub fn load_file_json(&mut self, text: &str) -> Result<(), AmError> {
        let value: Value = serde_json::from_str(text).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("config file parse error: {e}"),
            )
        })?;
        let Value::Object(map) = value else {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "config file root must be a JSON object",
            ));
        };
        self.file_overlay.clear();
        flatten_object("", &map, &mut self.file_overlay);
        self.sync_log_level();
        Ok(())
    }

    /// Effective merged configuration as JSON (callers must mask for display).
    ///
    /// The layers merge in this order: file, environment, profile bundle,
    /// profile overlay, deployment overlay, session overlay. A restored or
    /// applied profile therefore outranks the environment for the knobs its
    /// bundle carries (`log.level`, `backend`), and the deployment file outranks
    /// both, so `CLIO_LOG_LEVEL` and `CLIO_BACKEND` stop applying once a profile
    /// is persisted.
    ///
    /// # Panics
    /// Panics if ranking or bank policy fails to serialize (both are `Serialize` by construction).
    #[must_use]
    pub fn effective_raw(&self) -> Value {
        let mut root = self.defaults.clone();
        apply_overlay(&mut root, &self.file_overlay);
        apply_overlay(&mut root, &self.env_overlay);
        apply_overlay(&mut root, &self.profile_bundle);
        apply_overlay(&mut root, &self.profile_overlay);
        apply_overlay(&mut root, &self.deployment_overlay);
        apply_overlay(&mut root, &self.session_overlay);
        set_path(
            &mut root,
            "ranking",
            serde_json::to_value(&self.ranking).expect("RankingEnv serializes"),
        );
        if let Some(name) = &self.active_profile {
            set_path(&mut root, "profile.active", Value::String(name.clone()));
        }
        set_path(
            &mut root,
            "bank.policy",
            serde_json::to_value(&self.bank_policy).expect("BankDefaultPolicy serializes"),
        );
        root
    }

    #[must_use]
    pub fn effective_masked(&self) -> Value {
        masked_clone(&self.effective_raw())
    }

    /// Resolve the history threshold policy from the effective configuration.
    ///
    /// # Errors
    /// An invalid overlay combination (for example `default_limit > max_limit`).
    pub fn history_policy(&self) -> Result<HistoryPolicy, AmError> {
        HistoryKnobs::from_config(&self.effective_raw()).to_policy()
    }

    pub(super) fn sync_log_level(&mut self) {
        if let Some(level) = self
            .effective_raw()
            .pointer("/log/level")
            .and_then(Value::as_str)
            .and_then(LogLevel::parse)
        {
            log::set_level(level);
        }
    }

    /// Resolve operation context.
    ///
    /// Bank precedence: explicit → `bank.fixed` / `CLIO_BANK` → active profile policy.
    ///
    /// # Errors
    /// Missing bank under a required policy (or failed per-repo derivation).
    pub fn resolve_context(
        &self,
        bank: Option<&str>,
        actor: Option<&str>,
    ) -> Result<OpContext, AmError> {
        let fixed = self
            .effective_raw()
            .pointer("/bank/fixed")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.is_empty());
        let bank = resolve_bank(
            &self.bank_policy,
            bank,
            fixed.as_deref(),
            self.repo_root.as_deref(),
        )?;
        let actor = match actor {
            Some(a) => Actor::parse(a)?,
            None => self.default_actor,
        };
        Ok(OpContext::new(bank, actor))
    }

}

