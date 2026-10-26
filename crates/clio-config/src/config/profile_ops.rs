// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for profile list/apply and `ranking_env` tool handlers on `Runtime`.
//!
//! ## Owns
//! - `config_profiles` / `config_profile_apply`
//! - `ranking_env_get` / `ranking_env_set`
//!
//! ## Does not own
//! - Overlay merge order or `config_get` / `config_set`
//! - Ranking weight validation math (see `ranking`)
//!
//! ## Boundary
//!
//! Changes to profile-apply side effects on Runtime should normally require changes here.
//! Changes to config path validation should not require changes here.
//!
//! Keep this module focused on profile and `ranking_env` Runtime handlers.

use super::merge::json_diff;
use super::{
    ConfigProfileApplyData, ConfigProfilesData, ProfileSummary, RankingEnvSetData, Runtime,
};
use crate::error::{AmError, ErrorCode, ToolResult};
use crate::log;
use crate::profile::{Profile, builtin_profiles, coding_tool_inventory, lookup_profile};
use crate::ranking::{RankingEnv, RankingEnvPatch, preview_patch};
use serde_json::{Value, json};

impl Runtime {
    #[must_use]
    pub fn config_profiles(&self) -> ToolResult<ConfigProfilesData> {
        let profiles: Vec<ProfileSummary> = builtin_profiles()
            .into_iter()
            .map(|p| ProfileSummary {
                name: p.name,
                description: p.description,
                single_host: p.single_host,
                omit_sync: p.omit_sync,
                fr10_satisfied: p.fr10_satisfied,
            })
            .collect();
        ToolResult::success(ConfigProfilesData {
            profiles,
            active: self.active_profile.clone(),
        })
    }

    pub fn config_profile_apply(&mut self, name: &str) -> ToolResult<ConfigProfileApplyData> {
        ToolResult::from_result(self.config_profile_apply_inner(name))
    }

    fn config_profile_apply_inner(
        &mut self,
        name: &str,
    ) -> Result<ConfigProfileApplyData, AmError> {
        let profile = lookup_profile(name).ok_or_else(|| {
            AmError::new(
                ErrorCode::UnknownProfile,
                format!("unknown profile `{name}`"),
            )
        })?;
        let before = self.effective_masked();
        let omit_sync = profile.omit_sync;
        self.apply_profile(&profile);
        log::info("config_profile_apply", &[("name", &profile.name)]);
        let after = self.effective_masked();
        Ok(ConfigProfileApplyData {
            name: profile.name,
            diff: json_diff(&before, &after),
            bank_policy: self.bank_policy.clone(),
            fr10_satisfied: false,
            note: "Profile apply changes tunable knobs only; it does not bypass the category or admission gates, and packaging alone does not satisfy them.".into(),
            tool_inventory: coding_tool_inventory(omit_sync),
        })
    }

    pub(super) fn apply_profile(&mut self, profile: &Profile) {
        self.active_profile = Some(profile.name.clone());
        self.bank_policy = profile.bank_default.clone();
        self.ranking = profile.ranking.clone();
        self.profile_overlay.clear();
        self.profile_bundle.clear();
        self.profile_bundle.insert(
            "backend".into(),
            Value::String(profile.backend_hint.clone()),
        );
        self.profile_bundle
            .insert("log.level".into(), Value::String(profile.log_level.clone()));
        self.profile_bundle.insert(
            "injection.budget_tokens".into(),
            json!(profile.injection_budget_tokens),
        );
        self.profile_bundle
            .insert("sync.omit".into(), json!(profile.omit_sync));
        self.profile_bundle
            .insert("mcp.revision".into(), json!(profile.mcp_revision));
        self.profile_bundle
            .insert("mcp.transports".into(), json!(profile.mcp_transports));
        self.profile_bundle
            .insert("fr10_satisfied".into(), json!(false));
        self.sync_log_level();
    }

    #[must_use]
    pub fn ranking_env_get(&self) -> ToolResult<RankingEnv> {
        ToolResult::success(self.ranking.clone())
    }

    /// The live ranking environment, borrowed.
    ///
    /// `ranking_env_set` writes `self.ranking` in place, so an in-process caller
    /// that needs the value a wave will actually use reads it here rather than
    /// unwrapping the tool envelope from [`Self::ranking_env_get`].
    #[must_use]
    pub fn ranking_env(&self) -> &RankingEnv {
        &self.ranking
    }

    pub fn ranking_env_set(
        &mut self,
        patch: &RankingEnvPatch,
        dry_run: bool,
    ) -> ToolResult<RankingEnvSetData> {
        ToolResult::from_result(self.ranking_env_set_inner(patch, dry_run))
    }

    fn ranking_env_set_inner(
        &mut self,
        patch: &RankingEnvPatch,
        dry_run: bool,
    ) -> Result<RankingEnvSetData, AmError> {
        let normalized = preview_patch(&self.ranking, patch)?;
        if dry_run {
            return Ok(RankingEnvSetData {
                dry_run: true,
                applied: false,
                ranking: normalized,
            });
        }
        self.ranking = normalized.clone();
        Ok(RankingEnvSetData {
            dry_run: false,
            applied: true,
            ranking: normalized,
        })
    }
}

#[cfg(test)]
#[path = "profile_ops_tests.rs"]
mod profile_ops_tests;
