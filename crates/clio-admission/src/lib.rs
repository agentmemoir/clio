// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Category gate and five-factor admission scoring.
//!
//! Pure logic over [`RankingEnv`]; no I/O. Later write tools call
//! [`gated_create`] so no public long-term write bypasses these gates.

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]

pub mod decision;
pub mod factors;
pub mod gated;
pub mod policy;

pub use clio_config::ranking::RankingEnv;
pub use decision::{
    Candidate, Decision, EXACT_DUPLICATE_REASON, FactorBreakdown, SignalInputs, admit_preview,
    admit_preview_raw, decide, decide_raw, decide_ungated, is_exact_duplicate_reason,
};
pub use factors::factor_unavailable;
pub use gated::{
    AdmissionEvent, EventSink, GateRequest, SignalSource, VecSink, candidate_text, gated_create,
    gated_create_raw, signals_for,
};
pub use policy::AdmissionPolicy;
