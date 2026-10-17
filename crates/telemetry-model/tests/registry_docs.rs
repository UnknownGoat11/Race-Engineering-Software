//! Registry integrity tests against the repository docs:
//! - `docs/CHANNEL_IDS.md` is generated from the registry (golden file; `LMURE_BLESS=1` regenerates it);
//! - ids, canonical names and export names are unique;
//! - every measured shared-memory origin names a field that exists in `docs/research/lmu_abi_layout.json`
//!   (the ABI facts compiled from the installed SDK headers), which proves no LMU field is fabricated, and its
//!   wire type holds every value of that field's kind;
//! - `docs/LMU_CHANNEL_MAP.md` and the registry agree: every map row is a registry channel (or a documented
//!   rename / deliberate exclusion), and every map rule that requires a value above 0 rejects 0.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value as Json;
use telemetry_model::channel::{Availability, ChannelId, Provenance};
use telemetry_model::{ChannelDef, ValueType, ch, registry, validate};

/// Environment variable that regenerates the golden file instead of comparing it.
const BLESS_ENV: &str = "LMURE_BLESS";
/// Vehicle slot count of the `telemInfo[]` / `vehScoringInfo[]` arrays; `[player]` indexes these.
const VEHICLE_SLOTS: u64 = 104;
/// Root struct of the shared-memory object (`generic`, `paths`, `scoring`, `telemetry`).
const ROOT_STRUCT: &str = "SharedMemoryObjectOut";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn channel_ids_doc_is_up_to_date() {
    let path = repo_root().join("docs").join("CHANNEL_IDS.md");
    let generated = registry::channel_ids_markdown();
    if std::env::var(BLESS_ENV).is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write docs/CHANNEL_IDS.md");
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == generated,
        "docs/CHANNEL_IDS.md is out of date with the registry. Regenerate it with:\n  \
         PowerShell: $env:{BLESS_ENV}='1'; cargo test -p telemetry-model --test registry_docs; \
         Remove-Item Env:{BLESS_ENV}\n  bash: {BLESS_ENV}=1 cargo test -p telemetry-model --test registry_docs\n\
         then review and commit the diff."
    );
}

#[test]
fn ids_and_names_are_unique() {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut exports = HashSet::new();
    for d in registry::all() {
        assert!(ids.insert(d.id), "duplicate id 0x{:04X}", d.id.0);
        assert!(names.insert(d.name), "duplicate canonical name {}", d.name);
        // Case-insensitive: CSV/MoTeC consumers may fold case.
        assert!(
            exports.insert(d.export_name.to_ascii_lowercase()),
            "duplicate export name {}",
            d.export_name
        );
    }
    assert!(
        registry::len() > 300,
        "registry unexpectedly small: {}",
        registry::len()
    );
}

/// One path segment: field name plus its index expressions (`mWheel[0]` → (`mWheel`, ["0"])).
fn parse_segment(segment: &str) -> Result<(&str, Vec<&str>), String> {
    let (name, mut rest) = match segment.find('[') {
        Some(i) => (&segment[..i], &segment[i..]),
        None => (segment, ""),
    };
    let mut indices = Vec::new();
    while !rest.is_empty() {
        let close = rest
            .find(']')
            .ok_or_else(|| format!("unclosed index in {segment}"))?;
        if !rest.starts_with('[') {
            return Err(format!("bad index syntax in {segment}"));
        }
        indices.push(&rest[1..close]);
        rest = &rest[close + 1..];
    }
    if name.is_empty() {
        return Err(format!("empty field name in {segment}"));
    }
    Ok((name, indices))
}

/// Resolves an origin path through the ABI layout; returns the leaf field's `kind`.
fn resolve(layout: &Json, path: &str) -> Result<String, String> {
    let structs = &layout["structs"];
    let mut current = ROOT_STRUCT.to_owned();
    let segments: Vec<&str> = path.split('.').collect();
    for (n, segment) in segments.iter().enumerate() {
        let (name, indices) = parse_segment(segment)?;
        let fields = structs[current.as_str()]["fields"]
            .as_array()
            .ok_or_else(|| format!("struct {current} not in layout"))?;
        let field = fields
            .iter()
            .find(|f| f["name"] == name)
            .ok_or_else(|| format!("{current} has no field {name} (path {path})"))?;
        let dims: Vec<u64> = field["dims"]
            .as_array()
            .map(|a| a.iter().filter_map(Json::as_u64).collect())
            .unwrap_or_default();
        let kind = field["kind"].as_str().unwrap_or_default().to_owned();
        let is_last = n + 1 == segments.len();
        // A whole char array (string) may be read without an index.
        let string_leaf = is_last && kind == "char" && indices.len() + 1 == dims.len();
        if indices.len() != dims.len() && !string_leaf {
            return Err(format!(
                "{path}: {name} has dims {dims:?} but the path gives {} index(es)",
                indices.len()
            ));
        }
        for (index, dim) in indices.iter().zip(&dims) {
            match *index {
                "player" if *dim == VEHICLE_SLOTS => {}
                other => {
                    let i: u64 = other
                        .parse()
                        .map_err(|_| format!("{path}: bad index {other}"))?;
                    if i >= *dim {
                        return Err(format!("{path}: index {i} out of bounds {dim}"));
                    }
                }
            }
        }
        if is_last {
            if kind == "struct" {
                return Err(format!("{path} ends on a struct"));
            }
            return Ok(kind);
        }
        if kind != "struct" {
            return Err(format!("{path}: {name} is a {kind}, not a struct"));
        }
        let ty = field["type"].as_str().unwrap_or_default();
        let ty = ty
            .strip_prefix("struct ")
            .ok_or_else(|| format!("{path}: unexpected type {ty}"))?;
        current = ty.split('[').next().unwrap_or(ty).trim().to_owned();
    }
    Err(format!("empty path {path}"))
}

/// Wire types that hold every value of an ABI leaf kind after unit conversion only, so a measured channel can
/// never truncate, wrap or reinterpret its LMU field (GEAR stored as U8 would turn reverse, −1, into 255).
/// Doubles may be stored as F32 by design (registry module docs), and `u8` flag fields as BOOL (≠ 0 → true,
/// LMU_CHANNEL_MAP).
fn compatible_wire_types(kind: &str) -> &'static [ValueType] {
    use ValueType as T;
    match kind {
        "bool" => &[T::Bool],
        "char" => &[T::Str],
        "enum:u8" => &[T::U8],
        "enum:u32" => &[T::U32, T::U64],
        "u8" => &[T::U8, T::U32, T::I32, T::U64, T::Bool],
        "i8" => &[T::I8, T::I32],
        "i16" => &[T::I32],
        "u16" => &[T::I32, T::U32, T::U64],
        "i32" => &[T::I32],
        "u32" => &[T::U32, T::U64],
        "u64" => &[T::U64],
        "f32" | "f64" => &[T::F32, T::F64],
        _ => &[],
    }
}

/// Measured channels whose stored type deliberately differs from the field kind because of a documented
/// conversion: `(channel, ABI kind, stored type)`. Exact, so the list cannot hide any other mismatch.
const CONVERTED_CHANNELS: &[(ChannelId, &str, ValueType)] = &[
    // mStaticUndeflectedRadius: u8 centimetres → metres (A-LMU-060).
    (ch::TYRE_RADIUS_FL, "u8", ValueType::F32),
    (ch::TYRE_RADIUS_FR, "u8", ValueType::F32),
    (ch::TYRE_RADIUS_RL, "u8", ValueType::F32),
    (ch::TYRE_RADIUS_RR, "u8", ValueType::F32),
];

fn type_compatible(def: &ChannelDef, kind: &str) -> bool {
    compatible_wire_types(kind).contains(&def.value_type)
        || CONVERTED_CHANNELS.contains(&(def.id, kind, def.value_type))
}

#[test]
fn abi_kind_compatibility_is_strict() {
    let gear = registry::by_id(ch::GEAR).expect("gear");
    assert!(type_compatible(gear, "i32"));
    let mut narrowed = *gear;
    narrowed.value_type = ValueType::U8;
    assert!(
        !type_compatible(&narrowed, "i32"),
        "U8 cannot hold the i32 mGear (reverse is -1)"
    );
    narrowed.value_type = ValueType::Str;
    assert!(!type_compatible(&narrowed, "i32"));
    let radius = registry::by_id(ch::TYRE_RADIUS_FL).expect("radius");
    assert!(
        type_compatible(radius, "u8"),
        "documented cm → m conversion"
    );
    assert!(
        !type_compatible(radius, "i16"),
        "the conversion allowance is exact"
    );
}

#[test]
fn measured_origins_exist_in_the_abi_layout() {
    let path = repo_root()
        .join("docs")
        .join("research")
        .join("lmu_abi_layout.json");
    let text = std::fs::read_to_string(&path).expect("read docs/research/lmu_abi_layout.json");
    let layout: Json = serde_json::from_str(&text).expect("parse lmu_abi_layout.json");
    let mut checked = 0;
    let mut errors = Vec::new();
    for d in registry::all() {
        if !d.source.is_lmu_shared_memory() || d.availability == Availability::NotProvided {
            continue;
        }
        assert_eq!(
            d.provenance,
            Provenance::Measured,
            "{}: shared-memory channels are measured",
            d.name
        );
        match resolve(&layout, d.origin) {
            Ok(kind) if type_compatible(d, &kind) => checked += 1,
            Ok(kind) => errors.push(format!(
                "{}: {:?} is not compatible with ABI kind {kind}",
                d.name, d.value_type
            )),
            Err(e) => errors.push(format!("{}: {e}", d.name)),
        }
    }
    assert!(
        errors.is_empty(),
        "origins not found in the ABI layout:\n{}",
        errors.join("\n")
    );
    assert!(checked > 200, "only {checked} measured origins checked");
}

#[test]
fn resolver_rejects_fabricated_paths() {
    let path = repo_root()
        .join("docs")
        .join("research")
        .join("lmu_abi_layout.json");
    let layout: Json =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse");
    assert!(resolve(&layout, "telemetry.telemInfo[player].mWheel[0].mPressure").is_ok());
    assert!(
        resolve(&layout, "telemetry.telemInfo[player].mWheel[4].mPressure").is_err(),
        "index out of bounds"
    );
    assert!(
        resolve(
            &layout,
            "telemetry.telemInfo[player].mWheel[0].mBrakeLinePressure"
        )
        .is_err(),
        "no such field"
    );
    assert!(
        resolve(&layout, "telemetry.telemInfo[player].mWheel.mPressure").is_err(),
        "missing index"
    );
    assert!(
        resolve(&layout, "scoring.scoringInfo.mWind").is_err(),
        "ends on a struct"
    );
    assert!(
        resolve(&layout, "scoring.scoringInfo.mTrackName").is_ok(),
        "string leaf"
    );
}

// ---------------------------------------------------------------------------------------------------------
// docs/LMU_CHANNEL_MAP.md
// ---------------------------------------------------------------------------------------------------------

/// Header cell that opens a channel table in LMU_CHANNEL_MAP.md.
const MAP_TABLE_HEADER: &str = "| Internal Name |";
/// Cells per channel-table row (Internal Name … Notes).
const MAP_COLUMNS: usize = 12;
/// Index of the Conversion cell.
const MAP_CONVERSION_COLUMN: usize = 9;
/// Index of the Validation cell.
const MAP_VALIDATION_COLUMN: usize = 10;
/// Marker of the Kelvin → Celsius conversion: its offset moves raw 0, so a positive raw lower bound says
/// nothing about an internal 0 (0 °C is a valid tyre temperature).
const MAP_KELVIN_OFFSET_MARKER: &str = "273.15";
/// Validation cell meaning "same rule as the row above".
const MAP_SAME_RULE: &str = "same";
/// Map names of per-car standings fields (`scoring::CompetitorEntry`), not registry channels.
const MAP_PER_CAR_PREFIX: &str = "cars.";
/// Wheel tokens substituted for `{w}` (LMU `mWheel[0..4]` order).
const WHEEL_TOKENS: [&str; 4] = ["fl", "fr", "rl", "rr"];

/// One channel-table row of the map.
struct MapRow {
    /// 1-based line number, for messages.
    line: usize,
    /// Internal names with `{w}`, `{x,y,z}` and `{0..7}` patterns expanded and relative names completed.
    names: Vec<String>,
    /// Conversion from the raw value.
    conversion: String,
    /// Validation rule ("same" resolved to the rule of the row above).
    validation: String,
}

/// Expands every `{…}` pattern of a map name: `{w}` → the four wheels, `{a,b}` → each item, `{0..7}` → the
/// inclusive integer range.
fn expand_braces(name: &str) -> Vec<String> {
    let Some(open) = name.find('{') else {
        return vec![name.to_owned()];
    };
    let close = open
        + name[open..]
            .find('}')
            .unwrap_or_else(|| panic!("unclosed brace in map name {name}"));
    let (head, body, tail) = (&name[..open], &name[open + 1..close], &name[close + 1..]);
    let options: Vec<String> = if body == "w" {
        WHEEL_TOKENS.iter().map(|w| (*w).to_owned()).collect()
    } else if let Some((first, last)) = body.split_once("..") {
        let first: u32 = first.parse().expect("range start");
        let last: u32 = last.parse().expect("range end");
        (first..=last).map(|i| i.to_string()).collect()
    } else {
        body.split(',').map(|s| s.trim().to_owned()).collect()
    };
    options
        .iter()
        .flat_map(|o| expand_braces(&format!("{head}{o}{tail}")))
        .collect()
}

/// Names of an Internal Name cell: every code span, a relative one (no `.`) completed with the first name's
/// parent path (`brakes.{w}.wear_frac` / `thickness_m` → `brakes.{w}.thickness_m`), then expanded.
fn cell_names(cell: &str) -> Vec<String> {
    let spans: Vec<&str> = cell.split('`').skip(1).step_by(2).collect();
    let parent = spans
        .first()
        .and_then(|first| first.rfind('.').map(|i| &first[..=i]))
        .unwrap_or_default();
    spans
        .iter()
        .flat_map(|span| {
            let full = if span.contains('.') {
                (*span).to_owned()
            } else {
                format!("{parent}{span}")
            };
            expand_braces(&full)
        })
        .collect()
}

/// Every channel-table row of docs/LMU_CHANNEL_MAP.md.
fn channel_map_rows() -> Vec<MapRow> {
    let path = repo_root().join("docs").join("LMU_CHANNEL_MAP.md");
    let text = std::fs::read_to_string(&path).expect("read docs/LMU_CHANNEL_MAP.md");
    let mut rows = Vec::new();
    let mut in_table = false;
    let mut previous_rule = String::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if line.starts_with(MAP_TABLE_HEADER) {
            in_table = true;
            continue;
        }
        if !line.starts_with('|') {
            in_table = false;
            continue;
        }
        if !in_table || !line.starts_with("| `") {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        assert_eq!(
            cells.len(),
            MAP_COLUMNS,
            "LMU_CHANNEL_MAP.md:{line_no}: expected {MAP_COLUMNS} cells"
        );
        let mut validation = cells[MAP_VALIDATION_COLUMN].to_owned();
        if validation == MAP_SAME_RULE {
            validation.clone_from(&previous_rule);
        }
        previous_rule.clone_from(&validation);
        rows.push(MapRow {
            line: line_no,
            names: cell_names(cells[0]),
            conversion: cells[MAP_CONVERSION_COLUMN].to_owned(),
            validation,
        });
    }
    rows
}

/// True when a map row excludes an internal 0: "> 0", "> `other_time`", "0 < x …", "(0, …]", or a range
/// "a..b" with a > 0 ("0.2..0.8", "20..60 cm raw") under a conversion that keeps 0 at 0 (a scale; the
/// Kelvin offset is excluded).
fn requires_above_zero(rule: &str, conversion: &str) -> bool {
    let positive_range_start = rule
        .split_once("..")
        .and_then(|(start, _)| start.trim().parse::<f64>().ok())
        .is_some_and(|start| start > 0.0)
        && !conversion.contains(MAP_KELVIN_OFFSET_MARKER);
    rule.starts_with("> ")
        || rule.contains("0 < ")
        || rule.starts_with("(0,")
        || positive_range_start
}

#[test]
fn map_parser_understands_the_name_patterns() {
    assert_eq!(
        cell_names("`tyres.{w}.pressure_pa`"),
        [
            "tyres.fl.pressure_pa",
            "tyres.fr.pressure_pa",
            "tyres.rl.pressure_pa",
            "tyres.rr.pressure_pa"
        ]
    );
    assert_eq!(
        cell_names("`brakes.{w}.wear_frac` / `thickness_m`").len(),
        8
    );
    assert_eq!(
        cell_names("`vehicle.gps_lat_deg` / `gps_lon_deg`"),
        ["vehicle.gps_lat_deg", "vehicle.gps_lon_deg"]
    );
    assert_eq!(cell_names("`damage.dent_severity_{0..7}`").len(), 8);
    assert_eq!(
        cell_names("`vehicle.local_vel_{x,y,z}_mps`")[2],
        "vehicle.local_vel_z_mps"
    );
    let none = "none";
    assert!(requires_above_zero("(0, 0.5]", none));
    assert!(requires_above_zero("0 < x ≤ 1440°", "assumed degrees"));
    assert!(requires_above_zero("> `best_sector1`", "≤ 0 → NONE"));
    assert!(requires_above_zero("0.2..0.8", "display front = 1 − x"));
    assert!(requires_above_zero("20..60 cm raw", "cm ÷ 100"));
    assert!(!requires_above_zero("200..700 K raw", "K − 273.15"));
    assert!(!requires_above_zero("−0.01..1.01", none));
    assert!(!requires_above_zero("0..1", none));
    assert!(!requires_above_zero("≥ 0", none));
    assert!(!requires_above_zero(
        "each abs < 490 (50 g); > 60 m/s² outside an impact is SUSPECT",
        none
    ));
}

/// Review finding: the registry let 0 through as VALID where the map rules it out (rear brake bias,
/// steering wheel ranges, update interval). Every "above 0" rule of the map must reject 0, so the registry
/// cannot drift from the map again.
#[test]
fn map_rules_above_zero_reject_zero() {
    let mut checked = 0;
    let mut errors = Vec::new();
    for row in channel_map_rows() {
        if !requires_above_zero(&row.validation, &row.conversion) {
            continue;
        }
        for name in &row.names {
            // Per-car fields and renamed rows are covered by `map_rows_are_registry_channels`.
            let Some(def) = registry::by_name(name) else {
                continue;
            };
            checked += 1;
            if validate::check_value(def, 0.0).is_ok() {
                errors.push(format!(
                    "LMU_CHANNEL_MAP.md:{}: {name} accepts 0, the map requires `{}`",
                    row.line, row.validation
                ));
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    // 28 registry channels carry such a rule today: game version, update interval, steering wheel ranges,
    // lap and sector times, track length, gear count, rev limit, rear brake bias, tyre radii and positions.
    assert!(
        checked >= 28,
        "only {checked} above-zero rules checked: is the map parser broken?"
    );
}

/// Review finding: CHANNEL_IDS.md claimed the canonical names match the map, but several rows differ and
/// three map rows have no channel. Every map row (other than per-car standings fields) must be a registry
/// channel, a documented rename or a documented exclusion, and both exception lists must stay current.
#[test]
fn map_rows_are_registry_channels() {
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let rows = channel_map_rows();
    for row in &rows {
        for name in &row.names {
            seen.insert(name.clone());
            if name.starts_with(MAP_PER_CAR_PREFIX) {
                continue;
            }
            let rename = registry::CHANNEL_MAP_RENAMES
                .iter()
                .find(|(map_name, _)| map_name == name);
            let unmapped = registry::CHANNEL_MAP_UNMAPPED
                .iter()
                .any(|(map_name, _)| map_name == name);
            let in_registry = registry::by_name(name).is_some();
            match (rename, unmapped, in_registry) {
                (Some((_, channels)), false, false) => {
                    for c in *channels {
                        if registry::by_name(c).is_none() {
                            errors.push(format!("rename target {c} of {name} is not a channel"));
                        }
                    }
                }
                (None, true, false) | (None, false, true) => {}
                _ => errors.push(format!(
                    "LMU_CHANNEL_MAP.md:{}: {name} must be exactly one of: a registry channel, a \
                     CHANNEL_MAP_RENAMES entry, a CHANNEL_MAP_UNMAPPED entry",
                    row.line
                )),
            }
        }
    }
    let exceptions = registry::CHANNEL_MAP_RENAMES
        .iter()
        .map(|(name, _)| *name)
        .chain(registry::CHANNEL_MAP_UNMAPPED.iter().map(|(name, _)| *name));
    for name in exceptions {
        if !seen.contains(name) {
            errors.push(format!("stale exception: {name} is no longer a map row"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(
        rows.len() > 200,
        "only {} map rows parsed: is the parser broken?",
        rows.len()
    );
}
