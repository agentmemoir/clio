// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading the derivation record a wave leaves on
//! disk, so a harness assertion describes stored rows rather than anything a
//! tool reports about itself.
//!
//! ## Owns
//! - `Fixture::distilled_records`, the item/record join for a distilled bank
//! - `Fixture::stale_reason_of`, a unit's recorded stale reason read from the row
//! - `Fixture::distilled_evidence`, `Fixture::evidence_trail`, `Fixture::hub_of`,
//!   `Fixture::keep_only_unit`, and `Fixture::set_item_updated_at`: the stored
//!   evidence column read back as a trail, a unit's hub, a one-unit bank, and the
//!   one stamp a CLI verb cannot move
//!
//! ## Does not own
//! - The fixture and its store path (`consolidate_wave_support`)
//! - What any test asserts about the rows (the harness)
//!
//! ## Boundary
//!
//! Changes to how the record is read back for a harness assertion belong here.
//!
//! Test-only. Never compiled into production builds.

use super::Fixture;
use serde_json::Value;

impl Fixture {
    /// `(item_id, source_ref, hub_id, source_ids_json)` for every stored
    /// hub-distilled item, ordered by item id.
    ///
    /// The join is a left join, so a unit with no derivation record comes back
    /// with empty strings instead of disappearing — a missing record is exactly
    /// the regression this reader must be able to show.
    ///
    /// Read through a writable connection with `PRAGMA query_only`, for the same
    /// reason [`Fixture::rows`] gives: a wave this harness is still waiting on
    /// can end with a hot journal behind it, and a `SQLITE_OPEN_READ_ONLY`
    /// connection refuses to open rather than rolling that journal back.
    pub fn distilled_records(&self) -> Vec<(String, String, String, String)> {
        let conn = self.reading_connection();
        let mut stmt = conn
            .prepare(
                "select i.id, coalesce(i.source_ref, ''), coalesce(d.hub_id, ''),
                        coalesce(d.source_ids_json, '')
                 from items i
                 left join distilled_units d on d.bank_id = i.bank_id and d.item_id = i.id
                 where i.id like 'hub-distill:%'
                 order by i.id",
            )
            .expect("prepare the record join");
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("query the record join")
        .map(|row| row.expect("record row"))
        .collect()
    }

    /// Every stored item id in the fixture's single bank, sorted.
    pub fn stored_item_ids(&self) -> Vec<String> {
        self.rows("select id, 0 from items order by id")
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Ids of every stored hub-distilled item in one bank, sorted.
    ///
    /// A scenario that puts a hub in a second bank needs this scoped read to
    /// prove which bank a wave actually touched.
    pub fn distilled_ids_in(&self, bank: &str) -> Vec<String> {
        let escaped = bank.replace('\'', "''");
        self.rows(&format!(
            "select id, 0 from items where bank_id = '{escaped}' and id like 'hub-distill:%' order by id"
        ))
        .into_iter()
        .map(|(id, _)| id)
        .collect()
    }

    /// The `stale_reason` a unit's derivation record carries, or `""` when the
    /// record says it is current (the column is NULL then).
    ///
    /// Read from the row rather than from a tool's own report, because the claim
    /// being tested is about what recall will read: the retrieve path resolves a
    /// unit's lapse from this record, and a summary tool describing itself is not
    /// evidence of it.
    pub fn stale_reason_of(&self, unit: &str) -> String {
        let conn = self.reading_connection();
        conn.query_row(
            "select coalesce(d.stale_reason, '') from items i
             left join distilled_units d on d.bank_id = i.bank_id and d.item_id = i.id
             where i.id = ?1",
            [unit],
            |row| row.get::<_, String>(0),
        )
        .expect("query the record's stale reason")
    }

    /// The hub a unit's derivation record names.
    ///
    /// The evidence list cannot say which of its entries is the hub, and a
    /// scenario that acts on the wrong one exercises a different path: a
    /// withdrawn hub leaves its unit stale by design, while a withdrawn
    /// neighbour is one a wave rebuilds from what is left.
    pub fn hub_of(&self, unit: &str) -> String {
        self.distilled_records()
            .into_iter()
            .find(|(id, ..)| id == unit)
            .map_or_else(|| panic!("no record for {unit}"), |(_, _, hub, _)| hub)
    }

    /// A unit's stored evidence trail, parsed: one list per revision, oldest
    /// first.
    ///
    /// The parsed form is what a scenario asserts on, so it is resolved once
    /// here rather than re-derived in each harness.
    pub fn evidence_trail(&self, unit: &str) -> Vec<Value> {
        let stored = self.distilled_evidence();
        let raw = stored.iter().find(|(id, _)| id == unit).map_or_else(
            || panic!("no evidence column for {unit}: {stored:?}"),
            |(_, json)| json.clone(),
        );
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("the trail is a JSON array: {e}: {raw}"))
    }

    /// The stored `evidence_json` of every distilled unit, keyed by the unit's
    /// item id, in the same order `distilled_records` uses.
    ///
    /// The column is read straight from the row so an assertion is about stored
    /// state and not about what a tool reports about itself, which is the rule
    /// the rest of this reader already follows. A unit with no derivation record
    /// comes back with an empty string rather than disappearing: a missing record
    /// is exactly the regression a reader has to be able to show.
    pub fn distilled_evidence(&self) -> Vec<(String, String)> {
        let conn = self.reading_connection();
        let mut stmt = conn
            .prepare(
                "select i.id, coalesce(d.evidence_json, '')
                 from items i
                 left join distilled_units d on d.bank_id = i.bank_id and d.item_id = i.id
                 where i.id like 'hub-distill:%'
                 order by i.id",
            )
            .expect("prepare the evidence read");
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query the evidence read")
        .map(|row| row.expect("evidence row"))
        .collect()
    }

    /// Delete every distilled unit except `keep`, so a scenario that counts by
    /// reason, or that names one unit's state, is a statement about that unit
    /// and not about the rest of the bank.
    ///
    /// The item row and its derivation record go together in one transaction,
    /// which is exactly what the store's own delete path does: no CLI verb
    /// exposes that path, and leaving a record behind would make the deleted
    /// unit's sources look removed to every other unit that claimed them.
    pub fn keep_only_unit(&self, keep: &str) {
        let mut conn = rusqlite::Connection::open(&self.db).expect("open the store to write");
        let tx = conn.transaction().expect("begin the delete transaction");
        for id in self.distilled_ids() {
            if id != keep {
                tx.execute("delete from items where id = ?1", [&id])
                    .expect("delete the unit's item row");
                tx.execute("delete from distilled_units where item_id = ?1", [&id])
                    .expect("delete the unit's record");
            }
        }
        tx.commit().expect("commit the delete transaction");
        let left = self.distilled_ids();
        assert_eq!(left, vec![keep.to_owned()], "only {keep} is left");
    }

    /// Move one item's stored last-change time to `stamp`, as a later write
    /// would, so a freshness assertion is a fact about the fixture rather than
    /// about when the suite runs.
    ///
    /// The CLI cannot do this: every `remember` is a new item id, so no CLI verb
    /// rewrites an existing row's `updated_at`. The stamp is a literal the caller
    /// chooses, never a computed "now", which is what makes the comparison the
    /// store performs deterministic on any machine and any date.
    pub fn set_item_updated_at(&self, item_id: &str, stamp: &str) {
        let conn = rusqlite::Connection::open(&self.db).expect("open the store to write");
        let changed = conn
            .execute(
                "update items set updated_at = ?1 where id = ?2",
                rusqlite::params![stamp, item_id],
            )
            .expect("update the stored stamp");
        assert_eq!(changed, 1, "the item must exist: {item_id}");
    }
}
