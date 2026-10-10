// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres belief-object repository.
//!
//! ## Owns
//! - Postgres create / append / get / history SQL for beliefs, including the
//!   sealed optional `context` read and replacement and the `context_as_of`
//!   effective-time read mapping
//!
//! ## Does not own
//! - The `BeliefStore` contract (see `belief_store`)
//! - Admission scoring or tool orchestration (`clio-belief`)
//!
//! ## Boundary
//!
//! Changes to Postgres belief SQL should normally require changes here.
//! Changes to SQLite belief SQL should not require changes here.
//!
//! Keep this module focused on Postgres belief SQL.

use crate::belief_store::{
    BeliefStore, open_belief_context, open_proposition, seal_belief_context, seal_proposition,
    validate_context_arg,
};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::{AmError, BeliefObject, ConfidenceEntry, ErrorCode, SourceType, proposition_key};

impl PostgresStore {
    pub(crate) fn create_belief_impl(
        &self,
        belief: &BeliefObject,
        actor: &str,
    ) -> Result<(), AmError> {
        crate::belief_tx::validate_belief_create(belief, actor)?;
        let dek_ref = self.kms().ensure_dek(&belief.subject_id)?;
        let cipher =
            seal_proposition(self.kms().as_ref(), &belief.subject_id, &belief.proposition)?;
        let context_cipher = seal_belief_context(
            self.kms().as_ref(),
            &belief.subject_id,
            belief.context.as_deref(),
        )?;
        let key = proposition_key(&belief.proposition);
        self.with_client(|client| {
            let mut tx = client.transaction().am("belief create begin")?;
            crate::belief_tx::create_belief_postgres_tx(
                &mut tx,
                belief,
                &cipher,
                context_cipher.as_deref(),
                &dek_ref,
                &key,
                actor,
            )?;
            tx.commit().am("belief create commit")?;
            Ok(())
        })
    }

    pub(crate) fn append_confidence_impl(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
    ) -> Result<(), AmError> {
        self.append_confidence_with_context_impl(bank_id, belief_id, entry, actor, None)
    }

    /// Append with an optional context observation: `None` preserves the stored
    /// context; `Some(context)` must be byte-identical to the stored value
    /// So the cipher is never replaced here. A differing value fails
    /// closed before any write; the authorized replacement lives in
    /// `correct_belief_context`.
    pub(crate) fn append_confidence_with_context_impl(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), AmError> {
        crate::belief_tx::validate_append(bank_id, entry, actor)?;
        validate_context_arg(context)?;
        let context_stats = context.map(|c| crate::belief_store::belief_context_stats(Some(c)));
        self.with_client(|client| {
            let mut tx = client.transaction().am("belief append begin")?;
            if let Some(context) = context {
                let stored: Option<Option<String>> = tx
                    .query_opt(
                        "SELECT context_cipher FROM beliefs WHERE id = $1 AND bank_id = $2",
                        &[&belief_id, &bank_id],
                    )
                    .am("belief context lookup")?
                    .map(|r| r.get(0));
                let stored = stored.ok_or_else(|| crate::belief_tx::belief_not_found(belief_id))?;
                if open_belief_context(self.kms().as_ref(), stored.as_deref()).as_deref()
                    != Some(context)
                {
                    return Err(crate::belief_store::context_change_requires_correction());
                }
            }
            crate::belief_tx::append_confidence_postgres_tx(
                &mut tx,
                bank_id,
                belief_id,
                entry,
                actor,
                None,
                context_stats.as_ref(),
            )?;
            tx.commit().am("belief append commit")?;
            Ok(())
        })
    }

    pub(crate) fn get_belief_impl(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<BeliefObject>, AmError> {
        let row = self.with_client(|client| {
            client
                .query_opt(
                    "SELECT id, bank_id, subject_id, proposition_ciphertext, context_cipher,
                         created_at, context_as_of
                     FROM beliefs WHERE id = $1 AND bank_id = $2",
                    &[&belief_id, &bank_id],
                )
                .am("beliefs get")
        })?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = row.get(0);
        let bank_id: String = row.get(1);
        let subject_id: String = row.get(2);
        let cipher: String = row.get(3);
        let context_cipher: Option<String> = row.get(4);
        let created_at: String = row.get(5);
        let context_as_of: Option<String> = row.get(6);
        let proposition = open_proposition(self.kms().as_ref(), &cipher)?;
        let context = open_belief_context(self.kms().as_ref(), context_cipher.as_deref());
        let history = self.belief_history_impl(&bank_id, &id, None)?;
        let updated_at = history
            .last()
            .expect("stored beliefs always keep at least one entry")
            .as_of
            .clone();
        Ok(Some(BeliefObject {
            id,
            bank_id,
            subject_id,
            proposition,
            context,
            context_as_of,
            confidence_history: history,
            created_at,
            updated_at,
        }))
    }

    pub(crate) fn find_belief_by_proposition_impl(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<BeliefObject>, AmError> {
        let id: Option<String> = self
            .with_client(|client| {
                client
                    .query_opt(
                        "SELECT id FROM beliefs WHERE bank_id = $1 AND proposition_key = $2",
                        &[&bank_id, &key],
                    )
                    .am("belief by proposition")
            })?
            .map(|r| r.get(0));
        match id {
            Some(id) => self.get_belief_impl(bank_id, &id),
            None => Ok(None),
        }
    }

    pub(crate) fn list_beliefs_impl(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError> {
        if bank_id.is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        let ids: Vec<String> = self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT id FROM beliefs WHERE bank_id = $1 ORDER BY id ASC",
                    &[&bank_id],
                )
                .am("beliefs list")?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })?;
        let mut out = Vec::new();
        for id in &ids {
            if let Some(belief) = self.get_belief_impl(bank_id, id)? {
                out.push(belief);
            }
        }
        Ok(out)
    }

    pub(crate) fn belief_history_impl(
        &self,
        bank_id: &str,
        belief_id: &str,
        as_of: Option<&str>,
    ) -> Result<Vec<ConfidenceEntry>, AmError> {
        let rows = self.with_client(|client| {
            client
                .query(
                    "SELECT e.as_of, e.confidence::float8, e.evidence_ref, e.source_type
                     FROM belief_confidence_entries e
                     JOIN beliefs b ON b.id = e.belief_id
                     WHERE b.id = $1 AND b.bank_id = $2
                       AND ($3::text IS NULL OR e.as_of <= $3)
                     ORDER BY e.as_of, e.id",
                    &[&belief_id, &bank_id, &as_of],
                )
                .am("belief history query")
        })?;
        let mut out = Vec::new();
        for row in rows {
            let as_of: String = row.get(0);
            let confidence: f64 = row.get(1);
            let evidence_ref: String = row.get(2);
            let source: String = row.get(3);
            out.push(ConfidenceEntry {
                as_of,
                confidence,
                evidence_ref,
                source_type: SourceType::parse(&source)?,
            });
        }
        Ok(out)
    }

    pub(crate) fn inspect_belief_ciphertext_impl(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<String>, AmError> {
        Ok(self
            .with_client(|client| {
                client
                    .query_opt(
                        "SELECT proposition_ciphertext FROM beliefs
                         WHERE id = $1 AND bank_id = $2",
                        &[&belief_id, &bank_id],
                    )
                    .am("belief ciphertext inspect")
            })?
            .map(|r| r.get(0)))
    }
}

impl BeliefStore for PostgresStore {
    fn create_belief(&self, belief: &BeliefObject, actor: &str) -> Result<(), AmError> {
        self.create_belief_impl(belief, actor)
    }

    fn append_confidence(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
    ) -> Result<(), AmError> {
        self.append_confidence_impl(bank_id, belief_id, entry, actor)
    }

    fn append_confidence_with_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), AmError> {
        self.append_confidence_with_context_impl(bank_id, belief_id, entry, actor, context)
    }

    fn get_belief(&self, bank_id: &str, belief_id: &str) -> Result<Option<BeliefObject>, AmError> {
        self.get_belief_impl(bank_id, belief_id)
    }

    fn find_belief_by_proposition(
        &self,
        bank_id: &str,
        proposition_key: &str,
    ) -> Result<Option<BeliefObject>, AmError> {
        self.find_belief_by_proposition_impl(bank_id, proposition_key)
    }

    fn belief_history(
        &self,
        bank_id: &str,
        belief_id: &str,
        as_of: Option<&str>,
    ) -> Result<Vec<ConfidenceEntry>, AmError> {
        self.belief_history_impl(bank_id, belief_id, as_of)
    }

    fn inspect_belief_ciphertext(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<String>, AmError> {
        self.inspect_belief_ciphertext_impl(bank_id, belief_id)
    }

    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError> {
        self.list_beliefs_impl(bank_id)
    }
}

#[cfg(test)]
#[path = "postgres_belief_context_tests.rs"]
pub(crate) mod postgres_belief_context_tests;
