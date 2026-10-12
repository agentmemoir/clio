// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the dense/lexical index pipelines: embedding
//! sidecar client, durable outbox worker, search primitives, and rebuild hooks.
//!
//! ## Owns
//! - `Embedder` trait + HTTP sidecar client (TEI-compatible `/embed`)
//! - `IndexCoordinator`: outbox worker, rebuild hooks, pipeline status
//! - `dense_search` / `lexical_search` primitive facades over `IndexStore`
//!
//! ## Does not own
//! - Backend index SQL (see `clio_store::IndexStore` implementations)
//! - Hybrid fusion, rerank, or the intent gate (owned by the retrieval layer)
//! - Item admission or MemTree maintenance
//!
//! ## Boundary
//!
//! Changes to index pipeline behavior (embed/worker/rebuild/search) should
//! normally require changes here. Changes to backend SQL or admission gates
//! should not require changes here.
//!
//! # Index maintenance contract
//!
//! Leaf writes durably enqueue an index job inside the same transaction that
//! commits the item (outbox in `clio_store`), so the write ACK never waits
//! on embedding HTTP. [`IndexCoordinator`] drains the outbox off the write
//! path: decrypt → assemble text → sidecar embed → dense upsert + lexical doc.
//! The two legs treat the assembled text differently, and the difference is the
//! point: the dense leg sends it to the embedder and stores a vector, while the
//! lexical leg hands it to the store, which replaces every stemmed token with a
//! keyed HMAC-SHA256 digest before `content_text` / `content_tsv` is written. No
//! readable word is persisted on the lexical path. Failures leave the job
//! pending with a bounded retry backoff; leaves stay readable throughout.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::option_if_let_else)]

mod embed;
mod http;
mod openai;
mod probe;
mod provider;
mod rebuild;
mod report;
mod search;
mod worker;
mod worker_observe;

#[cfg(test)]
mod embed_test_support;

pub use clio_store::{EMBEDDING_DIMS, IndexCoverage, IndexJob, KnnHit, SearchHit};
pub use clio_types::SearchDomain;
pub use embed::{Embedder, TeiEmbedder, embed_text, lexical_text, scrub_embed_text};
pub use http::{HttpEndpoint, get_json, post_json};
pub use openai::OpenAiEmbedder;
pub use probe::{PROBE_TIMEOUT, endpoint as probe_endpoint};
pub use provider::{EMBED_PROVIDERS, build_embedder};
pub use rebuild::{REBUILD_EMBED_BATCH, REBUILD_PAGE, rebuild};
pub use report::{EmbedStatus, IndexStatus, ProcessReport, RebuildReport, RebuildTargets};
pub use search::{DenseSearchArgs, LexicalSearchArgs, dense_search, lexical_search};
pub use worker::{DEFAULT_JOB_BATCH, DEFAULT_MAX_ATTEMPTS, IndexCoordinator, RETRY_BACKOFF_MS};
