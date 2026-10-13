// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the local compliance slot rows of the
//! scope ledger: one planned row per slot table the local store always
//! reports.
//!
//! ## Owns
//! - The eight slot-table `RowPlan` rows and their report order
//!
//! ## Does not own
//! - The fixed `ROW_PLAN` table and its overall report order (`inventory_plan`)
//! - The `RowPlan` schema and plan-to-store matching (`inventory_rows`)
//!
//! ## Boundary
//!
//! Changes to which slot families the ledger reports, or to what is known
//! about a slot family, should normally require changes here. Changes to the
//! row schema or to how the plan is matched against a live store should not.
//!
//! Keep this module focused on the slot-table rows.

use super::super::RowPlan;
use crate::inventory::{Custody, Owner, Readability, RetentionAction, RowSource};

pub(crate) const ARCHIVE_EXPERIMENTS: RowPlan = RowPlan {
    family: "archive_experiments",
    tables: &["archive_experiments"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the archive experiment slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Retain,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const COPY_INVENTORY: RowPlan = RowPlan {
    family: "copy_inventory",
    tables: &["copy_inventory"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the copy inventory slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Record,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const DURABLE_REQUESTS: RowPlan = RowPlan {
    family: "durable_requests",
    tables: &["durable_requests"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the durable request slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Erase,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const ERASURE_ADAPTERS: RowPlan = RowPlan {
    family: "erasure_adapters",
    tables: &["erasure_adapters"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the erasure adapter slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Erase,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const LOCAL_ERASURE_JOURNAL: RowPlan = RowPlan {
    family: "local_erasure_journal",
    tables: &["local_erasure_journal"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the local erasure journal slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Retain,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const LOCAL_EVENTS: RowPlan = RowPlan {
    family: "local_events",
    tables: &["local_events"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the local event slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Retain,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const RESTORE_GUARDS: RowPlan = RowPlan {
    family: "restore_guards",
    tables: &["restore_guards"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the restore guard slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Retain,
    proof: "sql/003_local_compliance_slots.sql",
};

pub(crate) const SUBJECT_FAMILY_LINKS: RowPlan = RowPlan {
    family: "subject_family_links",
    tables: &["subject_family_links"],
    source: RowSource::Tables,
    link: None,
    gap: Some("no subject column: the subject-family link slot carries an id only"),
    owner: Owner::Clio,
    custody: Custody::Clio,
    readability: Readability::Plaintext,
    key_path: None,
    action: RetentionAction::Erase,
    proof: "sql/003_local_compliance_slots.sql",
};
