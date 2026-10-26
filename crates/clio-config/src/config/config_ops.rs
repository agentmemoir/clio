// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `config_get` / `config_set` tool handlers on `Runtime`.
//!
//! ## Owns
//! - `config_get` pointer resolution and secret masking of the result
//! - `config_set` path/value validation, scope routing, and deployment persistence
//!
//! ## Does not own
//! - Overlay merge order and the effective config tree (see `super`)
//! - Path and value allowlist rules (see `super::validate`)
//! - Profile apply and `ranking_env_*` handlers (see `profile_ops`)
//!
//! ## Boundary
//!
//! Changes to what `config_get` / `config_set` returns should normally require changes here.
//! Changes to validation rules or merge order should not require changes here.
//!
//! Keep this module focused on the `config_get` / `config_set` Runtime handlers.

use super::merge::{path_to_pointer, value_as_string};
use super::persist::{ensure_overwritable, save_overlay_file};
use super::validate::{validate_set_path, validate_set_value};
use super::{ConfigGetData, ConfigScope, ConfigSetData, Runtime};
use crate::error::{AmError, ErrorCode, ToolResult};
use crate::log;
use crate::secret::{masked_at_path, masked_clone, path_is_secret};
use serde_json::Value;

impl Runtime {
    #[must_use]
    pub fn config_get(&self, path: Option<&str>) -> ToolResult<ConfigGetData> {
        ToolResult::from_result(self.config_get_inner(path))
    }

    fn config_get_inner(&self, path: Option<&str>) -> Result<ConfigGetData, AmError> {
        // Resolve the pointer on the unmasked tree: masking renames keys that
        // themselves look secret, so a pointer such as `credentials.api_key`
        // would otherwise stop at a name the masked view no longer has.
        let raw = self.effective_raw();
        let value = match path {
            None | Some("") => masked_clone(&raw),
            Some(p) => {
                let pointer = path_to_pointer(p);
                let found = raw.pointer(&pointer).ok_or_else(|| {
                    AmError::new(ErrorCode::UnknownPath, format!("unknown config path `{p}`"))
                })?;
                masked_at_path(p, found)
            }
        };
        Ok(ConfigGetData {
            path: path.unwrap_or("").to_owned(),
            value,
            active_profile: self.active_profile.clone(),
        })
    }

    pub fn config_set(
        &mut self,
        path: &str,
        value: Value,
        scope: ConfigScope,
    ) -> ToolResult<ConfigSetData> {
        ToolResult::from_result(self.config_set_inner(path, value, scope))
    }

    fn config_set_inner(
        &mut self,
        path: &str,
        value: Value,
        scope: ConfigScope,
    ) -> Result<ConfigSetData, AmError> {
        validate_set_path(path)?;
        validate_set_value(path, &value)?;
        {
            let overlay = match scope {
                ConfigScope::Session => &mut self.session_overlay,
                ConfigScope::Profile => {
                    if self.active_profile.is_none() {
                        return Err(AmError::new(
                            ErrorCode::InvalidArgument,
                            "config_set scope=profile requires an active profile",
                        ));
                    }
                    &mut self.profile_overlay
                }
                ConfigScope::Deployment => &mut self.deployment_overlay,
            };
            overlay.insert(path.to_owned(), value.clone());
        }
        if path == "log.level" {
            self.sync_log_level();
        }
        if path == "bank.policy" {
            // Validated above; deserialize cannot fail for allowlisted shapes.
            self.bank_policy =
                serde_json::from_value(value.clone()).expect("bank.policy after validate");
        }
        if scope == ConfigScope::Deployment
            && let Some(path_buf) = self.deployment_path.clone()
        {
            ensure_overwritable(&path_buf)?;
            save_overlay_file(&path_buf, &self.deployment_overlay)?;
        }
        log::info("config_set", &[("path", path), ("scope", scope.as_str())]);
        Ok(ConfigSetData {
            path: path.to_owned(),
            scope: scope.as_str().to_owned(),
            value: if path_is_secret(path) {
                Value::String(crate::secret::mask_secret(&value_as_string(&value)))
            } else {
                value
            },
        })
    }
}

#[cfg(test)]
#[path = "config_ops_tests.rs"]
mod config_ops_tests;
