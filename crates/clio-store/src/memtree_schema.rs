// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reshaping `memtree_nodes` when columns are stale.
//!
//! ## Owns
//! - Detecting missing MemTree interval / child-list columns
//! - Dropping empty stale `memtree_nodes` so portable DDL can recreate it
//!
//! ## Does not own
//! - MemTree attach / dirty-path algorithms
//! - Item CRUD (`sqlite_memory` / `postgres_memory`)
//!
//! ## Boundary
//!
//! Changes to MemTree DDL reshape should normally require changes here.
//! Changes to item schema should not require changes here.
//!
//! Keep this module focused on MemTree table shape repair at open time.

use crate::migrate::IntoAmError;
use clio_types::AmError;

const MEMTREE_DDL: &str = r"
CREATE TABLE IF NOT EXISTS memtree_nodes
(
  id text PRIMARY KEY,
  bank_id text NOT NULL,
  parent_id text,
  item_id text,
  depth integer NOT NULL DEFAULT 0,
  interval_start text NOT NULL,
  interval_end text NOT NULL,
  child_ids_json text NOT NULL DEFAULT '[]',
  summary_gist text,
  dirty integer NOT NULL DEFAULT 0 CHECK (dirty IN (0, 1)),
  generation integer NOT NULL DEFAULT 0,
  refreshing integer NOT NULL DEFAULT 0 CHECK (refreshing IN (0, 1)),
  task_id text,
  last_error text,
  created_at text NOT NULL,
  updated_at text NOT NULL
);
CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_parent_id_inx
ON memtree_nodes (bank_id, parent_id);
CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_dirty_inx
ON memtree_nodes (bank_id) WHERE dirty = 1;
CREATE INDEX IF NOT EXISTS memtree_nodes_item_id_inx
ON memtree_nodes (item_id);
CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_interval_inx
ON memtree_nodes (bank_id, interval_start, interval_end);
";

fn sqlite_has_interval(conn: &rusqlite::Connection) -> Result<bool, AmError> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'memtree_nodes'",
            [],
            |r| r.get(0),
        )
        .am("memtree exists")?;
    if exists == 0 {
        return Ok(true);
    }
    let mut stmt = conn
        .prepare("PRAGMA table_info(memtree_nodes)")
        .am("pragma memtree")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .am("memtree cols")?;
    for name in names {
        if name.am("col")? == "interval_start" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Drop stale `memtree_nodes` (no interval columns) and recreate current DDL.
pub(crate) fn ensure_memtree_shape_sqlite(conn: &rusqlite::Connection) -> Result<(), AmError> {
    if sqlite_has_interval(conn)? {
        return Ok(());
    }
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memtree_nodes", [], |r| r.get(0))
        .am("memtree count")?;
    if rows > 0 {
        return Err(AmError::new(
            clio_types::ErrorCode::ConfigCorrupt,
            format!("refusing to wipe memtree_nodes holding {rows} row(s)"),
        ));
    }
    conn.execute_batch("DROP TABLE IF EXISTS memtree_nodes;")
        .am("drop stale memtree")?;
    conn.execute_batch(MEMTREE_DDL).am("recreate memtree")?;
    Ok(())
}

fn pg_has_interval(client: &mut postgres::Client) -> Result<bool, AmError> {
    let exists: bool = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'memtree_nodes'
             )",
            &[],
        )
        .am("pg memtree exists")?
        .get(0);
    if !exists {
        return Ok(true);
    }
    let has: bool = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public' AND table_name = 'memtree_nodes'
                  AND column_name = 'interval_start'
             )",
            &[],
        )
        .am("pg memtree col")?
        .get(0);
    Ok(has)
}

/// Drop stale Postgres `memtree_nodes` and recreate current DDL.
pub(crate) fn ensure_memtree_shape_pg(client: &mut postgres::Client) -> Result<(), AmError> {
    if pg_has_interval(client)? {
        return Ok(());
    }
    let rows: i64 = client
        .query_one("SELECT COUNT(*) FROM public.memtree_nodes", &[])
        .am("pg memtree count")?
        .get(0);
    if rows > 0 {
        return Err(AmError::new(
            clio_types::ErrorCode::ConfigCorrupt,
            format!("refusing to wipe public.memtree_nodes holding {rows} row(s)"),
        ));
    }
    client
        .batch_execute("DROP TABLE IF EXISTS public.memtree_nodes;")
        .am("drop stale pg memtree")?;
    client
        .batch_execute(MEMTREE_DDL)
        .am("recreate pg memtree")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_sqlite_needs_no_reshape() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(MEMTREE_DDL).unwrap();
        ensure_memtree_shape_sqlite(&conn).unwrap();
        assert!(sqlite_has_interval(&conn).unwrap());
    }

    #[test]
    fn stale_empty_sqlite_memtree_is_recreated() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memtree_nodes (
               id text PRIMARY KEY,
               bank_id text NOT NULL,
               parent_id text,
               item_id text,
               depth integer NOT NULL DEFAULT 0,
               summary_gist text,
               dirty integer NOT NULL DEFAULT 0,
               created_at text NOT NULL,
               updated_at text NOT NULL
             );",
        )
        .unwrap();
        ensure_memtree_shape_sqlite(&conn).unwrap();
        assert!(sqlite_has_interval(&conn).unwrap());
    }

    #[test]
    fn refuse_wipe_when_stale_sqlite_has_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memtree_nodes (
               id text PRIMARY KEY,
               bank_id text NOT NULL,
               parent_id text,
               item_id text,
               depth integer NOT NULL DEFAULT 0,
               summary_gist text,
               dirty integer NOT NULL DEFAULT 0,
               created_at text NOT NULL,
               updated_at text NOT NULL
             );
             INSERT INTO memtree_nodes (id, bank_id, depth, dirty, created_at, updated_at)
             VALUES ('n', 'b', 0, 0, 't', 't');",
        )
        .unwrap();
        let err = ensure_memtree_shape_sqlite(&conn).unwrap_err();
        assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    }

    #[test]
    fn sqlite_open_does_not_stamp_schema_version_when_reshape_refuses() {
        let path = std::env::temp_dir().join(format!("am-memtree-guard-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE memtree_nodes (
                   id text PRIMARY KEY,
                   bank_id text NOT NULL,
                   parent_id text,
                   item_id text,
                   depth integer NOT NULL DEFAULT 0,
                   summary_gist text,
                   dirty integer NOT NULL DEFAULT 0,
                   created_at text NOT NULL,
                   updated_at text NOT NULL
                 );
                 INSERT INTO memtree_nodes (id, bank_id, depth, dirty, created_at, updated_at)
                 VALUES ('n', 'b', 0, 0, 't', 't');",
            )
            .unwrap();
        }
        let Err(err) = crate::sqlite::SqliteStore::open(
            &path,
            std::sync::Arc::new(crate::dek::LocalDevKms::new()),
        ) else {
            panic!("open must refuse stale memtree with rows");
        };
        assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
        // Core DDL never ran, so schema_settings (and the version stamp) is absent.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            tables, 0,
            "schema_version must not be stamped before memtree reshape passes"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn postgres_memtree_shape_ok_on_fresh_open() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let mut client = crate::pg_tls::connect(&url).unwrap();
        ensure_memtree_shape_pg(&mut client).unwrap();
        assert!(pg_has_interval(&mut client).unwrap());
    }

    #[test]
    fn postgres_stale_empty_memtree_recreated() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let mut client = crate::pg_tls::connect(&url).unwrap();
        client
            .batch_execute(
                "DROP TABLE IF EXISTS public.memtree_nodes;
                 CREATE TABLE public.memtree_nodes (
                   id text PRIMARY KEY,
                   bank_id text NOT NULL,
                   parent_id text,
                   item_id text,
                   depth integer NOT NULL DEFAULT 0,
                   summary_gist text,
                   dirty integer NOT NULL DEFAULT 0,
                   created_at text NOT NULL,
                   updated_at text NOT NULL
                 );",
            )
            .unwrap();
        ensure_memtree_shape_pg(&mut client).unwrap();
        assert!(pg_has_interval(&mut client).unwrap());
    }

    #[test]
    fn postgres_refuse_wipe_with_rows() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let mut client = crate::pg_tls::connect(&url).unwrap();
        client
            .batch_execute(
                "DROP TABLE IF EXISTS public.memtree_nodes;
                 CREATE TABLE public.memtree_nodes (
                   id text PRIMARY KEY,
                   bank_id text NOT NULL,
                   parent_id text,
                   item_id text,
                   depth integer NOT NULL DEFAULT 0,
                   summary_gist text,
                   dirty integer NOT NULL DEFAULT 0,
                   created_at text NOT NULL,
                   updated_at text NOT NULL
                 );
                 INSERT INTO public.memtree_nodes (id, bank_id, depth, dirty, created_at, updated_at)
                 VALUES ('n', 'b', 0, 0, 't', 't');",
            )
            .unwrap();
        let err = ensure_memtree_shape_pg(&mut client).unwrap_err();
        assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
        // Restore proper shape for other tests sharing the DB.
        client
            .batch_execute("DROP TABLE IF EXISTS public.memtree_nodes;")
            .unwrap();
        ensure_memtree_shape_pg(&mut client).unwrap();
    }
}
