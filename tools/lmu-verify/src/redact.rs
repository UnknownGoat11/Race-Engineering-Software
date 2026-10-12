//! Personal-data redaction for everything the report prints from a capture.
//!
//! Captures contain driver names, the player name, the player file name and server names (PII,
//! `lmu_reader::capture` docs). The report never prints those fields, and as a safety net every other string
//! taken from a capture (vehicle, track and class names, compatibility details, notice and event texts,
//! error messages) passes through [`Redactor::redact`], which replaces every collected name with
//! [`REDACTED`] (ASCII case-insensitive). Collection is bounded by [`MAX_NAMES`].

use std::collections::BTreeSet;

/// Replacement text for a redacted name.
pub const REDACTED: &str = "<name>";
/// Most names collected per run; beyond it [`NameCollector::overflowed`] is set and reported.
pub const MAX_NAMES: usize = 4_096;
/// Shorter names are not redacted (a one- or two-letter name would erase ordinary words and numbers).
pub const MIN_NAME_CHARS: usize = 3;

/// Collects the personal names seen in a capture.
#[derive(Debug, Clone, Default)]
pub struct NameCollector {
    names: BTreeSet<String>,
    overflowed: bool,
}

impl NameCollector {
    /// Records one name (trimmed; too-short names are ignored).
    pub fn observe(&mut self, name: &str) {
        let name = name.trim();
        if name.chars().count() < MIN_NAME_CHARS || self.names.contains(name) {
            return;
        }
        if self.names.len() >= MAX_NAMES {
            self.overflowed = true;
            return;
        }
        self.names.insert(name.to_owned());
    }

    /// Adds every name of `other`.
    pub fn merge(&mut self, other: &Self) {
        for name in &other.names {
            self.observe(name);
        }
        self.overflowed |= other.overflowed;
    }

    /// Names collected.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// `true` when no name was collected.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// `true` if more than [`MAX_NAMES`] distinct names were offered.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The redactor for the collected names.
    pub fn redactor(&self) -> Redactor {
        let mut needles: Vec<(String, usize)> = self
            .names
            .iter()
            .map(|n| (n.to_ascii_lowercase(), n.len()))
            .collect();
        // Longest first, so a name containing another is replaced whole.
        needles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        needles.dedup_by(|a, b| a.0 == b.0);
        Redactor {
            needles: needles.into_iter().map(|(n, _)| n).collect(),
        }
    }
}

/// Replaces collected names in text.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// ASCII-lowercased names, longest first.
    needles: Vec<String>,
}

impl Redactor {
    /// `text` with every collected name replaced by [`REDACTED`].
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for needle in &self.needles {
            out = replace_ascii_case_insensitive(&out, needle);
        }
        out
    }

    /// [`Redactor::redact`] of an optional string.
    pub fn redact_opt(&self, text: Option<&str>) -> Option<String> {
        text.map(|t| self.redact(t))
    }
}

/// Replaces every ASCII-case-insensitive occurrence of `needle_lower` (already lowercased) in `text`.
fn replace_ascii_case_insensitive(text: &str, needle_lower: &str) -> String {
    if needle_lower.is_empty() {
        return text.to_owned();
    }
    // ASCII lowercasing keeps byte lengths, so offsets in `hay` are offsets in `text`; a match of a whole
    // UTF-8 needle always starts and ends on character boundaries.
    let hay = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, _) in hay.match_indices(needle_lower) {
        if start < last {
            continue;
        }
        out.push_str(text.get(last..start).unwrap_or_default());
        out.push_str(REDACTED);
        last = start + needle_lower.len();
    }
    out.push_str(text.get(last..).unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_replaced_case_insensitively_longest_first() {
        let mut c = NameCollector::default();
        c.observe("Jane Doe");
        c.observe("Jane");
        c.observe("Al"); // too short: never redacted
        c.observe("  ");
        assert_eq!(c.len(), 2);
        let r = c.redactor();
        assert_eq!(
            r.redact("#7 JANE DOE Racing, driven by jane; Al Unser"),
            "#7 <name> Racing, driven by <name>; Al Unser"
        );
        assert_eq!(r.redact("Hypercar"), "Hypercar");
        assert_eq!(r.redact_opt(None), None);
    }

    #[test]
    fn non_ascii_text_survives() {
        let mut c = NameCollector::default();
        c.observe("Zoë Müller");
        let r = c.redactor();
        assert_eq!(r.redact("Pilote: Zoë Müller — Spa"), "Pilote: <name> — Spa");
    }

    #[test]
    fn collection_is_bounded() {
        let mut c = NameCollector::default();
        for i in 0..MAX_NAMES + 10 {
            c.observe(&format!("Driver {i:05}"));
        }
        assert_eq!(c.len(), MAX_NAMES);
        assert!(c.overflowed());
        let mut merged = NameCollector::default();
        merged.merge(&c);
        assert!(merged.overflowed());
    }
}
