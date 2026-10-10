// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the pinned mission pattern subset: parsing and
//! bounded matching for regex-flavored mission entries.
//!
//! ## Owns
//! - The pinned subset grammar: literals, character classes, `+`/`*`/`?` quantifiers,
//!   and `|` alternation; anything else is rejected at parse time
//! - Compilation into branch terms and step-budgeted matching (ReDoS-safe by bound)
//!
//! ## Does not own
//! - Mission schema validation, duplicate checks, and rule ids (see `retention_mission`)
//! - Literal substring matching for `literal` entries (see `retention_mission`)
//!
//! ## Boundary
//!
//! Changes to the accepted pattern syntax or the matching bound should normally
//! require changes here. Changes to profile fields or admission math should not
//! require changes here.
//!
//! Keep this module focused on the pinned pattern subset.

/// Longest accepted pattern (entries carry the same cap).
pub const MAX_PATTERN_CHARS: usize = 200;

/// Hard step budget for one entry evaluation. Matching is deterministic; the
/// budget is the evaluation-time bound that replaces a wall-clock timeout so
/// crafted missions can never stall a write.
pub const MAX_MATCH_STEPS: u64 = 50_000;

/// One atom: a single literal char or one character class.
#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Char(char),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

/// Quantifier applied to a single atom (no nesting is representable).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Quant {
    One,
    Opt,
    Star,
    Plus,
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    atom: Atom,
    quant: Quant,
}

/// A compiled pinned-subset pattern: alternation of term sequences.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionPattern {
    branches: Vec<Vec<Term>>,
}

/// Result of a budgeted match: `None` means the step budget ran out and the
/// entry is treated as a non-match for that item (logged by the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionMatchResult {
    Matched,
    NoMatch,
    BudgetExhausted,
}

/// Compile a pinned-subset pattern. Case handling is the caller's: both the
/// pattern and the candidate text are lowercased before matching.
///
/// # Panics
/// Panics on internal invariant violations only (branch vec is never empty).
///
/// # Errors
/// Anything outside the pinned subset: empty branches, groups, braces,
/// backslash escapes, nested or dangling quantifiers, malformed classes,
/// or patterns over [`MAX_PATTERN_CHARS`].
pub fn compile(pattern: &str) -> Result<MissionPattern, String> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(format!(
            "regex entry exceeds {MAX_PATTERN_CHARS} characters"
        ));
    }
    let chars: Vec<char> = pattern.to_lowercase().chars().collect();
    let mut branches: Vec<Vec<Term>> = vec![Vec::new()];
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '|' => {
                if branches.last().is_some_and(Vec::is_empty) {
                    return Err("empty alternation branch".into());
                }
                branches.push(Vec::new());
                i += 1;
            }
            '?' | '*' | '+' => {
                return Err(format!("quantifier `{c}` has nothing to repeat"));
            }
            '[' => {
                let (atom, next) = parse_class(&chars, i)?;
                i = next;
                let (term, next) = parse_quant(&chars, i, atom)?;
                i = next;
                branches.last_mut().expect("at least one branch").push(term);
            }
            '(' | ')' | '{' | '}' | '\\' => {
                return Err(format!("character `{c}` is outside the pinned subset"));
            }
            _ => {
                // The quantifier, if any, sits after the atom, not on it.
                let (term, next) = parse_quant(&chars, i + 1, Atom::Char(c))?;
                i = next;
                branches.last_mut().expect("at least one branch").push(term);
            }
        }
    }
    if branches.iter().any(Vec::is_empty) {
        return Err("empty alternation branch".into());
    }
    Ok(MissionPattern { branches })
}

/// Parse a `[...]` class starting at `open` (which is `[`); returns the atom and
/// the index just past the closing `]`.
fn parse_class(chars: &[char], open: usize) -> Result<(Atom, usize), String> {
    let mut i = open + 1;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    loop {
        let Some(&c) = chars.get(i) else {
            return Err("unterminated character class".into());
        };
        if c == ']' && !first {
            i += 1;
            break;
        }
        first = false;
        // Range `a-b` needs a following `-` and a closing char.
        if chars.get(i + 1) == Some(&'-') && chars.get(i + 2).is_some_and(|&e| e != ']') {
            let end = chars[i + 2];
            if end < c {
                return Err(format!("inverted class range `{c}-{end}`"));
            }
            ranges.push((c, end));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }
    Ok((Atom::Class { negated, ranges }, i))
}

/// Parse an optional quantifier after the atom starting at `i`.
fn parse_quant(chars: &[char], i: usize, atom: Atom) -> Result<(Term, usize), String> {
    let quant = match chars.get(i) {
        Some('?') => Quant::Opt,
        Some('*') => Quant::Star,
        Some('+') => Quant::Plus,
        _ => Quant::One,
    };
    let next = if quant == Quant::One { i } else { i + 1 };
    if chars
        .get(next)
        .is_some_and(|&c| matches!(c, '?' | '*' | '+'))
    {
        return Err("nested quantifiers are outside the pinned subset".into());
    }
    Ok((Term { atom, quant }, next))
}

/// Does one atom match the char at `pos`?
fn atom_matches(atom: &Atom, text: &[char], pos: usize) -> bool {
    let Some(&c) = text.get(pos) else {
        return false;
    };
    match atom {
        Atom::Char(expected) => c == *expected,
        Atom::Class { negated, ranges } => {
            let inside = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
            inside != *negated
        }
    }
}

/// Budgeted search for an unanchored match of `pattern` anywhere in `text`
/// (already lowercased by the caller).
#[must_use]
pub fn is_match(pattern: &MissionPattern, text: &[char]) -> MissionMatchResult {
    let mut steps: u64 = 0;
    for start in 0..=text.len() {
        for branch in &pattern.branches {
            match match_terms(branch, 0, start, text, &mut steps) {
                Some(true) => return MissionMatchResult::Matched,
                Some(false) => {}
                None => return MissionMatchResult::BudgetExhausted,
            }
        }
    }
    MissionMatchResult::NoMatch
}

/// Match `terms[ti..]` against `text` at `pos`; `None` on budget exhaustion.
fn match_terms(
    terms: &[Term],
    ti: usize,
    pos: usize,
    text: &[char],
    steps: &mut u64,
) -> Option<bool> {
    *steps += 1;
    if *steps > MAX_MATCH_STEPS {
        return None;
    }
    let Some(term) = terms.get(ti) else {
        return Some(true);
    };
    match term.quant {
        Quant::One => {
            if atom_matches(&term.atom, text, pos) {
                match_terms(terms, ti + 1, pos + 1, text, steps)
            } else {
                Some(false)
            }
        }
        Quant::Opt => {
            if atom_matches(&term.atom, text, pos) {
                match match_terms(terms, ti + 1, pos + 1, text, steps) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => return None,
                }
            }
            match_terms(terms, ti + 1, pos, text, steps)
        }
        Quant::Star | Quant::Plus => repeat_term(terms, ti, pos, pos, text, steps),
    }
}

/// Greedy repetition of `terms[ti]` starting at `pos`; `start` is where the
/// first repetition must begin for `Plus`.
///
/// Iterative on purpose: the recursive form consumed one stack frame per
/// repetition, so a long candidate text could push ~50k frames (the step
/// budget) and overflow small thread stacks. Positions to retry are tracked
/// explicitly, longest repetition count first, mirroring the greedy order.
fn repeat_term(
    terms: &[Term],
    ti: usize,
    pos: usize,
    start: usize,
    text: &[char],
    steps: &mut u64,
) -> Option<bool> {
    let term = &terms[ti];
    let mut consumed: Vec<usize> = Vec::new();
    let mut p = pos;
    loop {
        *steps += 1;
        if *steps > MAX_MATCH_STEPS {
            return None;
        }
        if !atom_matches(&term.atom, text, p) {
            break;
        }
        consumed.push(p);
        p += 1;
    }
    if term.quant == Quant::Plus && p == start {
        // Zero repetitions for `+`: the tail may only match at start+1 or later.
        return Some(false);
    }
    loop {
        match match_terms(terms, ti + 1, p, text, steps) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => return None,
        }
        match consumed.pop() {
            Some(prev) => p = prev,
            None => return Some(false),
        }
        if term.quant == Quant::Plus && p == start {
            return Some(false);
        }
    }
}

#[cfg(test)]
#[path = "mission_match_tests.rs"]
mod mission_match_tests;
