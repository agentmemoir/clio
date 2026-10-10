// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Behavioral parity suite for the belief repository.

use crate::belief_store::BeliefStore;
use crate::dek::{DekProvider, LocalDevKms};
use clio_types::{BeliefObject, ConfidenceEntry, ErrorCode, SourceType, proposition_key};
use std::sync::Arc;

use crate::test_unique::unique;

pub(crate) fn mk_belief(id: &str, bank: &str, subject: &str) -> BeliefObject {
    BeliefObject {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        proposition: "User Is Likely Vegetarian".into(),
        context: None,
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-06-01", 0.4, "turn_112", SourceType::AgentInferred)
                .expect("entry"),
        ],
        created_at: "2026-06-01".into(),
        updated_at: "2026-06-01".into(),
    }
}

/// Contract assertions any [`BeliefStore`] + KMS pair must satisfy.
#[allow(clippy::too_many_lines)]
pub fn run_belief_suite(store: &dyn BeliefStore, kms: &dyn DekProvider) {
    let bank = unique("bank-belief");
    let other = unique("bank-other");
    let subject = unique("subj");
    let id = unique("bel");

    let belief = mk_belief(&id, &bank, &subject);
    store.create_belief(&belief, "agent").expect("create");

    let got = store.get_belief(&bank, &id).expect("get").expect("present");
    assert_eq!(got.proposition, belief.proposition);
    assert_eq!(got.confidence_history.len(), 1);
    assert_eq!(got.current_confidence(), Some(0.4));
    assert_eq!(got.current_source_type(), Some(SourceType::AgentInferred));
    // A belief written without context omits the key from its serialized form.
    let json = serde_json::to_string(&got).unwrap();
    assert!(!json.contains("\"context\""), "{json}");

    // Bank scoping: reads fail closed.
    assert!(store.get_belief(&other, &id).unwrap().is_none());
    let history = store.belief_history(&other, &id, None).unwrap();
    assert!(history.is_empty(), "{history:?}");

    // Identity by normalized proposition key.
    let key = proposition_key(&belief.proposition);
    let found = store
        .find_belief_by_proposition(&bank, &key)
        .unwrap()
        .expect("found");
    assert_eq!(found.id, id);
    assert!(
        store
            .find_belief_by_proposition(&bank, "no such belief")
            .unwrap()
            .is_none()
    );

    // Proposition content is encrypted at rest.
    let raw = store
        .inspect_belief_ciphertext(&bank, &id)
        .unwrap()
        .expect("cipher");
    assert!(!raw.contains("Vegetarian"));
    assert!(raw.contains("ciphertext_b64"));

    // Append grows history; prior entry byte-identical.
    let first = got.confidence_history[0].clone();
    let second = ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred)
        .expect("entry");
    store
        .append_confidence(&bank, &id, &second, "agent")
        .expect("append");
    let hist = store.belief_history(&bank, &id, None).unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0], first);
    assert_eq!(hist[1], second);
    let after = store.get_belief(&bank, &id).unwrap().unwrap();
    assert_eq!(after.current_confidence(), Some(0.65));
    assert_eq!(after.updated_at, "2026-07-14");

    // Point-in-time.
    assert_eq!(
        store
            .belief_history(&bank, &id, Some("2026-05-31"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        store
            .belief_history(&bank, &id, Some("2026-06-01"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .belief_history(&bank, &id, Some("2026-08-01"))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(after.as_of_confidence("2026-06-15"), Some(0.4));

    // Optional context: created sealed under the subject DEK and
    // surfaced on reads; plain appends preserve it, context appends replace it.
    let ctx_bank = unique("bank-bctx");
    let ctx_id = unique("bel-bctx");
    let mut with_ctx = mk_belief(&ctx_id, &ctx_bank, &subject);
    with_ctx.context = Some("prefer oat milk at home".into());
    store.create_belief(&with_ctx, "agent").expect("create");
    let want = Some("prefer oat milk at home".to_owned());
    let got_ctx = store.get_belief(&ctx_bank, &ctx_id).unwrap().expect("get");
    assert_eq!(got_ctx.context, want);
    assert!(
        store
            .list_beliefs(&ctx_bank)
            .unwrap()
            .iter()
            .any(|b| b.id == ctx_id && b.context == want)
    );
    let ctx_key = proposition_key(&with_ctx.proposition);
    assert_eq!(
        store
            .find_belief_by_proposition(&ctx_bank, &ctx_key)
            .unwrap()
            .expect("found")
            .context,
        want
    );
    store
        .append_confidence(&ctx_bank, &ctx_id, &second, "agent")
        .expect("append");
    let after_plain = store.get_belief(&ctx_bank, &ctx_id).unwrap().expect("get");
    assert_eq!(after_plain.context, want, "plain append must preserve");
    // An append may omit context or supply a byte-identical value; a
    // differing value is rejected before any write. Only the authorized
    // `correct_belief_context` operation replaces a stored context.
    let err = store
        .append_confidence_with_context(&ctx_bank, &ctx_id, &second, "agent", Some("new context"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let after_reject = store.get_belief(&ctx_bank, &ctx_id).unwrap().expect("get");
    assert_eq!(
        after_reject.context, want,
        "a rejected change must not write"
    );
    // A byte-identical supplied context is accepted and preserves the value.
    store
        .append_confidence_with_context(&ctx_bank, &ctx_id, &second, "agent", want.as_deref())
        .expect("byte-identical context accepted");
    let after_identical = store.get_belief(&ctx_bank, &ctx_id).unwrap().expect("get");
    assert_eq!(after_identical.context, want);
    // Supplying a context where none is stored is a change, not an append.
    let err = store
        .append_confidence_with_context(&bank, &id, &second, "agent", Some("no context stored"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // Append rejects unknown belief / wrong bank.
    assert!(
        store
            .append_confidence(&bank, "missing-belief", &second, "agent")
            .is_err()
    );
    assert!(
        store
            .append_confidence(&other, &id, &second, "agent")
            .is_err()
    );

    // Duplicate create and structural rejects.
    assert!(store.create_belief(&belief, "agent").is_err());
    assert!(store.create_belief(&belief, "").is_err());
    let mut empty = mk_belief(&unique("bel-empty"), &bank, &subject);
    empty.confidence_history.clear();
    assert!(store.create_belief(&empty, "agent").is_err());
    let bad = ConfidenceEntry {
        as_of: "2026-08-01".into(),
        confidence: 2.0,
        evidence_ref: "turn".into(),
        source_type: SourceType::UserStated,
    };
    assert_eq!(
        store
            .append_confidence(&bank, &id, &bad, "agent")
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        store
            .append_confidence(&bank, &id, &second, "")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );

    // Crypto-shredding renders the proposition unreadable.
    let shredded = unique("subj-shred");
    let sid = unique("bel-shred");
    let mut b = mk_belief(&sid, &bank, &shredded);
    b.proposition = "secret proposition".into();
    store.create_belief(&b, "agent").unwrap();
    kms.destroy_dek(&shredded).unwrap();
    assert_eq!(
        store.get_belief(&bank, &sid).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::IntoAmError;
    use crate::postgres::PostgresStore;
    use crate::sqlite::SqliteStore;

    #[test]
    fn sqlite_belief_parity() {
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite");
        run_belief_suite(&store, kms.as_ref());
    }

    /// Durable belief audit rows must reconstruct the observe: the
    /// confidence column and detail_json carry confidence, source_type, and the
    /// trajectory length after the write; beliefs.updated_at stays in sync.
    #[test]
    #[allow(clippy::float_cmp)]
    fn sqlite_belief_audit_rows_carry_reconstruction_fields() {
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
        let bank = unique("bank-audit");
        let id = unique("bel-audit");
        let belief = mk_belief(&id, &bank, &unique("subj-audit"));
        store.create_belief(&belief, "agent").unwrap();
        let second =
            ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred)
                .expect("entry");
        store
            .append_confidence(&bank, &id, &second, "agent")
            .unwrap();

        let rows: Vec<(f64, String)> = store
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT confidence, detail_json FROM audit_events
                         WHERE operation = 'belief_observe' AND item_id = ?1
                         ORDER BY created_at, id",
                    )
                    .am("audit query prepare")?;
                let mapped = stmt
                    .query_map([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
                    .am("audit query rows")?;
                mapped
                    .collect::<Result<Vec<_>, _>>()
                    .am("audit query collect")
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        let (create_conf, create_detail) = &rows[0];
        assert_eq!(*create_conf, 0.4);
        assert!(
            create_detail.contains("\"created\":true"),
            "{create_detail}"
        );
        assert!(
            create_detail.contains("\"confidence\":0.4"),
            "{create_detail}"
        );
        assert!(
            create_detail.contains("\"source_type\":\"agent_inferred\""),
            "{create_detail}"
        );
        assert!(
            create_detail.contains("\"entry_count\":1"),
            "{create_detail}"
        );
        let (append_conf, append_detail) = &rows[1];
        assert_eq!(*append_conf, 0.65);
        assert!(
            append_detail.contains("\"appended\":true"),
            "{append_detail}"
        );
        assert!(
            append_detail.contains("\"confidence\":0.65"),
            "{append_detail}"
        );
        assert!(
            append_detail.contains("\"source_type\":\"agent_inferred\""),
            "{append_detail}"
        );
        assert!(
            append_detail.contains("\"entry_count\":2"),
            "{append_detail}"
        );

        let updated: String = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT updated_at FROM beliefs WHERE id = ?1",
                    [id.as_str()],
                    |r| r.get(0),
                )
                .am("updated_at read")
            })
            .unwrap();
        assert_eq!(updated, "2026-07-14");
    }

    #[test]
    fn sqlite_invalid_utf8_proposition_errors() {
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
        let bank = unique("bank-utf8");
        let id = unique("bel-utf8");
        let cipher = kms
            .encrypt("subj-utf8", &[0xff, 0xfe, 0xfd])
            .unwrap()
            .to_storage_string();
        store
            .test_execute(&format!(
                "INSERT INTO beliefs (
                    id, bank_id, subject_id, proposition_key, proposition_ciphertext, created_at
                 ) VALUES ('{id}', '{bank}', 'subj-utf8', 'key-{id}', '{cipher}', 't')"
            ))
            .unwrap();
        let err = store.get_belief(&bank, &id).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
    }

    #[test]
    fn postgres_belief_parity() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        assert!(url.starts_with("postgres"));
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = crate::postgres::PostgresStore::open(&url, Arc::clone(&kms))
            .unwrap_or_else(|e| panic!("postgres+pgvector required (compose up): {e}"));
        run_belief_suite(&store, kms.as_ref());
    }

    /// Postgres mirror of the durable-telemetry reconstruction assertions.
    #[test]
    #[allow(clippy::float_cmp)]
    fn postgres_belief_audit_rows_carry_reconstruction_fields() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        assert!(url.starts_with("postgres"));
        let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
        let store = PostgresStore::open(&url, Arc::clone(&kms))
            .unwrap_or_else(|e| panic!("postgres+pgvector required (compose up): {e}"));
        let bank = unique("bank-pgaudit");
        let id = unique("bel-pgaudit");
        let belief = mk_belief(&id, &bank, &unique("subj-pgaudit"));
        store.create_belief(&belief, "agent").unwrap();
        let second =
            ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred)
                .expect("entry");
        store
            .append_confidence(&bank, &id, &second, "agent")
            .unwrap();

        let rows: Vec<(f64, String)> = store
            .with_client(|client| {
                client
                    .query(
                        "SELECT confidence::float8, detail_json FROM audit_events
                         WHERE operation = 'belief_observe' AND item_id = $1
                         ORDER BY created_at, id",
                        &[&id],
                    )
                    .am("audit query")?
                    .into_iter()
                    .map(|r| Ok((r.get(0), r.get(1))))
                    .collect()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        let (create_conf, create_detail) = &rows[0];
        assert_eq!(*create_conf, 0.4);
        assert!(
            create_detail.contains("\"created\":true"),
            "{create_detail}"
        );
        assert!(
            create_detail.contains("\"confidence\":0.4"),
            "{create_detail}"
        );
        assert!(
            create_detail.contains("\"entry_count\":1"),
            "{create_detail}"
        );
        let (append_conf, append_detail) = &rows[1];
        assert_eq!(*append_conf, 0.65);
        assert!(
            append_detail.contains("\"appended\":true"),
            "{append_detail}"
        );
        assert!(
            append_detail.contains("\"confidence\":0.65"),
            "{append_detail}"
        );
        assert!(
            append_detail.contains("\"entry_count\":2"),
            "{append_detail}"
        );

        let updated: String = store
            .with_client(|client| {
                client
                    .query_one("SELECT updated_at FROM beliefs WHERE id = $1", &[&id])
                    .am("updated_at read")
                    .map(|r| r.get(0))
            })
            .unwrap();
        assert_eq!(updated, "2026-07-14");
    }
}
