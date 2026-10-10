// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for JSON tool-name dispatch into `Runtime` handlers.
//!
//! ## Owns
//! - `call_tool` argument decoding for config/ranking tools
//! - Stub `not_implemented` responses for other catalog names
//!
//! ## Does not own
//! - Config merge or ranking validation logic
//!
//! ## Boundary
//!
//! Changes to tool name routing should normally require changes here.
//! Changes to handler internals should not require changes here.
//!
//! Keep this module focused on tool-name dispatch.

use super::{ConfigScope, Runtime};
use crate::error::{AmError, ErrorCode, ToolResult};
use crate::ranking::RankingEnvPatch;
use serde_json::Value;

impl Runtime {
    /// Dispatch a named tool; unimplemented catalog entries return `not_implemented`.
    ///
    /// # Panics
    /// Panics if a tool response fails to serialize (should not happen for these payloads).
    pub fn call_tool(&mut self, name: &str, args: &Value) -> Value {
        match name {
            "config_get" => {
                let path = args.get("path").and_then(Value::as_str);
                serde_json::to_value(self.config_get(path)).expect("serialize config_get")
            }
            "config_set" => dispatch_config_set(self, args),
            "config_profiles" => {
                serde_json::to_value(self.config_profiles()).expect("serialize config_profiles")
            }
            "config_profile_apply" => {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("");
                serde_json::to_value(self.config_profile_apply(name))
                    .expect("serialize config_profile_apply")
            }
            "ranking_env_get" => {
                serde_json::to_value(self.ranking_env_get()).expect("serialize ranking_env_get")
            }
            "ranking_env_set" => dispatch_ranking_set(self, args),
            "retention_profile_get" => dispatch_retention_get(self, args),
            "retention_profile_set" => dispatch_retention_set(self, args),
            other => {
                serde_json::to_value(ToolResult::<()>::failure(AmError::not_implemented(other)))
                    .expect("serialize not_implemented")
            }
        }
    }
}

fn dispatch_config_set(rt: &mut Runtime, args: &Value) -> Value {
    let path = args.get("path").and_then(Value::as_str).unwrap_or("");
    let value = args.get("value").cloned().unwrap_or(Value::Null);
    let scope = args
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("session");
    match ConfigScope::parse(scope) {
        Ok(scope) => {
            serde_json::to_value(rt.config_set(path, value, scope)).expect("serialize config_set")
        }
        Err(e) => {
            serde_json::to_value(ToolResult::<()>::failure(e)).expect("serialize scope error")
        }
    }
}

fn dispatch_ranking_set(rt: &mut Runtime, args: &Value) -> Value {
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let patch: RankingEnvPatch = match args.get("patch") {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_value(ToolResult::<()>::failure(AmError::new(
                    ErrorCode::InvalidType,
                    format!("invalid ranking patch: {e}"),
                )))
                .expect("serialize patch error");
            }
        },
        None => RankingEnvPatch::default(),
    };
    serde_json::to_value(rt.ranking_env_set(&patch, dry_run)).expect("serialize ranking_env_set")
}

fn dispatch_retention_get(rt: &Runtime, args: &Value) -> Value {
    let bank = args.get("bank").and_then(Value::as_str).unwrap_or("");
    serde_json::to_value(rt.retention_profile_get(bank)).expect("serialize retention_profile_get")
}

fn dispatch_retention_set(rt: &mut Runtime, args: &Value) -> Value {
    let profile = match args.get("profile") {
        Some(p) => match serde_json::from_value::<crate::retention::RetentionProfile>(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_value(ToolResult::<()>::failure(AmError::new(
                    ErrorCode::InvalidType,
                    format!("invalid retention profile: {e}"),
                )))
                .expect("serialize profile error");
            }
        },
        None => {
            return serde_json::to_value(ToolResult::<()>::failure(AmError::new(
                ErrorCode::InvalidArgument,
                "profile is required",
            )))
            .expect("serialize profile error");
        }
    };
    let scope = args.get("scope").and_then(Value::as_str).unwrap_or("bank");
    let result = match scope {
        "bank" => {
            let bank = args.get("bank").and_then(Value::as_str).unwrap_or("");
            rt.retention_profile_set(bank, profile)
        }
        "deployment" => rt.retention_profile_set_deployment(profile),
        other => {
            return serde_json::to_value(ToolResult::<()>::failure(AmError::new(
                ErrorCode::InvalidArgument,
                format!("scope must be bank|deployment, got `{other}`"),
            )))
            .expect("serialize scope error");
        }
    };
    serde_json::to_value(result).expect("serialize retention_profile_set")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_tool_covers_config_and_ranking_paths() {
        let mut rt = Runtime::with_env(&|_| None);
        assert_eq!(rt.call_tool("config_get", &json!({}))["ok"], json!(true));
        assert_eq!(
            rt.call_tool("config_profiles", &json!({}))["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool("config_profile_apply", &json!({"name": "coding_local"}))["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool(
                "config_set",
                &json!({"path": "backend", "value": "postgres", "scope": "session"})
            )["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool("config_set", &json!({"path": "backend", "scope": "bogus"}))["ok"],
            json!(false)
        );
        assert_eq!(
            rt.call_tool("ranking_env_get", &json!({}))["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool("ranking_env_set", &json!({"dry_run": true}))["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool(
                "ranking_env_set",
                &json!({"patch": {"theta_admit": 0.4}, "dry_run": false})
            )["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool("ranking_env_set", &json!({"patch": 1}))["ok"],
            json!(false)
        );
        assert_eq!(rt.call_tool("store", &json!({}))["ok"], json!(false));
    }

    #[test]
    fn call_tool_covers_retention_profile_paths() {
        let mut rt = Runtime::with_env(&|_| None);
        assert_eq!(
            rt.call_tool("retention_profile_get", &json!({"bank": "bank-a"}))["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool("retention_profile_get", &json!({}))["ok"],
            json!(false)
        );
        assert_eq!(
            rt.call_tool(
                "retention_profile_set",
                &json!({"bank": "bank-a", "profile": {"verbosity": "selective"}})
            )["ok"],
            json!(true)
        );
        assert_eq!(
            rt.call_tool(
                "retention_profile_set",
                &json!({"profile": {"verbosity": "selective"}, "scope": "deployment"})
            )["ok"],
            json!(true)
        );
        // `permissive` MUST NOT become a deployment default.
        assert_eq!(
            rt.call_tool(
                "retention_profile_set",
                &json!({"profile": {"verbosity": "permissive"}, "scope": "deployment"})
            )["ok"],
            json!(false)
        );
        assert_eq!(
            rt.call_tool(
                "retention_profile_set",
                &json!({"bank": "bank-a", "profile": {"verbosity": "permissive"}, "scope": "bogus"})
            )["ok"],
            json!(false)
        );
        assert_eq!(
            rt.call_tool("retention_profile_set", &json!({"bank": "bank-a"}))["ok"],
            json!(false)
        );
        assert_eq!(
            rt.call_tool(
                "retention_profile_set",
                &json!({"bank": "bank-a", "profile": {"verbosity": "nope"}})
            )["ok"],
            json!(false)
        );
        let get = rt.call_tool("retention_profile_get", &json!({"bank": "bank-a"}));
        assert_eq!(get["source"], json!("bank"));
        assert_eq!(get["verbosity"], json!("selective"));
        assert_eq!(
            get["accepted_but_inert"]["honored_by"],
            json!("recall_scope_and_dedup")
        );
    }
}
