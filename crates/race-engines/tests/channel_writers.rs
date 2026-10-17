//! Writer coverage (BLUEPRINT I-2 / §14 Q5, D-010): every calculated registry channel (origin
//! `formula:<id>`) that is not experimental has **exactly one** writer, and experimental channels have none
//! until they are verified live. Writers:
//! - `lmu-normalizer`: `mapping::DERIVED_HERE`, the four wheels of `mapping::DERIVED_HERE_PER_WHEEL`, and
//!   `DERIVED_PER_STEP` (written by `Normalizer` itself);
//! - `race-engines`: `publish::OWNED_CHANNELS`, `energy::PUBLISHED_CHANNELS`, `publish::EXTENDED_CHANNELS`.
//!
//! The writers' own tests prove they write every listed channel on every call; this test proves the lists
//! partition the registry's calculated channels.

use std::collections::BTreeMap;

use lmu_normalizer::DERIVED_PER_STEP;
use lmu_normalizer::mapping::{DERIVED_HERE, DERIVED_HERE_PER_WHEEL};
use race_engines::energy::PUBLISHED_CHANNELS;
use race_engines::publish::{EXTENDED_CHANNELS, OWNED_CHANNELS};
use telemetry_model::channel::{Availability, Source};
use telemetry_model::registry::{self, Wheel};
use telemetry_model::{ChannelId, ch};

/// Prefix of a formula origin.
const FORMULA_PREFIX: &str = "formula:";

/// Every writer list, with a name for messages.
fn writers() -> Vec<(&'static str, Vec<ChannelId>)> {
    let mut per_wheel = Vec::new();
    for &fl in DERIVED_HERE_PER_WHEEL {
        let ids = registry::wheel_group(fl).expect("per-wheel FL id");
        per_wheel.extend(Wheel::ALL.map(|w| ids[w.index()]));
    }
    vec![
        ("lmu-normalizer DERIVED_HERE", DERIVED_HERE.to_vec()),
        ("lmu-normalizer DERIVED_HERE_PER_WHEEL", per_wheel),
        ("lmu-normalizer DERIVED_PER_STEP", DERIVED_PER_STEP.to_vec()),
        (
            "race-engines publish::OWNED_CHANNELS",
            OWNED_CHANNELS
                .iter()
                .copied()
                // The engine version is an application channel the publisher also writes.
                .filter(|&id| id != ch::CALC_ENGINE_VERSION)
                .collect(),
        ),
        (
            "race-engines energy::PUBLISHED_CHANNELS",
            PUBLISHED_CHANNELS.to_vec(),
        ),
        (
            "race-engines publish::EXTENDED_CHANNELS",
            EXTENDED_CHANNELS.to_vec(),
        ),
    ]
}

#[test]
fn every_calculated_channel_has_exactly_one_writer() {
    let mut count: BTreeMap<ChannelId, Vec<&'static str>> = BTreeMap::new();
    for (name, ids) in writers() {
        for id in ids {
            count.entry(id).or_default().push(name);
        }
    }
    let mut errors = Vec::new();
    let mut calculated = 0;
    for def in registry::all() {
        let formula = def.origin.starts_with(FORMULA_PREFIX);
        assert_eq!(
            formula,
            def.source == Source::Calculated,
            "{}: formula origins and Source::Calculated go together",
            def.name
        );
        if !formula {
            continue;
        }
        calculated += 1;
        let by = count.get(&def.id).cloned().unwrap_or_default();
        let expected = if def.availability == Availability::Experimental {
            0
        } else {
            1
        };
        if by.len() != expected {
            errors.push(format!(
                "{} (0x{:04X}, {:?}): {} writer(s) {by:?}, expected {expected}",
                def.name,
                def.id.0,
                def.availability,
                by.len()
            ));
        }
    }
    // Every listed channel is a calculated registry channel.
    for (id, by) in &count {
        match registry::by_id(*id) {
            Some(def) if def.source == Source::Calculated => {}
            Some(def) => errors.push(format!("{} listed by {by:?} is not calculated", def.name)),
            None => errors.push(format!("0x{:04X} listed by {by:?} is not registered", id.0)),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(
        calculated > 150,
        "only {calculated} calculated channels: registry changed?"
    );
}

#[test]
fn experimental_channels_stay_unwritten() {
    let listed: Vec<ChannelId> = writers().into_iter().flat_map(|(_, ids)| ids).collect();
    for id in [ch::SLIP_RATIO_FL, ch::SLIP_ANGLE_RR, ch::LOCAL_YELLOW_S1] {
        assert!(!listed.contains(&id), "0x{:04X} is experimental", id.0);
    }
}
