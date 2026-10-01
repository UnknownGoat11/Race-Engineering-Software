//! Number formatting for the diagnostic views. NaN and infinite values are never printed as
//! numbers: they read `UNAVAILABLE (...)`, so a missing channel cannot be mistaken for a value.

use std::fmt::Display;

use lmu_reader::RawString;

/// Marker printed for absent or non-finite values.
pub(crate) const UNAVAILABLE: &str = "UNAVAILABLE";

const NS_PER_US: f64 = 1_000.0;
const NS_PER_MS: f64 = 1_000_000.0;
const NS_PER_S: f64 = 1_000_000_000.0;
const MS_PER_S: f64 = 1_000.0;
const MS_PER_S_U64: u64 = 1_000;
const MS_PER_MINUTE: u64 = 60_000;

fn non_finite(v: f64) -> Option<String> {
    if v.is_nan() {
        Some(format!("{UNAVAILABLE} (NaN)"))
    } else if v.is_infinite() {
        Some(format!(
            "{UNAVAILABLE} ({}inf)",
            if v > 0.0 { "+" } else { "-" }
        ))
    } else {
        None
    }
}

/// Display-only: prints `-0.0` as `0.0` (e.g. a negated zero acceleration).
fn no_negative_zero(v: f64) -> f64 {
    if v == 0.0 { 0.0 } else { v }
}

/// `v` with `decimals` decimals, or UNAVAILABLE.
pub(crate) fn num(v: f64, decimals: usize) -> String {
    non_finite(v).unwrap_or_else(|| format!("{:.decimals$}", no_negative_zero(v)))
}

/// `v` with an explicit sign, or UNAVAILABLE.
pub(crate) fn signed(v: f64, decimals: usize) -> String {
    non_finite(v).unwrap_or_else(|| format!("{:+.decimals$}", no_negative_zero(v)))
}

/// An `f32` channel.
pub(crate) fn num32(v: f32, decimals: usize) -> String {
    num(f64::from(v), decimals)
}

/// Lap or sector time in seconds as `m:ss.mmm`; `none` for the "no time" sentinel (≤ 0).
pub(crate) fn lap_time(seconds: f64) -> String {
    if let Some(text) = non_finite(seconds) {
        return text;
    }
    if seconds <= 0.0 {
        return "none".to_owned();
    }
    // Round once to whole milliseconds so 59.9996 s prints as 1:00.000, not 0:60.000.
    let total_ms = (seconds * MS_PER_S).round() as u64;
    let minutes = total_ms / MS_PER_MINUTE;
    let secs = total_ms % MS_PER_MINUTE / MS_PER_S_U64;
    let millis = total_ms % MS_PER_S_U64;
    format!("{minutes}:{secs:02}.{millis:03}")
}

/// A duration given in nanoseconds, in a readable unit.
pub(crate) fn duration_ns(ns: u64) -> String {
    let v = ns as f64;
    if v < NS_PER_US {
        format!("{ns} ns")
    } else if v < NS_PER_MS {
        format!("{:.1} us", v / NS_PER_US)
    } else if v < NS_PER_S {
        format!("{:.1} ms", v / NS_PER_MS)
    } else {
        format!("{:.2} s", v / NS_PER_S)
    }
}

/// An optional duration in nanoseconds.
pub(crate) fn opt_duration_ns(ns: Option<u64>) -> String {
    ns.map_or_else(|| "n/a".to_owned(), duration_ns)
}

/// A measured rate.
pub(crate) fn hz(rate: Option<f64>) -> String {
    rate.map_or_else(
        || "measuring...".to_owned(),
        |r| format!("{} Hz", num(r, 1)),
    )
}

/// An optional displayable value.
pub(crate) fn opt<T: Display>(v: Option<T>) -> String {
    v.map_or_else(|| "n/a".to_owned(), |v| v.to_string())
}

/// An LMU-supplied string, safe to print to a terminal driven with escape sequences: control
/// characters (ESC, OSC introducers, BEL, ...) are shown escaped (`\u{1b}`), never emitted, so a
/// lobby or team name cannot rewrite the screen or the clipboard.
pub(crate) fn safe_text(s: &RawString) -> String {
    let mut out = String::with_capacity(s.text.len());
    for c in s.text.chars() {
        if c.is_control() {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

/// `yes` / `no`.
pub(crate) fn yes_no(v: bool) -> &'static str {
    if v { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_values_are_unavailable() {
        assert_eq!(num(f64::NAN, 2), "UNAVAILABLE (NaN)");
        assert_eq!(signed(f64::NEG_INFINITY, 1), "UNAVAILABLE (-inf)");
        assert_eq!(num32(f32::INFINITY, 1), "UNAVAILABLE (+inf)");
        assert_eq!(num(1.236, 2), "1.24");
        assert_eq!(signed(0.5, 1), "+0.5");
    }

    #[test]
    fn control_characters_are_escaped_not_emitted() {
        let hostile = RawString::decode(b"Team \x1b]52;c;aGk=\x07 \x1b[2J\0");
        assert!(!hostile.printable);
        let shown = safe_text(&hostile);
        assert!(!shown.chars().any(char::is_control), "{shown:?}");
        assert_eq!(shown, r"Team \u{1b}]52;c;aGk=\u{7} \u{1b}[2J");
        assert_eq!(safe_text(&RawString::decode(b"Le Mans\0")), "Le Mans");
    }

    #[test]
    fn lap_times() {
        assert_eq!(lap_time(211.234), "3:31.234");
        assert_eq!(lap_time(59.9), "0:59.900");
        assert_eq!(lap_time(59.9996), "1:00.000");
        assert_eq!(lap_time(0.0), "none");
        assert_eq!(lap_time(-1.0), "none");
        assert_eq!(lap_time(f64::NAN), "UNAVAILABLE (NaN)");
    }

    #[test]
    fn durations_and_rates() {
        assert_eq!(duration_ns(950), "950 ns");
        assert_eq!(duration_ns(12_300), "12.3 us");
        assert_eq!(duration_ns(4_200_000), "4.2 ms");
        assert_eq!(duration_ns(1_500_000_000), "1.50 s");
        assert_eq!(hz(None), "measuring...");
        assert_eq!(hz(Some(99.84)), "99.8 Hz");
    }
}
