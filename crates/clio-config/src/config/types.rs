// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for serializable payload types returned by config/ranking tools.
//!
//! ## Owns
//! - `ConfigGetData`, `ConfigSetData`, profile list/apply payloads, ranking set payloads
//!
//! ## Does not own
//! - How those payloads are computed
//! - Error envelopes (`error::ToolResult`)
//!
//! ## Boundary
//!
//! Changes to success-payload field shape for config/ranking tools should normally require changes here.
//! Changes to Runtime behavior should not require changes here unless the response schema changes.
//!
//! Keep this module focused on config/ranking tool success payload types.

use crate::context::BankDefaultPolicy;
use crate::profile::ToolInventoryEntry;
use crate::ranking::RankingEnv;
use crate::retention::RetentionProfile;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ConfigGetData {
    pub path: String,
    pub value: Value,
    pub active_profile: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConfigSetData {
    pub path: String,
    pub scope: String,
    pub value: Value,
}

#[derive(Debug, Serialize)]
pub struct ProfileSummary {
    pub name: String,
    pub description: String,
    pub single_host: bool,
    pub omit_sync: bool,
    pub fr10_satisfied: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigProfilesData {
    pub profiles: Vec<ProfileSummary>,
    pub active: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConfigProfileApplyData {
    pub name: String,
    pub diff: Value,
    pub bank_policy: BankDefaultPolicy,
    pub fr10_satisfied: bool,
    pub note: String,
    pub tool_inventory: Vec<ToolInventoryEntry>,
}

#[derive(Debug, Serialize)]
pub struct RankingEnvSetData {
    pub dry_run: bool,
    pub applied: bool,
    pub ranking: RankingEnv,
}

#[derive(Debug, Serialize)]
pub struct RetentionProfileGetData {
    pub bank: String,
    pub source: &'static str,
    pub verbosity: &'static str,
    pub theta_offsets: std::collections::BTreeMap<String, f64>,
    pub duplicate_tolerance_write: &'static str,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission: Option<RetentionMissionSummary>,
    pub accepted_but_inert: Value,
}

/// Mission summary shown by profile-get surfaces (counts, not full lists).
#[derive(Debug, Serialize)]
pub struct RetentionMissionSummary {
    pub statement: String,
    pub keep_examples: usize,
    pub drop_examples: usize,
    pub schema_version: u32,
}

impl RetentionMissionSummary {
    /// Summarize the mission stored on a profile; `None` when absent.
    #[must_use]
    pub fn from_profile(profile: &RetentionProfile) -> Option<Self> {
        let mission = profile.mission.as_ref()?;
        Some(Self {
            statement: mission.mission_statement.clone(),
            keep_examples: mission.keep_examples.len(),
            drop_examples: mission.drop_examples.len(),
            schema_version: crate::retention::RETENTION_PROFILE_VERSION,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct RetentionProfileSetData {
    pub bank: Option<String>,
    pub scope: &'static str,
    pub profile: RetentionProfile,
    pub accepted_but_inert: Value,
    pub persisted: bool,
}

#[cfg(test)]
#[path = "types_embed_knob_tests.rs"]
mod types_embed_knob_tests;

#[cfg(test)]
#[path = "types_extract_knob_tests.rs"]
mod types_extract_knob_tests;
