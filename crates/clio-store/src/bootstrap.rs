// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Open a store from effective config / connection options.

use crate::dek::DekProvider;
use crate::embedding_space::EmbeddingSpace;
use crate::postgres::PostgresStore;
use crate::space_reconcile::SpaceReconcile;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Which driver was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreBackend {
    /// SQLite + sqlite-vec.
    Sqlite,
    /// PostgreSQL + pgvector.
    Postgres,
}

/// Connection options resolved from config + environment.
#[derive(Debug, Clone)]
pub struct StoreOpenOptions {
    /// `sqlite` or `postgres`.
    pub backend: StoreBackend,
    /// Postgres URL or `sqlite:` / filesystem path for SQLite.
    pub database_url: String,
    /// Optional explicit SQLite filesystem path (wins over URL path parsing).
    pub sqlite_path: Option<PathBuf>,
    /// Configured active embedding space, when `embed.model`/`embed.dims` are set.
    pub embed_space: Option<EmbeddingSpace>,
    /// Explicit opt-in (`embed.switch_space`) allowing a destructive space switch.
    pub confirm_space_switch: bool,
}

impl StoreOpenOptions {
    /// Build options from an effective config JSON document plus env lookup.
    ///
    /// # Errors
    /// Unknown backend, missing connection target, or a half-configured embedding space.
    pub fn from_effective(
        effective: &Value,
        env: &mut dyn FnMut(&str) -> Option<String>,
    ) -> Result<Self, AmError> {
        let backend_raw = effective
            .pointer("/backend")
            .and_then(Value::as_str)
            .unwrap_or("sqlite");
        let backend = match backend_raw {
            "sqlite" => StoreBackend::Sqlite,
            "postgres" => StoreBackend::Postgres,
            other => {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("backend must be sqlite|postgres, got `{other}`"),
                ));
            }
        };
        let database_url = effective
            .pointer("/store/database_url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| clio_config::db_url_env::database_url_from(&mut *env))
            .unwrap_or_default();
        let sqlite_path = effective
            .pointer("/store/sqlite_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        if backend == StoreBackend::Postgres && database_url.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "postgres backend requires store.database_url or {}",
                    clio_config::db_url_env::DATABASE_URL_ENV
                ),
            ));
        }
        let embed_space = crate::space_reconcile::configured_embedding_space(effective)?;
        let confirm_space_switch = crate::space_reconcile::space_switch_confirmed(effective);
        Ok(Self {
            backend,
            database_url,
            sqlite_path,
            embed_space,
            confirm_space_switch,
        })
    }

    fn resolve_sqlite_path(&self) -> PathBuf {
        if let Some(path) = &self.sqlite_path {
            return path.clone();
        }
        if let Some(rest) = self.database_url.strip_prefix("sqlite:") {
            return PathBuf::from(rest);
        }
        if !self.database_url.is_empty() && !self.database_url.starts_with("postgres") {
            return PathBuf::from(&self.database_url);
        }
        PathBuf::from("./data/clio.db")
    }
}

/// Open the configured backend. Returns a trait object implementing [`Store`].
///
/// When the effective config selects an embedding space that differs from the
/// recorded one, the store reconciles it: an opt-in (`embed.switch_space`) is
/// required because the switch purges the derived dense index.
///
/// # Errors
/// Config, migration, driver, or refused-space-switch failures.
pub fn open_store(
    options: &StoreOpenOptions,
    kms: Arc<dyn DekProvider>,
) -> Result<(StoreBackend, Arc<dyn Store>), AmError> {
    match options.backend {
        StoreBackend::Sqlite => {
            let path = options.resolve_sqlite_path();
            // `:memory:` is a valid SQLite path; keep one open() path for coverage.
            let mut store = SqliteStore::open(Path::new(&path), kms)?;
            store.reconcile_space(options.embed_space.as_ref(), options.confirm_space_switch)?;
            Ok((StoreBackend::Sqlite, Arc::new(store)))
        }
        StoreBackend::Postgres => {
            let mut store = PostgresStore::open(&options.database_url, kms)?;
            store.reconcile_space(options.embed_space.as_ref(), options.confirm_space_switch)?;
            Ok((StoreBackend::Postgres, Arc::new(store)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::LocalDevKms;
    use serde_json::json;

    #[test]
    fn resolves_sqlite_defaults_and_opens() {
        let path = std::env::temp_dir().join(format!(
            "am-store-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(1)
        ));
        let effective = json!({
            "backend": "sqlite",
            "store": {"sqlite_path": path.to_str().unwrap()}
        });
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        assert_eq!(opts.backend, StoreBackend::Sqlite);
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let (backend, store) = open_store(&opts, kms).unwrap();
        assert_eq!(backend, StoreBackend::Sqlite);
        let _ = store;
        let _ = std::fs::remove_file(&path);
    }

    /// The refusal a Postgres backend with no address of any kind produces.
    ///
    /// Built by the real path so a test that asserts on the wording is asserting
    /// on the returned error rather than on a restatement of it.
    fn missing_url_refusal() -> AmError {
        let effective = json!({"backend": "postgres"});
        let mut env = |_k: &str| None;
        StoreOpenOptions::from_effective(&effective, &mut env)
            .expect_err("a postgres backend with no address is refused")
    }

    #[test]
    fn postgres_requires_url() {
        let err = missing_url_refusal();
        assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    }

    #[test]
    fn the_missing_url_refusal_names_the_store_field() {
        // The refusal leads with the setting, so an operator who set it and still
        // sees this knows the value is not reaching the store.
        let message = missing_url_refusal().message;
        assert!(
            message.starts_with("postgres backend requires store.database_url or "),
            "unexpected refusal: {message}"
        );
    }

    #[test]
    fn the_missing_url_refusal_names_the_prefixed_variable() {
        // The refusal names the variable it looked for, so an operator who exported
        // the other spelling learns which one is read.
        let message = missing_url_refusal().message;
        assert!(
            message.contains(clio_config::db_url_env::DATABASE_URL_ENV),
            "the refusal must name the prefixed variable: {message}"
        );
    }

    #[test]
    fn rejects_unknown_backend() {
        let effective = json!({"backend": "redis"});
        let mut env = |_k: &str| None;
        assert!(StoreOpenOptions::from_effective(&effective, &mut env).is_err());
    }

    /// The unprefixed spelling is not a second name for the same variable, so a
    /// shell that exports only it leaves the store with no address at all. The
    /// string below is deliberately the name that is *not* read: it is the input
    /// to this negative case, not a lookup the production path performs.
    #[test]
    fn the_bare_database_url_name_is_not_read() {
        let effective = json!({"backend": "sqlite"});
        let mut env = |k: &str| {
            if k == "DATABASE_URL" {
                Some("postgres://bare/db".into())
            } else {
                None
            }
        };
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        assert!(
            opts.database_url.is_empty(),
            "the bare name must not reach the store, got {}",
            opts.database_url
        );
        assert_eq!(opts.resolve_sqlite_path(), PathBuf::from("./data/clio.db"));
    }

    /// The prefixed name is the only one read, and it lands on the same field a
    /// config-file address would.
    #[test]
    fn the_prefixed_database_url_name_reaches_the_store() {
        let effective = json!({"backend": "sqlite"});
        let mut env = |k: &str| {
            if k == clio_config::db_url_env::DATABASE_URL_ENV {
                Some("sqlite:/tmp/from-the-owner.db".into())
            } else {
                None
            }
        };
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        assert_eq!(
            opts.database_url, "sqlite:/tmp/from-the-owner.db",
            "the owner hands the value over unchanged"
        );
    }

    #[test]
    fn sqlite_url_and_env_paths() {
        let effective = json!({"backend": "sqlite", "store": {"database_url": "sqlite:/tmp/x.db"}});
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        assert_eq!(opts.resolve_sqlite_path(), PathBuf::from("/tmp/x.db"));
        let effective = json!({"backend": "sqlite"});
        // A Postgres address in the environment does not become a SQLite path.
        let mut env = |k: &str| {
            if k == clio_config::db_url_env::DATABASE_URL_ENV {
                Some("postgres://ignored".into())
            } else {
                None
            }
        };
        assert!(env("OTHER").is_none());
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        assert_eq!(opts.resolve_sqlite_path(), PathBuf::from("./data/clio.db"));

        // Successful nested sqlite path (covers create_parent_dir Ok + block end).
        let nested = std::env::temp_dir().join(format!("am-nest-{}/db.sqlite", std::process::id()));
        let effective = json!({
            "backend": "sqlite",
            "store": {"sqlite_path": nested.to_str().unwrap()}
        });
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let (backend, _) = open_store(&opts, kms).unwrap();
        assert_eq!(backend, StoreBackend::Sqlite);
        let _ = std::fs::remove_file(&nested);
        let _ = std::fs::remove_dir(nested.parent().expect("nested parent"));

        let bare = json!({"backend": "sqlite", "store": {"database_url": "/tmp/bare-am.db"}});
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&bare, &mut env).unwrap();
        assert_eq!(opts.resolve_sqlite_path(), PathBuf::from("/tmp/bare-am.db"));

        let mut env = |k: &str| {
            if k == clio_config::db_url_env::DATABASE_URL_ENV {
                Some("sqlite:/tmp/from-am.db".into())
            } else {
                None
            }
        };
        let opts =
            StoreOpenOptions::from_effective(&json!({"backend": "sqlite"}), &mut env).unwrap();
        assert!(opts.database_url.contains("from-am"));

        let mem = json!({"backend": "sqlite", "store": {"sqlite_path": ":memory:"}});
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&mem, &mut env).unwrap();
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let (backend, _) = open_store(&opts, kms).unwrap();
        assert_eq!(backend, StoreBackend::Sqlite);
    }

    #[test]
    fn postgres_open_store_and_bad_url() {
        let _guard = crate::lock_pg_test();
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        assert!(
            crate::postgres::PostgresStore::open(
                "postgres://nope:nope@127.0.0.1:1/none",
                Arc::clone(&kms)
            )
            .is_err()
        );
        let url = crate::db_url::postgres_url_from_env();
        let effective = json!({"backend": "postgres", "store": {"database_url": url}});
        let mut env = |_k: &str| None;
        let opts = StoreOpenOptions::from_effective(&effective, &mut env).unwrap();
        let (backend, _) = open_store(&opts, kms).unwrap();
        assert_eq!(backend, StoreBackend::Postgres);
    }
}
