// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the small per-service values a step asks with:
//! the endpoint, model, provider, and stored-key flag a hosted role offers as its
//! defaults, and the heading that role is asked under.
//!
//! ## Owns
//! - `embed_defaults`, the embedding service's defaults
//! - `rerank_defaults`, the reranking service's defaults
//! - `title`, the heading one of the two service groups is asked under
//!
//! ## Does not own
//! - Which questions a step asks (see `setup_step_ask`)
//! - Where the stored values come from (see `setup_cli_flows`)
//! - The hosted role's endpoint check (see `setup_validate`)
//!
//! ## Boundary
//!
//! Changes to what a hosted service offers as its default should normally require
//! changes here. Changes to the questions themselves should not require changes
//! here.
//!
//! Keep this module focused on the defaults one service offers.

use crate::setup_cli_flows::Ctx;
use crate::setup_services::ServiceDefaults;

/// The embedding defaults, from the last run.
pub(crate) fn embed_defaults(ctx: &Ctx<'_>) -> ServiceDefaults {
    let defaults = ctx.defaults();
    ServiceDefaults {
        url: defaults.embed_url.clone(),
        model: defaults.embed_model.clone(),
        provider: defaults.embed_provider.clone(),
        has_key: defaults.has_embed_key,
        dims: None,
    }
}

/// The rerank defaults, from the last run.
pub(crate) fn rerank_defaults(ctx: &Ctx<'_>) -> ServiceDefaults {
    let defaults = ctx.defaults();
    ServiceDefaults {
        url: defaults.rerank_url.clone(),
        model: defaults.rerank_model.clone(),
        provider: defaults.rerank_provider.clone(),
        has_key: defaults.has_rerank_key,
        dims: None,
    }
}

/// `Embedding` or `Reranking`.
pub(crate) fn title(name: &str) -> &'static str {
    if name == "embed" {
        "Embedding"
    } else {
        "Reranking"
    }
}
