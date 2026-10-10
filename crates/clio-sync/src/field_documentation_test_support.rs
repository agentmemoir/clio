// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one table that says which side of the
//! cleartext/sealed line each documented sync field sits on, and for locating
//! a name by the clause it is written in. Two test files read the same
//! operator-facing enumeration and must not disagree about what it says, so the
//! table and the clause locator live here rather than in either of them.
//!
//! ## Owns
//! - `Side`: the three sides of the line a documented field can sit on
//! - `DOCUMENTED_FIELDS`: the documented name, the journal column it is read
//!   from, and its side
//! - The three clause openers every enumeration is written with
//! - `field_regions` and `assert_each_field_is_named_on_its_side`
//!
//! ## Does not own
//! - The warning text (`bind_warning`) and the help text (`cli_help_usage`)
//! - The wire's own column list (`clio_store`)
//! - Any assertion; a shared helper has several users and so holds no test
//!
//! ## Boundary
//!
//! Changes to which side a documented field sits on, or to the words that open
//! each clause, should normally require changes here and in both readers.
//! Changes to the warning or the help text should not require changes here.
//!
//! Test-only. Never compiled into production builds.

/// Which side of the line a documented field sits on.
///
/// The three sides are not a matter of degree. A field on the routing side is
/// read as plain text by the relay, a field on the keyed side is read as plain
/// text but derived under a key the relay does not hold, and a field on the
/// sealed side is not readable at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// Read as plain text, because routing, ordering, or authorizing needs it.
    Routing,
    /// Read as plain text, but derived under a key the relay does not hold.
    KeyedCleartext,
    /// Not readable by the relay.
    Sealed,
}

/// One documented field: the name the operator-facing text uses, the journal
/// column it is read from, and the side it sits on.
pub(crate) struct DocumentedField {
    /// The name as the warning and the help write it.
    pub(crate) name: &'static str,
    /// The relay journal column the field is read from.
    pub(crate) column: &'static str,
    /// Which side of the line the field sits on.
    pub(crate) side: Side,
}

/// The words that open the routing clause of every enumeration.
pub(crate) const ROUTING_CLAUSE_START: &str = "bank scope";

/// The words that open the keyed-cleartext clause of every enumeration.
pub(crate) const KEYED_CLAUSE_START: &str = "the record identifier";

/// The words that open the sealed clause of every enumeration.
pub(crate) const SEALED_CLAUSE_START: &str = "only the content";

/// Every field the operator-facing text enumerates, with the column each is
/// read from and the side it sits on.
///
/// This is the single source both readers check against, so the warning, the
/// help, and the wire cannot disagree about one field without one of the two
/// checks failing.
pub(crate) const DOCUMENTED_FIELDS: [DocumentedField; 9] = [
    DocumentedField {
        name: "bank scope",
        column: "bank_id",
        side: Side::Routing,
    },
    DocumentedField {
        name: "record kind",
        column: "entity_kind",
        side: Side::Routing,
    },
    DocumentedField {
        name: "operation",
        column: "op",
        side: Side::Routing,
    },
    DocumentedField {
        name: "transaction time",
        column: "transaction_time",
        side: Side::Routing,
    },
    DocumentedField {
        name: "device id",
        column: "device_id",
        side: Side::Routing,
    },
    DocumentedField {
        name: "the record identifier",
        column: "entity_id",
        side: Side::KeyedCleartext,
    },
    DocumentedField {
        name: "the content digest",
        column: "content_hash",
        side: Side::KeyedCleartext,
    },
    DocumentedField {
        name: "the content and the admission score",
        column: "content_sealed",
        side: Side::Sealed,
    },
    DocumentedField {
        // The sealed clause names the content and the score as one phrase, so
        // both rows carry that phrase. Naming the phrase rather than a bare word
        // is what stops the check being satisfied by a substring somewhere else
        // in the sentence: `content` on its own also appears inside the keyed
        // clause's `content digest`.
        name: "the content and the admission score",
        column: "admission_receipt",
        side: Side::Sealed,
    },
];

/// Split an enumeration into its three clauses, in the order the wire presents
/// them: routing, then keyed cleartext, then sealed.
///
/// A name is located by the clause it appears in rather than by the string as a
/// whole, which is the property that makes the check fail when a cleartext
/// field is named inside the sealed clause. Finding one name on the wrong side
/// and finding none at all are the same defect, so both fail here.
pub(crate) fn field_regions(text: &str) -> (&str, &str, &str) {
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("the enumeration must open a clause with `{needle}`: {text}"))
    };
    let routing = at(ROUTING_CLAUSE_START);
    let keyed = at(KEYED_CLAUSE_START);
    let sealed = at(SEALED_CLAUSE_START);
    assert!(
        routing < keyed && keyed < sealed,
        "the three clauses must appear in order: {text}"
    );
    (&text[routing..keyed], &text[keyed..sealed], &text[sealed..])
}

/// Assert that every documented field is named inside the clause its side names,
/// and that the sealed clause names no field from another side.
pub(crate) fn assert_each_field_is_named_on_its_side(text: &str) {
    let (routing, keyed, sealed) = field_regions(text);
    for field in &DOCUMENTED_FIELDS {
        let region = match field.side {
            Side::Routing => routing,
            Side::KeyedCleartext => keyed,
            Side::Sealed => sealed,
        };
        assert!(
            region.contains(field.name),
            "`{}` is documented on the {:?} side but was not found in that clause: {text}",
            field.name,
            field.side
        );
    }
    for field in &DOCUMENTED_FIELDS {
        if field.side != Side::Sealed {
            assert!(
                !sealed.contains(field.name),
                "`{}` crosses in the clear and must not be named in the sealed clause: {text}",
                field.name
            );
        }
    }
}

/// The documented field names that cross the network in the clear, sorted, so a
/// set comparison against the wire reports the difference rather than an order.
pub(crate) fn documented_cleartext_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = DOCUMENTED_FIELDS
        .iter()
        .filter(|field| field.side != Side::Sealed)
        .map(|field| field.name)
        .collect();
    names.sort_unstable();
    names
}
