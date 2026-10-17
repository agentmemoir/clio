// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the local request commands and the total,
//! idempotent transition table that maps a request state and a command onto
//! the next state.
//!
//! ## Owns
//! - `RequestCommand`, the data-only commands a ledger applies
//! - `transition`, the pure total transition function
//!
//! ## Does not own
//! - `RequestState`, `FinalOutcome`, and `StateCategory`, the states the table
//!   maps (`subject_request.rs`)
//! - The ledger that applies a command to a stored record and persists it
//!   (`subject_request.rs`)
//! - Any store, record, or I/O: commands are data and `transition` is pure
//!
//! ## Boundary
//!
//! Changes to the command set or to which transitions are allowed should
//! normally require changes here. Changes to request storage, the record
//! vocabulary, or the ledger's persistence should not.
//!
//! Keep this module focused on the command contract and its transition table.

use crate::subject_request::{FinalOutcome, RequestState};
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

/// A local command on one request. Data only; a ledger applies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestCommand {
    /// Start the work: `Received` to `InProgress`.
    Begin,
    /// Record that the technical work is done: `InProgress` to
    /// `TechnicalComplete`.
    MarkTechnicalComplete,
    /// Hand the result to the operator: `TechnicalComplete` to
    /// `AwaitingOperator`.
    AwaitOperator,
    /// Record the final outcome: `AwaitingOperator` to `Final`.
    Finalize {
        /// The outcome the operator recorded.
        outcome: FinalOutcome,
    },
    /// Deny the request.
    Refuse {
        /// Why the request is denied; must stay generic.
        rationale: String,
    },
    /// Record an exception: `InProgress` to `Blocked`.
    Block {
        /// What stopped the work; must stay generic.
        reason: String,
    },
    /// Clear the exception: `Blocked` to `InProgress`.
    Resume,
}

impl RequestCommand {
    /// Every command, one per variant. Tests consume this so a new command
    /// fails the completeness guard instead of passing silently.
    #[must_use]
    pub fn all() -> [Self; 8] {
        [
            Self::Begin,
            Self::MarkTechnicalComplete,
            Self::AwaitOperator,
            Self::Finalize {
                outcome: FinalOutcome::Completed,
            },
            Self::Finalize {
                outcome: FinalOutcome::PartiallyCompleted,
            },
            Self::Refuse {
                rationale: "refused".into(),
            },
            Self::Block {
                reason: "blocked".into(),
            },
            Self::Resume,
        ]
    }
}

/// The total transition table: pure and idempotent, so a repeated input
/// never produces a second transition.
pub fn transition(state: &RequestState, command: &RequestCommand) -> Result<RequestState, AmError> {
    fn refuse(state_kind: &str) -> Result<RequestState, AmError> {
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("command not applicable to a {state_kind} request"),
        ))
    }
    match state {
        RequestState::Received => match command {
            RequestCommand::Begin => Ok(RequestState::InProgress),
            RequestCommand::Refuse { rationale } => Ok(RequestState::Refused {
                rationale: rationale.clone(),
            }),
            _ => refuse("received"),
        },
        RequestState::InProgress => match command {
            RequestCommand::MarkTechnicalComplete => Ok(RequestState::TechnicalComplete),
            RequestCommand::Block { reason } => Ok(RequestState::Blocked {
                reason: reason.clone(),
            }),
            RequestCommand::Refuse { rationale } => Ok(RequestState::Refused {
                rationale: rationale.clone(),
            }),
            _ => refuse("in-progress"),
        },
        RequestState::Blocked { .. } => match command {
            RequestCommand::Resume => Ok(RequestState::InProgress),
            RequestCommand::Refuse { rationale } => Ok(RequestState::Refused {
                rationale: rationale.clone(),
            }),
            _ => refuse("blocked"),
        },
        RequestState::TechnicalComplete => match command {
            RequestCommand::AwaitOperator => Ok(RequestState::AwaitingOperator),
            _ => refuse("technically-complete"),
        },
        RequestState::AwaitingOperator => match command {
            RequestCommand::Finalize { outcome } => Ok(RequestState::Final(*outcome)),
            RequestCommand::Refuse { rationale } => Ok(RequestState::Refused {
                rationale: rationale.clone(),
            }),
            _ => refuse("awaiting-operator"),
        },
        RequestState::Final(_) => refuse("final"),
        RequestState::Refused { .. } => refuse("refused"),
    }
}
