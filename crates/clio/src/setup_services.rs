// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what one model service *is*: where it runs,
//! what values it holds, and which config entries it writes.
//!
//! ## Owns
//! - `ServiceRole`: local Docker, hosted API, or skip
//! - `Service`, one service's resolved values
//! - `entries` and `watched`, the fields a service writes and the fields it owns
//! - `ServiceDefaults`, the last-known values a question offers
//!
//! ## Does not own
//! - The questions (see `setup_step_ask`)
//! - The endpoint check (see `setup_validate`)
//! - The probe (see `setup_test_seam`)
//! - The preview (see `setup_report`)
//!
//! ## Boundary
//!
//! Changes to what a service is or writes should normally require changes here.
//! Changes to which questions are asked should not.
//!
//! Keep this module focused on the values, with no prompting.
//!
//! # Why `watched` is separate from `entries`
//!
//! `entries` is what the run writes; `watched` is everything the run *owns*.
//! Switching reranking off writes nothing, and a preview built only from
//! `entries` would show no reranking block at all — leaving a stale endpoint in
//! the config with nothing telling the operator it would be removed. So the
//! preview is given `watched` and decides what is removed from that.

use crate::setup_wizard::{
    LOCAL_EMBED_MODEL, LOCAL_EMBED_URL, LOCAL_RERANK_MODEL, LOCAL_RERANK_URL,
};
use serde_json::{Value, json};

/// Where one model service runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceRole {
    /// The local Docker sidecar.
    Local,
    /// A hosted API the operator holds a key for.
    Hosted,
    /// Not configured. Reranking and extraction only.
    Skip,
}

impl ServiceRole {
    /// Read a role from its flag spelling.
    #[must_use]
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "online" | "hosted" | "remote" => Some(Self::Hosted),
            "skip" | "none" | "off" => Some(Self::Skip),
            _ => None,
        }
    }

    /// The word the summary and the preview use.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Hosted => "hosted",
            Self::Skip => "skip",
        }
    }

    /// The Compose service this role names, when it starts one.
    ///
    /// The one place a role becomes a service name, so the preview's `Will start`
    /// line and the start command cannot be two different strings.
    #[must_use]
    pub(crate) fn compose_service(self, name: &str) -> Option<&'static str> {
        match (self, name) {
            (Self::Local, "embed") => Some("embed"),
            (Self::Local, "rerank") => Some("rerank"),
            (Self::Local, "extract") => Some("extract"),
            _ => None,
        }
    }
}

/// One service's resolved values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Service {
    /// Where it runs.
    pub(crate) role: ServiceRole,
    /// The endpoint.
    pub(crate) url: String,
    /// The model name.
    pub(crate) model: String,
    /// The API key. Never rendered, and masked before it reaches a preview.
    pub(crate) key: Option<String>,
    /// The provider name, when the operator or the default chose one.
    pub(crate) provider: Option<String>,
    /// The embedding dimension count, hosted embedding only.
    pub(crate) dims: Option<u64>,
    /// What the probe reported, or `None` when it was never run.
    pub(crate) tested: Option<String>,
}

impl Service {
    /// The local sidecar defaults for one service.
    ///
    /// The provider is `tei` and the endpoint and model are the compose sidecar's,
    /// all three read from one place so the wizard cannot point at a sidecar that
    /// is not the one it would start.
    #[must_use]
    pub(crate) fn local(name: &str) -> Self {
        let (url, model) = if name == "embed" {
            (LOCAL_EMBED_URL, LOCAL_EMBED_MODEL)
        } else {
            (LOCAL_RERANK_URL, LOCAL_RERANK_MODEL)
        };
        Self {
            role: ServiceRole::Local,
            url: url.to_owned(),
            model: model.to_owned(),
            key: None,
            provider: Some("tei".to_owned()),
            dims: None,
            tested: None,
        }
    }

    /// A service this install does not configure.
    #[must_use]
    pub(crate) fn skipped() -> Self {
        Self {
            role: ServiceRole::Skip,
            url: String::new(),
            model: String::new(),
            key: None,
            provider: None,
            dims: None,
            tested: None,
        }
    }

    /// The config entries this service writes, in the order they are applied.
    ///
    /// A skipped service writes nothing. The provider falls back to the
    /// role-appropriate default — `tei` for a local sidecar, the hosted provider
    /// for an API — so a run that never chose a provider still writes a value the
    /// index understands.
    #[must_use]
    pub(crate) fn entries(&self, name: &str, hosted_provider: &str) -> Vec<(String, Value)> {
        if self.role == ServiceRole::Skip {
            return Vec::new();
        }
        let provider = self
            .provider
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| match self.role {
                ServiceRole::Hosted => hosted_provider.to_owned(),
                _ => "tei".to_owned(),
            });
        let mut entries = vec![
            (format!("{name}.provider"), json!(provider)),
            (format!("{name}.url"), json!(self.url)),
            (format!("{name}.model"), json!(self.model)),
        ];
        if let Some(key) = self.key.as_deref().filter(|k| !k.trim().is_empty()) {
            entries.push((format!("credentials.{name}_api_key"), json!(key)));
        }
        // The scrape of `--embed-dims` applies only to a hosted embed service: a
        // local sidecar reports its own dimension count, and a stored override
        // would make Clio disagree with it.
        if name == "embed"
            && let Some(dims) = self.dims
            && self.role == ServiceRole::Hosted
        {
            entries.push(("embed.dims".to_owned(), json!(dims)));
        }
        entries
    }

    /// Every watched path this service owns, whether or not it writes them.
    ///
    /// The preview needs the full set so a field that is *not* being written shows
    /// as removed rather than silently absent.
    #[must_use]
    pub(crate) fn watched(name: &str) -> Vec<&'static str> {
        let mut watched = match name {
            "embed" => vec!["embed.provider", "embed.url", "embed.model"],
            _ => vec!["rerank.provider", "rerank.url", "rerank.model"],
        };
        if name == "embed" {
            watched.push("embed.dims");
        }
        watched.push(if name == "embed" {
            "credentials.embed_api_key"
        } else {
            "credentials.rerank_api_key"
        });
        watched
    }
}

/// The defaults a service question offers.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServiceDefaults {
    /// The last endpoint.
    pub(crate) url: Option<String>,
    /// The last model.
    pub(crate) model: Option<String>,
    /// The last provider.
    pub(crate) provider: Option<String>,
    /// Whether a key is already stored.
    pub(crate) has_key: bool,
    /// The last dimension count.
    pub(crate) dims: Option<u64>,
}
