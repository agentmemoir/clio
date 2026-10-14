// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-request contract: the immutable
//! scope binding, the request state table, the total transition function, and
//! the local command contracts over an in-memory request ledger.
//!
//! ## Owns
//! - `RequestScope` bank and subject scope definitions
//! - `RequestState`, `FinalOutcome`, `StateCategory`, and the state table
//! - `RequestCommand` and the total, idempotent `transition` function
//! - The `SubjectRequestLedger` register/apply contracts, including the
//!   id-kind-scope binding
//!
//! ## Does not own
//! - The durable record and its frozen field vocabulary
//!   (`subject_request_frozen.rs`)
//! - Any storage adapter, SQL, or database access: the ledger is in-memory
//!   only, and reloading records models a restart
//! - Any CLI or I/O: commands are data applied by functions
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and refusal rationales and block reasons must stay generic
//!
//! ## Boundary
//!
//! Changes to the request states, the scope binding, or command semantics
//! should normally require changes here. Changes to how a request is stored on
//! disk or surfaced by a tool should not require changes here.
//!
//! Keep this module focused on the pure request state contract.

use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::subject_request_frozen::{
    PathOwner, PathSlot, PathState, PeerState, RequestAction, RequestActionKind,
    RequestActionState, RequestKind, SubjectRequestRecord, VerificationResult,
};

/// Where a request applies, fixed when the request is opened.
///
/// The scope of a stored request never changes; see
/// [`SubjectRequestLedger::register`] for the binding rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestScope {
    /// The whole bank: every subject the bank holds.
    Bank {
        /// The bank the request covers.
        bank_id: String,
    },
    /// One subject inside a bank.
    Subject {
        /// The bank the subject lives in.
        bank_id: String,
        /// The subject the request covers.
        subject_id: String,
    },
}

impl RequestScope {
    /// The contract scope check, shared with the action-event validator so no
    /// event carries a scope the contract would refuse.
    pub(crate) fn validate(&self) -> Result<(), AmError> {
        match self {
            Self::Bank { bank_id } => Self::validate_bank(bank_id),
            Self::Subject {
                bank_id,
                subject_id,
            } => {
                Self::validate_bank(bank_id)?;
                if subject_id.trim().is_empty() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "subject scope needs a subject id",
                    ));
                }
                Ok(())
            }
        }
    }

    fn validate_bank(bank_id: &str) -> Result<(), AmError> {
        if bank_id.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "request scope needs a bank id",
            ));
        }
        Ok(())
    }
}

/// The final outcome of finished work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalOutcome {
    /// Every covered path is done.
    Completed,
    /// Some covered paths are done and some could not be.
    PartiallyCompleted,
}

/// The state of one request. Final and refused states are terminal and never
/// move again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestState {
    /// Opened; no work has started.
    Received,
    /// Work has started.
    InProgress,
    /// The technical work is done; the outcome is not final yet.
    TechnicalComplete,
    /// The operator must act before the outcome is final.
    AwaitingOperator,
    /// An exception stopped the work.
    Blocked {
        /// What stopped the work; must stay generic.
        reason: String,
    },
    /// A final outcome was recorded.
    Final(FinalOutcome),
    /// The request was denied.
    Refused {
        /// Why the request was denied; must stay generic.
        rationale: String,
    },
}

/// Which kind of state a request is in. The state table separates working
/// states, technical completion, operator action, exceptions, and the final
/// outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateCategory {
    /// Work is open or running.
    Working,
    /// The technical work is done.
    TechnicalCompletion,
    /// An operator must act.
    OperatorAction,
    /// An exception stopped the work.
    Exception,
    /// A terminal outcome was recorded.
    FinalOutcome,
}

impl RequestState {
    /// Every state, in table order. Tests consume this so a new state fails
    /// the completeness guard instead of passing silently.
    #[must_use]
    pub fn all() -> [Self; 8] {
        [
            Self::Received,
            Self::InProgress,
            Self::TechnicalComplete,
            Self::AwaitingOperator,
            Self::Blocked {
                reason: "blocked".into(),
            },
            Self::Final(FinalOutcome::Completed),
            Self::Final(FinalOutcome::PartiallyCompleted),
            Self::Refused {
                rationale: "refused".into(),
            },
        ]
    }

    /// The category this state belongs to.
    #[must_use]
    pub const fn category(&self) -> StateCategory {
        match self {
            Self::Received | Self::InProgress => StateCategory::Working,
            Self::TechnicalComplete => StateCategory::TechnicalCompletion,
            Self::AwaitingOperator => StateCategory::OperatorAction,
            Self::Blocked { .. } => StateCategory::Exception,
            Self::Final(_) | Self::Refused { .. } => StateCategory::FinalOutcome,
        }
    }
}

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

/// What [`SubjectRequestLedger::register`] did with a request id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// The id was new: a `Received` record was created.
    Opened,
    /// The id and scope matched a stored record: its state was replayed and
    /// nothing was applied.
    Replayed {
        /// The stored state that was replayed.
        state: RequestState,
    },
}

/// In-memory ledger of durable request records.
///
/// The ledger owns no persistence: a caller reloads
/// [`SubjectRequestRecord`]s from its store and calls [`Self::load`] to model
/// a restart.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SubjectRequestLedger {
    requests: BTreeMap<String, SubjectRequestRecord>,
}

impl SubjectRequestLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a ledger from records reloaded after a restart. Stored records
    /// are trusted here; a duplicated id keeps the last record, silently
    /// dropping the earlier one.
    #[must_use]
    pub fn load(records: Vec<SubjectRequestRecord>) -> Self {
        Self {
            requests: records
                .into_iter()
                .map(|record| (record.request_id.clone(), record))
                .collect(),
        }
    }

    /// Every stored record, in request-id order.
    #[must_use]
    pub fn records(&self) -> Vec<SubjectRequestRecord> {
        self.requests.values().cloned().collect()
    }

    /// The stored record for a request id, if any.
    #[must_use]
    pub fn record(&self, request_id: &str) -> Option<&SubjectRequestRecord> {
        self.requests.get(request_id)
    }

    /// Open a request, or replay its stored result.
    ///
    /// The request id is durable and its kind and scope are immutable: the
    /// same id with the same kind and scope replays the stored record and
    /// applies nothing, ignoring the incoming peer list entirely; the same id
    /// with a changed kind or scope is refused and no state changes. The peer
    /// snapshot is captured once, at open; a replay keeps the stored snapshot.
    pub fn register(
        &mut self,
        request_id: &str,
        kind: RequestKind,
        scope: &RequestScope,
        peers: &[&str],
    ) -> Result<RegisterOutcome, AmError> {
        if request_id.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "request id must not be blank",
            ));
        }
        scope.validate()?;
        match self.requests.get(request_id) {
            Some(stored) if stored.kind == kind && stored.scope == *scope => {
                Ok(RegisterOutcome::Replayed {
                    state: stored.state.clone(),
                })
            }
            Some(_) => Err(AmError::new(
                ErrorCode::Forbidden,
                "request id is already bound to a different kind or scope",
            )),
            None => {
                let peer_slots = peer_snapshot(peers)?;
                self.requests.insert(
                    request_id.to_owned(),
                    SubjectRequestRecord {
                        request_id: request_id.to_owned(),
                        kind,
                        scope: scope.clone(),
                        state: RequestState::Received,
                        verification: None,
                        deadline: None,
                        actions: BTreeMap::new(),
                        paths: BTreeMap::new(),
                        peers: peer_slots,
                        exception: None,
                    },
                );
                Ok(RegisterOutcome::Opened)
            }
        }
    }

    /// Apply a command to the stored request and record the next state.
    ///
    /// Marking technical completion refuses while a copy action is still
    /// open; product-path and retention-exception gating stays in the closure
    /// seam. A refused command changes nothing.
    pub fn apply(
        &mut self,
        request_id: &str,
        command: &RequestCommand,
    ) -> Result<RequestState, AmError> {
        let Some(record) = self.requests.get_mut(request_id) else {
            return Err(AmError::new(
                ErrorCode::NotFound,
                format!("no request {request_id}"),
            ));
        };
        if matches!(command, RequestCommand::MarkTechnicalComplete)
            && record.actions.values().any(|action| {
                action.kind == RequestActionKind::Copy && action.state == RequestActionState::Open
            })
        {
            return Err(AmError::new(
                ErrorCode::Forbidden,
                "an open copy action blocks technical completion",
            ));
        }
        let next = transition(&record.state, command)?;
        record.state = next.clone();
        Ok(next)
    }
}

/// One pending slot per peer of a new request; duplicates collapse, and a
/// blank id is refused.
fn peer_snapshot(peers: &[&str]) -> Result<BTreeMap<String, PeerState>, AmError> {
    let mut slots = BTreeMap::new();
    for peer_id in peers {
        if peer_id.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "peer snapshot needs a peer id",
            ));
        }
        slots.insert((*peer_id).to_owned(), PeerState::Pending);
    }
    Ok(slots)
}

#[cfg(test)]
#[path = "subject_request_register_tests.rs"]
mod subject_request_register_tests;

#[cfg(test)]
#[path = "subject_request_tests.rs"]
mod subject_request_tests;
