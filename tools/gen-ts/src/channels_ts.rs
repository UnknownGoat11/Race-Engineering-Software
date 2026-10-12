//! Generates `apps/desktop/src/generated/channels.ts` from `telemetry_model`.
//!
//! Everything is read from the compiled model: enum tables come from [`crate::variants`], channel
//! metadata from `registry::all()`, wheel groups from `registry::wheel_group`, groups from
//! `ChannelGroup::of`. The only thing the compiled registry does not expose is the identifier of each
//! `ch::*` constant; it is parsed from `registry.rs` and every parsed entry is checked against the compiled
//! table (id, canonical name, export name), so a wrong parse fails generation instead of producing a wrong
//! name.

use std::collections::BTreeMap;
use std::fmt::{Debug, Write as _};

use serde::Deserialize;
use telemetry_model::codec::{MAX_RECORDS, RELAY_NS_UNKNOWN, TLV_HEADER_BYTES};
use telemetry_model::registry::{self, ChannelGroup, Wheel};
use telemetry_model::value::MAX_STR_VALUE_BYTES;
use telemetry_model::{
    Availability, ChannelDef, ChannelId, FrequencyClass, Interpolation, Priority, Provenance,
    Quality, Source, TELEMETRY_SCHEMA_VERSION, Unit, ValueType,
};

use crate::variants;

/// Source of the `registry!` invocation, used only to recover the `ch::*` constant identifiers.
const REGISTRY_SOURCE: &str = include_str!("../../../crates/telemetry-model/src/registry.rs");

/// First line of every generated TypeScript file.
pub const GENERATED_BANNER: &str = "// GENERATED - do not edit; run cargo run -p gen-ts";

/// Per-wheel placement of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelPlacement {
    pub wheel: Wheel,
    /// Ids of the group's FL, FR, RL, RR channels.
    pub group: [ChannelId; 4],
}

/// The wheel a channel belongs to, if it is a member of a stride-4 per-wheel group
/// (`registry::wheel_group`).
pub fn wheel_placement(id: ChannelId) -> Option<WheelPlacement> {
    for wheel in Wheel::ALL {
        let offset = u16::try_from(wheel.index()).ok()?;
        let Some(front_left) = id.0.checked_sub(offset) else {
            continue;
        };
        if let Some(group) = registry::wheel_group(ChannelId(front_left))
            && group[wheel.index()] == id
        {
            return Some(WheelPlacement { wheel, group });
        }
    }
    None
}

/// `ch::*` constant identifier of every channel id, parsed from `registry.rs` and verified against the
/// compiled registry.
pub fn constant_names() -> BTreeMap<u16, String> {
    let mut names = BTreeMap::new();
    for line in REGISTRY_SOURCE.lines() {
        let Some(entry) = parse_registry_line(line) else {
            continue;
        };
        let def = registry::by_id(ChannelId(entry.id)).unwrap_or_else(|| {
            panic!(
                "registry.rs entry {} = 0x{:04X} is not in the compiled registry",
                entry.konst, entry.id
            )
        });
        assert_eq!(
            (def.name, def.export_name),
            (entry.name, entry.export_name),
            "registry.rs entry {} does not match the compiled registry",
            entry.konst
        );
        let previous = names.insert(entry.id, entry.konst.to_owned());
        assert!(
            previous.is_none(),
            "channel id 0x{:04X} appears twice in registry.rs",
            entry.id
        );
    }
    assert_eq!(
        names.len(),
        registry::len(),
        "every compiled channel must have exactly one parsed `ch::*` constant"
    );
    names
}

struct RegistryLine<'a> {
    konst: &'a str,
    id: u16,
    name: &'a str,
    export_name: &'a str,
}

/// Parses `KONST = 0x0001, "canonical.name", "ExportName" => …` (one `registry!` entry per line).
fn parse_registry_line(line: &str) -> Option<RegistryLine<'_>> {
    let trimmed = line.trim_start();
    let (konst, rest) = trimmed.split_once(" = 0x")?;
    let is_const_ident = !konst.is_empty()
        && konst.starts_with(|c: char| c.is_ascii_uppercase())
        && konst
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !is_const_ident {
        return None;
    }
    let (hex, rest) = rest.split_once(',')?;
    let id = u16::from_str_radix(hex.trim(), 16).ok()?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let (name, rest) = rest.split_once('"')?;
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let (export_name, rest) = rest.split_once('"')?;
    rest.trim_start().starts_with("=>").then_some(RegistryLine {
        konst,
        id,
        name,
        export_name,
    })
}

/// A TypeScript single-quoted string literal.
pub fn ts_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A TypeScript number literal that parses back to exactly `v` (shortest round-trip representation).
pub fn ts_number(v: f64) -> String {
    assert!(!v.is_nan(), "NaN cannot appear in channel metadata");
    if v == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if v == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }
    let debug = format!("{v:?}");
    let literal = debug.strip_suffix(".0").unwrap_or(&debug).to_owned();
    let reparsed: f64 = literal
        .parse()
        .unwrap_or_else(|_| panic!("number literal {literal} does not parse"));
    assert_eq!(
        reparsed.to_bits(),
        v.to_bits(),
        "number literal {literal} does not round-trip"
    );
    literal
}

fn screaming_snake(camel: &str) -> String {
    let mut out = String::new();
    for (i, c) in camel.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Writes a string-valued `as const` table for a unit-only Rust enum, plus its value type and the list of
/// every value in declaration order.
fn string_enum<T>(out: &mut String, ts_name: &str, rust_path: &str, doc: &str, all_name: &str)
where
    T: Debug + Copy + for<'de> Deserialize<'de>,
{
    let names: Vec<String> = variants::all::<T>()
        .into_iter()
        .map(variants::name)
        .collect();
    let _ = writeln!(out, "/** `{rust_path}`: {doc} */");
    let _ = writeln!(out, "export const {ts_name} = {{");
    for name in &names {
        let _ = writeln!(out, "  {name}: {},", ts_string(name));
    }
    let _ = writeln!(out, "}} as const;");
    let _ = writeln!(out, "/** A `{ts_name}` value (the Rust variant name). */");
    let _ = writeln!(
        out,
        "export type {ts_name} = (typeof {ts_name})[keyof typeof {ts_name}];"
    );
    let _ = writeln!(
        out,
        "/** Every `{ts_name}` value in Rust declaration order. */"
    );
    let list: Vec<String> = names.iter().map(|n| ts_string(n)).collect();
    let _ = writeln!(
        out,
        "export const {all_name}: readonly {ts_name}[] = [{}];",
        list.join(", ")
    );
    out.push('\n');
}

/// Writes a numeric `as const` table for a `#[repr(u8)]` wire enum (`ValueType`, `Quality`).
fn wire_enum<T>(
    out: &mut String,
    (ts_name, all_name): (&str, &str),
    rust_path: &str,
    doc: &str,
    code: impl Fn(T) -> u8,
    from_code: impl Fn(u8) -> Option<T>,
) where
    T: Debug + Copy + PartialEq + for<'de> Deserialize<'de>,
{
    let all = variants::all::<T>();
    let screaming = screaming_snake(ts_name);
    let _ = writeln!(out, "/** `{rust_path}`: {doc} */");
    let _ = writeln!(out, "export const {ts_name} = {{");
    let mut entries = Vec::new();
    for v in &all {
        let c = code(*v);
        assert_eq!(
            from_code(c),
            Some(*v),
            "{rust_path}::from_u8 must invert the discriminant"
        );
        let name = variants::name(*v);
        let _ = writeln!(out, "  {name}: {c},");
        entries.push((c, name));
    }
    let _ = writeln!(out, "}} as const;");
    let _ = writeln!(out, "/** A `{ts_name}` wire code. */");
    let _ = writeln!(
        out,
        "export type {ts_name} = (typeof {ts_name})[keyof typeof {ts_name}];"
    );
    let _ = writeln!(
        out,
        "/** Every `{ts_name}` code in Rust declaration order. */"
    );
    let codes: Vec<String> = entries.iter().map(|(c, _)| c.to_string()).collect();
    let _ = writeln!(
        out,
        "export const {all_name}: readonly {ts_name}[] = [{}];",
        codes.join(", ")
    );
    let _ = writeln!(out, "/** Rust variant name of each `{ts_name}` code. */");
    let _ = writeln!(
        out,
        "export const {screaming}_NAMES: Readonly<Record<{ts_name}, keyof typeof {ts_name}>> = {{"
    );
    for (c, name) in &entries {
        let _ = writeln!(out, "  {c}: {},", ts_string(name));
    }
    let _ = writeln!(out, "}};");
    out.push('\n');
}

fn optional_range(range: Option<(f64, f64)>) -> String {
    match range {
        Some((lo, hi)) => format!("[{}, {}]", ts_number(lo), ts_number(hi)),
        None => "null".to_owned(),
    }
}

fn channel_entry(out: &mut String, def: &ChannelDef, konst: &str) {
    let group = ChannelGroup::of(def.id)
        .unwrap_or_else(|| panic!("channel 0x{:04X} is outside every group", def.id.0));
    let (wheel, wheel_group) = match wheel_placement(def.id) {
        Some(p) => (
            ts_string(&variants::name(p.wheel)),
            format!(
                "[0x{:04X}, 0x{:04X}, 0x{:04X}, 0x{:04X}]",
                p.group[0].0, p.group[1].0, p.group[2].0, p.group[3].0
            ),
        ),
        None => ("null".to_owned(), "null".to_owned()),
    };
    let max_rate = def
        .max_rate_per_s
        .map_or_else(|| "null".to_owned(), ts_number);
    let _ = writeln!(out, "  {{");
    let _ = writeln!(
        out,
        "    id: 0x{:04X}, constName: {}, name: {}, exportName: {},",
        def.id.0,
        ts_string(konst),
        ts_string(def.name),
        ts_string(def.export_name)
    );
    let _ = writeln!(out, "    label: {},", ts_string(def.label));
    let _ = writeln!(
        out,
        "    unit: {}, unitSymbol: {}, valueType: ValueType.{},",
        ts_string(&variants::name(def.unit)),
        ts_string(def.unit.symbol()),
        variants::name(def.value_type)
    );
    let _ = writeln!(
        out,
        "    source: {}, provenance: {}, priority: {}, frequency: {},",
        ts_string(&variants::name(def.source)),
        ts_string(&variants::name(def.provenance)),
        ts_string(&variants::name(def.priority)),
        ts_string(&variants::name(def.frequency))
    );
    let _ = writeln!(
        out,
        "    interpolation: {}, availability: {},",
        ts_string(&variants::name(def.interpolation)),
        ts_string(&variants::name(def.availability))
    );
    let _ = writeln!(out, "    origin: {},", ts_string(def.origin));
    let _ = writeln!(
        out,
        "    validRange: {}, maxRatePerS: {max_rate},",
        optional_range(def.valid_range)
    );
    let _ = writeln!(
        out,
        "    group: {}, wheel: {wheel}, wheelGroup: {wheel_group},",
        ts_string(&variants::name(group))
    );
    let _ = writeln!(out, "  }},");
}

const HEADER: &str = "\
//
// Source of truth: crates/telemetry-model (registry.rs, channel.rs, value.rs, units.rs, codec.rs), D-017 and
// D-023. Every table is produced from the compiled Rust definitions by tools/gen-ts; the `ch` constant names
// are read from registry.rs and checked against the compiled registry. A gen-ts test fails when this file is
// stale.
";

const CHANNEL_DEF_INTERFACE: &str = "\
/** Static definition of a channel (`telemetry_model::channel::ChannelDef`, spec §12). */
export interface ChannelDef {
  /** Stable numeric channel id (`ChannelId`), append-only and never reused (D-017). */
  readonly id: number;
  /** Name of the Rust `ch::*` constant, which is also the key in `ch` below. */
  readonly constName: ChannelConstName;
  /** Canonical dotted name, e.g. `vehicle.speed_mps`. */
  readonly name: string;
  /** Export / MoTeC-style name, e.g. `Speed`, `TyrePressure_FL`. */
  readonly exportName: string;
  /** Short human description for UI labels. */
  readonly label: string;
  /** Internal (SI) unit of the stored value. */
  readonly unit: Unit;
  /** `Unit::symbol()` of `unit` ('' for dimensionless units). */
  readonly unitSymbol: string;
  /** Wire/storage value type; every VALID sample of the channel carries exactly this type. */
  readonly valueType: ValueType;
  /** Where the value comes from. */
  readonly source: Source;
  /** Measured / derived / estimated classification shown to the user (spec §107). */
  readonly provenance: Provenance;
  /** Network priority class. */
  readonly priority: Priority;
  /** Nominal update class (actual rates are measured at runtime). */
  readonly frequency: FrequencyClass;
  /** Whether display code may interpolate between samples (`Discrete` channels never are). */
  readonly interpolation: Interpolation;
  /** Static availability, independent of the car driven. */
  readonly availability: Availability;
  /** Exact origin: an LMU path, `formula:<id>`, `app:<id>` or `unavailable:<where>`. */
  readonly origin: string;
  /** Inclusive plausibility range in internal units (bounds may be infinite), or null. */
  readonly validRange: readonly [number, number] | null;
  /** Largest plausible change per second in internal units, or null. */
  readonly maxRatePerS: number | null;
  /** Id range group (the id's high byte). */
  readonly group: ChannelGroup;
  /** Wheel of a per-wheel channel, or null. */
  readonly wheel: Wheel | null;
  /** Ids of the per-wheel group (FL, FR, RL, RR) this channel belongs to, or null. */
  readonly wheelGroup: readonly [number, number, number, number] | null;
}

";

/// The complete `channels.ts` source.
pub fn channels_ts() -> String {
    let mut out = String::new();
    out.push_str(GENERATED_BANNER);
    out.push('\n');
    out.push_str(HEADER);
    out.push('\n');

    let _ = writeln!(
        out,
        "/** `telemetry_model::TELEMETRY_SCHEMA_VERSION`: version of the canonical channel set and semantics. */"
    );
    let _ = writeln!(
        out,
        "export const TELEMETRY_SCHEMA_VERSION = {TELEMETRY_SCHEMA_VERSION};"
    );
    let _ = writeln!(
        out,
        "/** `telemetry_model::value::MAX_STR_VALUE_BYTES`: maximum UTF-8 length of a string value. */"
    );
    let _ = writeln!(
        out,
        "export const MAX_STR_VALUE_BYTES = {MAX_STR_VALUE_BYTES};"
    );
    let _ = writeln!(
        out,
        "/** `telemetry_model::codec::TLV_HEADER_BYTES`: size of the fixed TLV block header. */"
    );
    let _ = writeln!(out, "export const TLV_HEADER_BYTES = {TLV_HEADER_BYTES};");
    let _ = writeln!(
        out,
        "/** `telemetry_model::codec::MAX_RECORDS`: most records one TLV block can hold. */"
    );
    let _ = writeln!(out, "export const TLV_MAX_RECORDS = {MAX_RECORDS};");
    let _ = writeln!(
        out,
        "/** `telemetry_model::codec::RELAY_NS_UNKNOWN`: `capture_relay_ns_est` sentinel for \"unknown\". */"
    );
    let _ = writeln!(out, "export const RELAY_NS_UNKNOWN = {RELAY_NS_UNKNOWN}n;");
    out.push('\n');

    wire_enum::<ValueType>(
        &mut out,
        ("ValueType", "ALL_VALUE_TYPES"),
        "telemetry_model::value::ValueType",
        "wire value type (low 5 bits of the TLV tag byte).",
        |v| v as u8,
        ValueType::from_u8,
    );
    wire_enum::<Quality>(
        &mut out,
        ("Quality", "ALL_QUALITIES"),
        "telemetry_model::value::Quality",
        "per-sample quality (high 3 bits of the TLV tag byte).",
        |q| q as u8,
        Quality::from_u8,
    );
    string_enum::<Unit>(
        &mut out,
        "Unit",
        "telemetry_model::units::Unit",
        "internal (SI) unit of a channel.",
        "ALL_UNITS",
    );
    let _ = writeln!(
        out,
        "/** `Unit::symbol()`: short symbol of each internal unit ('' for dimensionless units). */"
    );
    let _ = writeln!(
        out,
        "export const UNIT_SYMBOLS: Readonly<Record<Unit, string>> = {{"
    );
    for unit in variants::all::<Unit>() {
        let _ = writeln!(
            out,
            "  {}: {},",
            variants::name(unit),
            ts_string(unit.symbol())
        );
    }
    let _ = writeln!(out, "}};\n");

    string_enum::<Source>(
        &mut out,
        "Source",
        "telemetry_model::channel::Source",
        "where a channel's value comes from.",
        "ALL_SOURCES",
    );
    string_enum::<Provenance>(
        &mut out,
        "Provenance",
        "telemetry_model::channel::Provenance",
        "measured vs calculated classification (spec §107).",
        "ALL_PROVENANCES",
    );
    string_enum::<Priority>(
        &mut out,
        "Priority",
        "telemetry_model::channel::Priority",
        "network priority class.",
        "ALL_PRIORITIES",
    );
    string_enum::<FrequencyClass>(
        &mut out,
        "FrequencyClass",
        "telemetry_model::channel::FrequencyClass",
        "nominal update class.",
        "ALL_FREQUENCY_CLASSES",
    );
    string_enum::<Interpolation>(
        &mut out,
        "Interpolation",
        "telemetry_model::channel::Interpolation",
        "continuous (may be interpolated for display) or discrete (never interpolated).",
        "ALL_INTERPOLATIONS",
    );
    string_enum::<Availability>(
        &mut out,
        "Availability",
        "telemetry_model::channel::Availability",
        "static availability of a channel.",
        "ALL_AVAILABILITIES",
    );
    string_enum::<ChannelGroup>(
        &mut out,
        "ChannelGroup",
        "telemetry_model::registry::ChannelGroup",
        "id range group (the id's high byte).",
        "ALL_CHANNEL_GROUPS",
    );
    let _ = writeln!(out, "/** `ChannelGroup::title()`. */");
    let _ = writeln!(
        out,
        "export const CHANNEL_GROUP_TITLES: Readonly<Record<ChannelGroup, string>> = {{"
    );
    for group in variants::all::<ChannelGroup>() {
        let _ = writeln!(
            out,
            "  {}: {},",
            variants::name(group),
            ts_string(group.title())
        );
    }
    let _ = writeln!(out, "}};");
    let _ = writeln!(
        out,
        "/** `ChannelGroup::number()`: the high byte shared by every id in the group. */"
    );
    let _ = writeln!(
        out,
        "export const CHANNEL_GROUP_NUMBERS: Readonly<Record<ChannelGroup, number>> = {{"
    );
    for group in variants::all::<ChannelGroup>() {
        let _ = writeln!(
            out,
            "  {}: 0x{:02X},",
            variants::name(group),
            group.number()
        );
    }
    let _ = writeln!(out, "}};\n");

    string_enum::<Wheel>(
        &mut out,
        "Wheel",
        "telemetry_model::registry::Wheel",
        "wheel position in LMU `mWheel[]` order.",
        "ALL_WHEELS",
    );
    let _ = writeln!(out, "/** `Wheel::suffix()`: export-name suffix (`FL`). */");
    let _ = writeln!(
        out,
        "export const WHEEL_SUFFIXES: Readonly<Record<Wheel, string>> = {{"
    );
    for wheel in variants::all::<Wheel>() {
        let _ = writeln!(
            out,
            "  {}: {},",
            variants::name(wheel),
            ts_string(wheel.suffix())
        );
    }
    let _ = writeln!(out, "}};");
    let _ = writeln!(
        out,
        "/** `Wheel::index()`: `mWheel[]` index and per-wheel id offset. */"
    );
    let _ = writeln!(
        out,
        "export const WHEEL_INDICES: Readonly<Record<Wheel, number>> = {{"
    );
    for wheel in variants::all::<Wheel>() {
        let _ = writeln!(out, "  {}: {},", variants::name(wheel), wheel.index());
    }
    let _ = writeln!(out, "}};\n");

    out.push_str(CHANNEL_DEF_INTERFACE);

    let names = constant_names();
    let _ = writeln!(
        out,
        "/** Every channel definition, sorted by id (`registry::all()`). A channel's position in this array is its"
    );
    let _ = writeln!(out, " * registry index. */");
    let _ = writeln!(out, "export const CHANNELS: readonly ChannelDef[] = [");
    for def in registry::all() {
        let konst = names
            .get(&def.id.0)
            .unwrap_or_else(|| panic!("no `ch::*` constant for 0x{:04X}", def.id.0));
        channel_entry(&mut out, def, konst);
    }
    let _ = writeln!(out, "];\n");

    let _ = writeln!(
        out,
        "/** Channel id constants, named like the Rust `ch::*` constants (`ch.SPEED`). */"
    );
    let _ = writeln!(out, "export const ch = {{");
    for def in registry::all() {
        if let Some(konst) = names.get(&def.id.0) {
            let _ = writeln!(out, "  {konst}: 0x{:04X},", def.id.0);
        }
    }
    let _ = writeln!(out, "}} as const;");
    let _ = writeln!(out, "/** Name of a `ch` constant. */");
    let _ = writeln!(out, "export type ChannelConstName = keyof typeof ch;\n");

    out.push_str(
        "\
// The lookup maps are annotated as pure so that a bundle importing only the enum tables (e.g. lib/units.ts)
// drops them, and with them the channel table.
/** Channel definitions by id. */
export const CHANNEL_BY_ID: ReadonlyMap<number, ChannelDef> = /* @__PURE__ */ new Map(
  /* @__PURE__ */ CHANNELS.map((def) => [def.id, def] as const),
);
/** Channel definitions by canonical name (`vehicle.speed_mps`). */
export const CHANNEL_BY_NAME: ReadonlyMap<string, ChannelDef> = /* @__PURE__ */ new Map(
  /* @__PURE__ */ CHANNELS.map((def) => [def.name, def] as const),
);
/** Channel definitions by export name (`Speed`, `TyrePressure_FL`). */
export const CHANNEL_BY_EXPORT_NAME: ReadonlyMap<string, ChannelDef> = /* @__PURE__ */ new Map(
  /* @__PURE__ */ CHANNELS.map((def) => [def.exportName, def] as const),
);
",
    );
    out
}

#[cfg(test)]
mod tests {
    use telemetry_model::ch;

    use super::*;

    #[test]
    fn parses_constant_names_for_every_channel() {
        let names = constant_names();
        assert_eq!(names.get(&ch::SPEED.0).map(String::as_str), Some("SPEED"));
        assert_eq!(
            names.get(&ch::TYRE_PRESSURE_FL.0).map(String::as_str),
            Some("TYRE_PRESSURE_FL")
        );
        assert_eq!(
            names.get(&ch::VEHICLE_NAME.0).map(String::as_str),
            Some("VEHICLE_NAME")
        );
        assert_eq!(names.len(), registry::len());
    }

    #[test]
    fn registry_line_parser_rejects_non_entries() {
        assert!(parse_registry_line("    // ===== 0x0001-0x00FF vehicle =====").is_none());
        assert!(parse_registry_line("const MAX_SPEED_MPS: f64 = 150.0;").is_none());
        let entry = parse_registry_line(
            r#"    SPEED = 0x0001, "vehicle.speed_mps", "Speed" => calc("Speed", U::MetrePerSecond)"#,
        )
        .expect("entry");
        assert_eq!(
            (entry.konst, entry.id, entry.name, entry.export_name),
            ("SPEED", 1, "vehicle.speed_mps", "Speed")
        );
    }

    #[test]
    fn wheel_placement_follows_the_registry_groups() {
        let fl = wheel_placement(ch::TYRE_PRESSURE_FL).expect("per-wheel channel");
        assert_eq!(fl.wheel, Wheel::FrontLeft);
        let rr_id = ChannelId(ch::TYRE_PRESSURE_FL.0 + 3);
        let rr = wheel_placement(rr_id).expect("per-wheel channel");
        assert_eq!(rr.wheel, Wheel::RearRight);
        assert_eq!(rr.group, fl.group);
        assert_eq!(wheel_placement(ch::SPEED), None);
    }

    #[test]
    fn literals_round_trip() {
        for v in [
            0.0,
            -0.0,
            150.0,
            1.0e-3,
            1.0 / 8_192.0,
            200.0 - 273.15,
            1.0e7,
            1.0e16,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let literal = ts_number(v);
            if v.is_finite() {
                let back: f64 = literal.parse().expect("parse");
                assert_eq!(back.to_bits(), v.to_bits(), "{literal}");
            }
        }
        assert_eq!(ts_number(150.0), "150");
        assert_eq!(ts_string("it's \\ \n"), r"'it\'s \\ \n'");
    }
}
