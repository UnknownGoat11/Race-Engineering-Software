//! Registry integrity tests against the repository docs:
//! - `docs/CHANNEL_IDS.md` is generated from the registry (golden file; `LMURE_BLESS=1` regenerates it);
//! - ids, canonical names and export names are unique;
//! - every measured shared-memory origin names a field that exists in `docs/research/lmu_abi_layout.json`
//!   (the ABI facts compiled from the installed SDK headers), which proves no LMU field is fabricated.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value as Json;
use telemetry_model::channel::{Availability, Provenance};
use telemetry_model::registry;
use telemetry_model::{ChannelDef, ValueType};

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

/// Value types compatible with an ABI leaf kind (the registry may widen/narrow numerics, but strings must
/// come from char arrays and booleans from bool/u8 fields).
fn type_compatible(def: &ChannelDef, kind: &str) -> bool {
    match def.value_type {
        ValueType::Str => kind == "char",
        ValueType::Bool => matches!(kind, "bool" | "u8"),
        _ => kind != "char",
    }
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
