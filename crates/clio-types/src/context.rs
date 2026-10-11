// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for per-operation `bank` isolation and `actor` attribution.
//!
//! ## Owns
//! - `Bank`, `Actor`, and `OpContext` types, including the optional presenting
//!   credential label on the context
//! - Bank-default policy resolution (`Required` / `Fixed` / `PerRepository`)
//!
//! ## Does not own
//! - Profile catalog contents or config overlays
//! - Durable bank storage or multi-bank backends
//!
//! ## Boundary
//!
//! Changes to bank/actor context semantics should normally require changes here.
//! Changes to profile packaging or config merge should not require changes here.
//!
//! Keep this module focused on bank isolation and actor attribution context.

use crate::audit_principal::PrincipalLabel;
use crate::error::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Isolation key (per-repo / per-user / per-agent).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Bank(String);

impl Bank {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Bank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Attribution actor; values align with telemetry (`agent` / `harness` / `user` / `system`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Agent,
    Harness,
    User,
    System,
}

impl Actor {
    /// Every actor in canonical order.
    pub const ALL: &[Self] = &[Self::Agent, Self::Harness, Self::User, Self::System];

    /// Wire strings in canonical order (parallel to `ALL`).
    pub const WIRE: &[&'static str] = &["agent", "harness", "user", "system"];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Harness => "harness",
            Self::User => "user",
            Self::System => "system",
        }
    }

    /// Parse a caller-supplied actor string.
    ///
    /// # Errors
    /// Returns [`ErrorCode::InvalidArgument`] when the value is not one of the four allowed actors.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "agent" => Ok(Self::Agent),
            "harness" => Ok(Self::Harness),
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("actor must be {}, got `{other}`", Self::WIRE.join("|")),
            )),
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a coding (or other) profile supplies a default bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BankDefaultPolicy {
    /// Caller must supply `bank`; no default.
    Required,
    /// Use a fixed bank id when the caller omits it.
    Fixed { bank: String },
    /// Derive `repo:<normalized_root>` from an explicit repo root or process cwd.
    PerRepository,
}

/// Per-operation context carried on every tool path.
///
/// `actor` is the role the operation runs as, which a caller may declare in its
/// arguments. `principal` is the credential that presented the request, and it
/// is absent unless a credential named one: a local process owner already holds
/// the store, so there is nothing to name. Where a credential did name one, the
/// two cannot disagree, because the transport refuses a declared actor that
/// differs from the bound one and falls back to the bound one otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpContext {
    pub bank: Bank,
    pub actor: Actor,
    /// Label of the presenting credential; `None` on an unbound transport.
    pub principal: Option<PrincipalLabel>,
}

impl OpContext {
    /// An operation with no presenting credential.
    ///
    /// This is the local-transport case and the default for every construction
    /// that never saw a credential.
    #[must_use]
    pub fn new(bank: Bank, actor: Actor) -> Self {
        Self {
            bank,
            actor,
            principal: None,
        }
    }

    /// An operation presented by a named credential.
    #[must_use]
    pub fn with_principal(bank: Bank, actor: Actor, principal: PrincipalLabel) -> Self {
        Self {
            bank,
            actor,
            principal: Some(principal),
        }
    }

    /// The presenting credential's label, or `None` when none was named.
    #[must_use]
    pub fn principal(&self) -> Option<&PrincipalLabel> {
        self.principal.as_ref()
    }
}

/// Resolve bank for an operation.
///
/// Precedence: explicit argument → `fixed` (`CLIO_BANK` / `bank.fixed`) → profile policy.
///
/// # Errors
/// - [`ErrorCode::MissingBank`] when policy is `Required` and no bank was supplied.
/// - [`ErrorCode::MissingBank`] when per-repository derivation cannot obtain a root path.
pub fn resolve_bank(
    policy: &BankDefaultPolicy,
    explicit: Option<&str>,
    fixed: Option<&str>,
    repo_root: Option<&str>,
) -> Result<Bank, AmError> {
    if let Some(raw) = explicit {
        if raw.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "bank must be a non-empty string",
            ));
        }
        return Ok(Bank::new(raw));
    }
    if let Some(raw) = fixed {
        if raw.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "bank.fixed must be a non-empty string",
            ));
        }
        return Ok(Bank::new(raw));
    }
    match policy {
        BankDefaultPolicy::Required => Err(AmError::new(
            ErrorCode::MissingBank,
            "bank is required by the active profile and was not supplied",
        )),
        BankDefaultPolicy::Fixed { bank } => Ok(Bank::new(bank.clone())),
        BankDefaultPolicy::PerRepository => {
            let root = match repo_root {
                Some(r) => r.to_owned(),
                None => cwd_root(std::env::current_dir()),
            };
            Ok(Bank::new(format!("repo:{}", normalize_repo_root(&root))))
        }
    }
}

fn cwd_root(dir: Result<std::path::PathBuf, std::io::Error>) -> String {
    match dir {
        Ok(p) => p.display().to_string(),
        Err(_) => ".".to_owned(),
    }
}

fn normalize_repo_root(raw: &str) -> String {
    raw.trim_end_matches(['/', '\\']).replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_repository_derives_bank_from_root() {
        let bank = resolve_bank(
            &BankDefaultPolicy::PerRepository,
            None,
            None,
            Some("/Users/me/proj/"),
        )
        .expect("derive");
        assert_eq!(bank.as_str(), "repo:/Users/me/proj");
    }

    #[test]
    fn required_policy_rejects_missing_bank() {
        let err =
            resolve_bank(&BankDefaultPolicy::Required, None, None, None).expect_err("missing");
        assert_eq!(err.code, ErrorCode::MissingBank);
    }

    #[test]
    fn fixed_env_beats_policy() {
        let bank = resolve_bank(
            &BankDefaultPolicy::PerRepository,
            None,
            Some("from-env"),
            Some("/tmp/repo"),
        )
        .expect("fixed");
        assert_eq!(bank.as_str(), "from-env");
    }

    #[test]
    fn explicit_bank_overrides_policy() {
        let bank = resolve_bank(
            &BankDefaultPolicy::Fixed {
                bank: "ignored".into(),
            },
            Some("custom"),
            Some("also-ignored"),
            None,
        )
        .expect("override");
        assert_eq!(bank.as_str(), "custom");
    }

    #[test]
    fn actor_tables_are_parallel() {
        assert_eq!(Actor::ALL.len(), Actor::WIRE.len());
        for (variant, wire) in Actor::ALL.iter().zip(Actor::WIRE) {
            assert_eq!(variant.as_str(), *wire);
            assert_eq!(Actor::parse(wire).unwrap(), *variant);
        }
    }

    #[test]
    fn actors_banks_and_edge_cases() {
        assert_eq!(Actor::parse("agent").unwrap(), Actor::Agent);
        assert_eq!(Actor::parse("harness").unwrap(), Actor::Harness);
        assert_eq!(Actor::parse("user").unwrap(), Actor::User);
        assert_eq!(Actor::parse("system").unwrap(), Actor::System);
        assert_eq!(
            Actor::parse("nope").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(Actor::Agent.as_str(), "agent");
        assert_eq!(Actor::Harness.as_str(), "harness");
        assert_eq!(Actor::User.as_str(), "user");
        assert_eq!(Actor::System.as_str(), "system");
        assert_eq!(Actor::Agent.to_string(), "agent");
        assert_eq!(Actor::Harness.to_string(), "harness");
        assert_eq!(Actor::User.to_string(), "user");
        assert_eq!(Actor::System.to_string(), "system");
        assert_eq!(Bank::new("b").to_string(), "b");
        let ctx = OpContext::new(Bank::new("x"), Actor::User);
        assert_eq!(ctx.actor, Actor::User);
        assert_eq!(ctx.principal(), None);
        assert_eq!(ctx.principal, None);

        assert_eq!(
            resolve_bank(&BankDefaultPolicy::Required, Some(""), None, None)
                .unwrap_err()
                .code,
            ErrorCode::MissingBank
        );
        assert_eq!(
            resolve_bank(&BankDefaultPolicy::Required, None, Some(""), None)
                .unwrap_err()
                .code,
            ErrorCode::MissingBank
        );
        let bank = resolve_bank(
            &BankDefaultPolicy::Fixed {
                bank: "fixed-bank".into(),
            },
            None,
            None,
            None,
        )
        .expect("fixed policy");
        assert_eq!(bank.as_str(), "fixed-bank");

        // PerRepository without explicit root uses cwd.
        let bank = resolve_bank(&BankDefaultPolicy::PerRepository, None, None, None).expect("cwd");
        assert!(bank.as_str().starts_with("repo:"));

        assert_eq!(cwd_root(Ok(std::path::PathBuf::from("/tmp/x"))), "/tmp/x");
        assert_eq!(
            cwd_root(Err(std::io::Error::other("no cwd"))),
            ".".to_owned()
        );
    }

    #[test]
    fn with_principal_names_the_credential_without_changing_the_actor() {
        let ctx = OpContext::with_principal(
            Bank::new("bank-a"),
            Actor::Harness,
            PrincipalLabel::new("ops-runner"),
        );
        assert_eq!(ctx.bank.as_str(), "bank-a");
        assert_eq!(ctx.actor, Actor::Harness);
        assert_eq!(
            ctx.principal().map(PrincipalLabel::as_str),
            Some("ops-runner")
        );
        assert_eq!(
            ctx.principal,
            Some(PrincipalLabel::new("ops-runner")),
            "the accessor and the field are the same value"
        );
    }

    #[test]
    fn a_bound_context_differs_from_an_unbound_one_only_in_the_label() {
        let unbound = OpContext::new(Bank::new("bank-a"), Actor::Agent);
        let bound =
            OpContext::with_principal(Bank::new("bank-a"), Actor::Agent, PrincipalLabel::new("k"));
        assert_ne!(bound, unbound, "the label is what separates the two");
        assert_eq!(bound.bank, unbound.bank);
        assert_eq!(bound.actor, unbound.actor);
    }
}
