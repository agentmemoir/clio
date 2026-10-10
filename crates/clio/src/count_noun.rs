// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one count and the noun that goes with it, so
//! every line that shows a count reads the way a person would write it.
//!
//! ## Owns
//! - `count_noun`, the one place the restore's count lines, a restore's table
//!   row count, the hygiene audit's summary line, and the `admit --file` batch
//!   preview choose a noun
//!
//! ## Does not own
//! - Which counts a command shows (see the command's own report module)
//! - The sentence, label, or field name around the count
//!
//! ## Boundary
//!
//! Changes to how a count is worded should normally require changes here.
//! Changes to which counts a command reports should not.
//!
//! Keep this module focused on one count and its noun.

/// `count` with its noun: the singular for exactly one, the plural otherwise.
///
/// A count of zero takes the plural, because "0 items" is what an operator
/// expects to read and "0 item" would read as a typo. One shared helper serves
/// the restore's counts phrase, the restore's table row count, the hygiene
/// audit's summary line, and the `admit --file` batch preview, so those lines
/// cannot drift into different wording for the same count of the same noun.
#[must_use]
pub(crate) fn count_noun(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::count_noun;

    #[test]
    fn a_count_of_one_takes_the_singular_noun() {
        assert_eq!(count_noun(1, "item", "items"), "1 item");
    }

    #[test]
    fn a_count_of_zero_takes_the_plural_noun() {
        assert_eq!(count_noun(0, "item", "items"), "0 items");
    }

    #[test]
    fn a_count_of_two_takes_the_plural_noun() {
        assert_eq!(count_noun(2, "item", "items"), "2 items");
    }

    #[test]
    fn the_singular_form_is_the_only_count_that_changes() {
        let many: Vec<String> = (2..12).map(|n| count_noun(n, "table", "tables")).collect();
        assert_eq!(
            many,
            vec![
                "2 tables",
                "3 tables",
                "4 tables",
                "5 tables",
                "6 tables",
                "7 tables",
                "8 tables",
                "9 tables",
                "10 tables",
                "11 tables",
            ],
            "only a count of 1 may differ from the plural"
        );
    }
}
