//! Text rendering of the live view and of the compatibility report.
//!
//! Every value is printed in its raw LMU unit first; tentative conversions (computed with
//! `telemetry_model::units::convert`, the normalizer's conversion functions) follow an arrow `->`.
//! Units the research marks as unverified are labelled `unit?`. Non-finite or absent values read
//! `UNAVAILABLE`.

use std::path::Path;

use lmu_reader::{
    CompatibilityReport, Outcome, PlayerIdentity, RawSnapshot, RawTelemetry, RawWheel,
    ReaderDiagnostics, SupportLevel, VehicleClass, split_current_sector,
};
use telemetry_model::units::{KELVIN_OFFSET, RAD_PER_DEG, convert};

use crate::fmt::{
    UNAVAILABLE, duration_ns, hz, lap_time, num, num32, opt, opt_duration_ns, safe_text, signed,
    yes_no,
};
use crate::worker::Status;

/// km/h per m/s (display only).
const KMH_PER_MPS: f64 = 3.6;
/// Width of one wheel column.
const WHEEL_COL: usize = 24;
/// Width of the row labels of the wheel table (longer labels would run into the first column).
const WHEEL_LABEL: usize = 20;
/// Wheel order of `mWheel[]` (A-LMU-019).
const WHEEL_NAMES: [&str; 4] = ["FL", "FR", "RL", "RR"];
/// Raw brake values below this are implausible as Kelvin (below −123 °C).
const BRAKE_ONLY_CELSIUS_BELOW: f64 = 150.0;
/// Raw brake values above this are implausible as °C for brake discs.
const BRAKE_ONLY_KELVIN_ABOVE: f64 = 1_500.0;
/// Cold brakes read within this many kelvin of ambient + 273.15 when the unit is Kelvin (A-LMU-015).
const BRAKE_AMBIENT_TOLERANCE_K: f64 = 25.0;
/// Quantiles shown for timing histograms.
const P50: f64 = 0.5;
const P99: f64 = 0.99;

struct Screen(String);

impl Screen {
    fn new() -> Self {
        Self(String::new())
    }

    fn line(&mut self, text: impl AsRef<str>) {
        self.0.push_str(text.as_ref());
        self.0.push('\n');
    }
}

fn level(l: Option<SupportLevel>) -> &'static str {
    l.map_or("NOT CHECKED", SupportLevel::as_str)
}

fn compat_lines(s: &mut Screen, report: &CompatibilityReport, verbose: bool) {
    s.line(format!(
        "Compat   overall {} | generic {} | scoring {} | telemetry {} | frames checked {} rejected {}",
        level(report.overall),
        level(report.generic),
        level(report.scoring),
        level(report.telemetry),
        report.frames_checked,
        report.frames_rejected,
    ));
    for check in &report.checks {
        let tag = match check.outcome {
            Some(Outcome::Fail) => "FAIL",
            Some(Outcome::Warn) => "WARN",
            Some(Outcome::Pass) if verbose => "pass",
            None if verbose => "n/a ",
            _ => {
                if let Some(problem) = &check.last_problem {
                    s.line(format!(
                        "         earlier: {} warned {}x / failed {}x, last: {problem}",
                        check.title, check.warn_frames, check.fail_frames
                    ));
                }
                continue;
            }
        };
        let expected = if check.expected.is_empty() {
            String::new()
        } else {
            format!(" [expected {}]", check.expected)
        };
        s.line(format!(
            "  {tag} {}: {}{expected}",
            check.title, check.detected
        ));
    }
}

/// The `lmu-diag compat` report.
pub(crate) fn render_compat(diag: &ReaderDiagnostics, report: &CompatibilityReport) -> String {
    let mut s = Screen::new();
    s.line("lmu-diag compat (read-only)");
    reader_state_line(&mut s, diag);
    s.line(format!(
        "Layout   {} from {} | mapping {} (need >= {} bytes)",
        report.probed_build,
        report.layout_source,
        report
            .mapping_size
            .map_or_else(|| "not mapped".to_owned(), |n| format!("{n} bytes")),
        report.mapping_required,
    ));
    s.line(format!(
        "         gameVersion {} (first seen {}) | {}",
        opt(report.game_version),
        opt(report.first_game_version),
        version_text(report.version_verified),
    ));
    if report.frames_checked == 0 && report.overall.is_none() {
        s.line(
            "Compat   NOT CHECKED: no frame could be read. Start LMU (get on track) and run again.",
        );
        return s.0;
    }
    compat_lines(&mut s, report, true);
    if report.overall == Some(SupportLevel::Unsupported) {
        s.line("");
        s.line(report.incompatibility_message());
    }
    s.0
}

fn version_text(verified: Option<bool>) -> &'static str {
    match verified {
        Some(true) => "version VERIFIED for this layout",
        Some(false) => "version NOT YET VERIFIED at runtime (A-LMU-010)",
        None => "version not read",
    }
}

fn identity_text(identity: PlayerIdentity) -> String {
    match identity {
        PlayerIdentity::NoVehicle => "no player vehicle".to_owned(),
        PlayerIdentity::Verified => "verified by mID".to_owned(),
        PlayerIdentity::FoundById { index_slot } => {
            format!("found by mID (playerVehicleIdx named slot {index_slot})")
        }
        PlayerIdentity::Unverified { reason } => format!("not verified ({reason:?})"),
    }
}

fn reader_state_line(s: &mut Screen, d: &ReaderDiagnostics) {
    s.line(format!(
        "LMU      state {} | pid {} | connection #{} up {} | sync {} | lock {}",
        d.state,
        opt(d.lmu_pid),
        d.connection_id,
        opt_duration_ns(d.connected_for_ns),
        d.sync_mode.map_or("n/a", |m| m.as_str()),
        d.lock_mode
            .as_ref()
            .map_or_else(|| "n/a".to_owned(), |m| m.label()),
    ));
    if let Some(reason) = &d.sync_reason {
        s.line(format!("         polling because: {reason}"));
    }
    if let Some(note) = &d.note {
        s.line(format!("         note: {note}"));
    }
    if let Some(error) = &d.last_error {
        s.line(format!("         last error: {error}"));
    }
}

/// Inputs of one live-view frame.
pub(crate) struct LiveInput<'a> {
    pub(crate) status: Option<&'a Status>,
    pub(crate) snapshot: Option<&'a RawSnapshot>,
    pub(crate) now_ns: u64,
    pub(crate) errors: &'a [String],
    pub(crate) log: Option<(&'a Path, u64)>,
}

/// One refresh of `lmu-diag live`.
pub(crate) fn render_live(input: &LiveInput<'_>) -> String {
    let mut s = Screen::new();
    s.line("lmu-diag live (read-only; Ctrl+C to quit)");
    match input.status {
        None => s.line("LMU      starting reader..."),
        Some(status) => status_lines(&mut s, status, input.now_ns),
    }
    match input.snapshot {
        None => s.line("Frame    no frame received yet"),
        Some(snapshot) => snapshot_lines(&mut s, snapshot, input.now_ns),
    }
    for error in input.errors {
        s.line(format!("Error    {error}"));
    }
    if let Some((path, frames)) = input.log {
        s.line(format!(
            "Log      {} frames written to {} (contains driver names)",
            frames,
            path.display()
        ));
    }
    s.0
}

fn status_lines(s: &mut Screen, status: &Status, now_ns: u64) {
    let d = &status.diagnostics;
    let st = &status.stats;
    reader_state_line(s, d);
    let c = &status.compat;
    s.line(format!(
        "Layout   mapping {} (need >= {}) | gameVersion {} | probed {}",
        c.mapping_size
            .map_or_else(|| "n/a".to_owned(), |n| format!("{n} bytes")),
        c.mapping_required,
        opt(c.game_version),
        c.probed_build,
    ));
    compat_lines(s, c, false);
    s.line(format!(
        "Rates    telemetry {} | scoring {} | gate wake-ups {} | snapshots {} | sample age {}",
        hz(st.telemetry_rate.rate_hz()),
        hz(st.scoring_rate.rate_hz()),
        hz(st.gate_rate.rate_hz()),
        hz(st.snapshot_rate.rate_hz()),
        opt_duration_ns(st.sample_age_ns(now_ns)),
    ));
    s.line(format!(
        "Frames   copies {} | snapshots {} | duplicates {} | generic-only {} | rejected {} | torn {} | ET regressions {} | UI drops {}",
        st.copies,
        st.snapshots,
        st.duplicate_frames,
        st.same_et_frames,
        st.rejected_frames,
        st.torn_copies,
        st.telemetry_et_regressions,
        status.dropped,
    ));
    s.line(format!(
        "Samples  skipped {} in {} gap(s) (ET step > 1.5 dt) | dt p50<={} p99<={} max {} | player found by mID {} unverified {}",
        st.telemetry_samples_skipped,
        st.telemetry_gaps,
        opt_duration_ns(st.delta_time.quantile_upper_ns(P50)),
        opt_duration_ns(st.delta_time.quantile_upper_ns(P99)),
        opt_duration_ns(st.delta_time.max_ns()),
        st.player_found_by_id_frames,
        st.player_unverified_frames,
    ));
    s.line(format!(
        "Gate     wake-ups {} new {} missed (witness) {} degraded {} | timeouts {} failures {} polls {} upgrades {} | wait mean {} p50<={} p99<={} max {}",
        st.gate_wakeups,
        st.gate_new_frames,
        st.gate_missed_updates,
        st.gate_degradations,
        st.gate_timeouts,
        st.gate_failures,
        st.polls,
        st.object_upgrades,
        st.gate_wait.mean_ns().map_or_else(|| "n/a".to_owned(), |m| duration_ns(m as u64)),
        opt_duration_ns(st.gate_wait.quantile_upper_ns(P50)),
        opt_duration_ns(st.gate_wait.quantile_upper_ns(P99)),
        opt_duration_ns(st.gate_wait.max_ns()),
    ));
    s.line(format!(
        "Lock     acquired {} timeouts {} failures {} | hold mean {} p50<={} p99<={} max {} | over 50 us {} | wait max {}",
        st.lock_acquired,
        st.lock_timeouts,
        st.lock_failures,
        st.lock_hold.mean_ns().map_or_else(|| "n/a".to_owned(), |m| duration_ns(m as u64)),
        opt_duration_ns(st.lock_hold.quantile_upper_ns(P50)),
        opt_duration_ns(st.lock_hold.quantile_upper_ns(P99)),
        opt_duration_ns(st.lock_hold.max_ns()),
        st.lock_hold_over_budget,
        opt_duration_ns(st.lock_wait.max_ns()),
    ));
    s.line(format!(
        "         lock words busy={} waiters={} | lock-free copies {} retries {}{}",
        opt(d.lock_busy_word),
        opt(d.lock_waiters_word),
        st.lock_free_copies,
        st.copy_retries,
        if d.lock_stuck {
            " | LOCK STUCK: held by another program, restart LMU (never reset by us)"
        } else {
            ""
        },
    ));
    let events: Vec<String> = st
        .event_code_occurrences
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(code, n)| format!("{code}:{n}"))
        .collect();
    s.line(format!(
        "Events   code:occurrences {} | unknown codes {} (queue of codes; 0 = SME_ENTER or empty slot)",
        if events.is_empty() { "none".to_owned() } else { events.join(" ") },
        st.unknown_event_codes,
    ));
}

fn game_phase_name(phase: u8) -> &'static str {
    match phase {
        0 => "before session",
        1 => "reconnaissance laps",
        2 => "grid walk-through",
        3 => "formation lap",
        4 => "starting-light countdown",
        5 => "green flag",
        6 => "full course yellow / safety car",
        7 => "session stopped",
        8 => "session over",
        9 => "paused",
        _ => "unknown",
    }
}

fn yellow_flag_name(state: i8) -> &'static str {
    match state {
        -1 => "invalid",
        0 => "none",
        1 => "pending",
        2 => "pits closed",
        3 => "pit lead lap",
        4 => "pits open",
        5 => "last lap",
        6 => "resume",
        7 => "race halt",
        _ => "unknown",
    }
}

fn motor_state_name(state: u8) -> &'static str {
    match state {
        0 => "unavailable",
        1 => "inactive",
        2 => "propulsion",
        3 => "regeneration",
        _ => "unknown",
    }
}

fn snapshot_lines(s: &mut Screen, snap: &RawSnapshot, now_ns: u64) {
    let info = &snap.scoring.info;
    s.line(format!(
        "Frame    seq {} | age {} | telemetry fresh {} | scoring fresh {} | support {} | {} | consistency {:?}",
        snap.seq,
        duration_ns(now_ns.saturating_sub(snap.capture_mono_ns)),
        yes_no(snap.telemetry_updated),
        yes_no(snap.scoring_updated),
        snap.support,
        version_text(Some(snap.version_verified)),
        snap.source.consistency,
    ));
    s.line(format!(
        "Session  track \"{}\" | session {} | phase {} {} | yellow flag {} {} | sector flags {:?} (1 yellow / 11 clear: CLAIMED, order unknown) | cars {}",
        safe_text(&info.track_name),
        info.session,
        info.game_phase,
        game_phase_name(info.game_phase),
        info.yellow_flag_state,
        yellow_flag_name(info.yellow_flag_state),
        info.sector_flag,
        snap.scoring.vehicles.len(),
    ));
    s.line(format!(
        "Weather  ambient {} C | track {} C | raining {} | wetness min {} avg {} max {} | dark cloud {}",
        num(info.ambient_temp, 1),
        num(info.track_temp, 1),
        num(info.raining, 2),
        num(info.min_path_wetness, 2),
        num(info.avg_path_wetness, 2),
        num(info.max_path_wetness, 2),
        num(info.dark_cloud, 2),
    ));
    let h = snap.telemetry_header;
    let Some(player) = &snap.player else {
        s.line(format!(
            "Player   {UNAVAILABLE}: {} (playerHasVehicle={} playerVehicleIdx={} activeVehicles={})",
            identity_text(snap.player_identity),
            h.player_has_vehicle,
            h.player_vehicle_idx,
            h.active_vehicles
        ));
        return;
    };
    let t = &player.telemetry;
    s.line(format!(
        "Player   slot {} ({}) | mID {} | \"{}\" | class enum {} ({:?}) | ET {} s | dt {} s | telemetry slots {}",
        player.slot,
        identity_text(snap.player_identity),
        t.id,
        safe_text(&t.vehicle_name),
        t.vehicle_class,
        VehicleClass::from_raw(t.vehicle_class),
        num(t.elapsed_time, 3),
        num(t.delta_time, 4),
        snap.vehicle_telemetry.len(),
    ));
    motion_lines(s, t);
    driver_lines(s, t);
    scoring_lines(s, snap, t);
    wheel_table(s, t, info.ambient_temp);
    s.line(hybrid_line(t));
    s.line(aids_line(t));
}

fn motion_lines(s: &mut Screen, t: &RawTelemetry) {
    let v = t.local_vel;
    let speed = convert::magnitude(v.x, v.y, v.z);
    s.line(format!(
        "Speed    |mLocalVel| {} m/s -> {} km/h (x {} y {} z {} m/s; +z = rear)",
        num(speed, 2),
        num(speed * KMH_PER_MPS, 1),
        signed(v.x, 2),
        signed(v.y, 2),
        signed(v.z, 2),
    ));
    // ISO-style signs from the header's axes (A-LMU-024): +x left, +y up, +z rear.
    let a = t.local_accel;
    s.line(format!(
        "G        lat {} g (+ left) | long {} g (+ accel) | vert {} g -> from mLocalAccel x {} y {} z {} m/s2 (gravity included? A-LMU-032)",
        signed(convert::accel_to_g(a.x), 2),
        signed(convert::accel_to_g(-a.z), 2),
        signed(convert::accel_to_g(a.y), 2),
        signed(a.x, 2),
        signed(a.y, 2),
        signed(a.z, 2),
    ));
    let yaw = -t.local_rot.y;
    s.line(format!(
        "Yaw      {} rad/s -> {} deg/s (+ left) from mLocalRot.y {} (+ yaws right)",
        signed(yaw, 3),
        signed(yaw / RAD_PER_DEG, 1),
        signed(t.local_rot.y, 3),
    ));
}

fn driver_lines(s: &mut Screen, t: &RawTelemetry) {
    s.line(format!(
        "Engine   rpm {} (max {}) | gear {} (-1 R, 0 N) | water {} C | oil {} C",
        num(t.engine_rpm, 0),
        num(t.engine_max_rpm, 0),
        t.gear,
        num(t.engine_water_temp, 1),
        num(t.engine_oil_temp, 1),
    ));
    s.line(format!(
        "Inputs   throttle f {} u {} | brake f {} u {} | steering f {} u {} | clutch f {} u {}",
        num(t.filtered_throttle, 3),
        num(t.unfiltered_throttle, 3),
        num(t.filtered_brake, 3),
        num(t.unfiltered_brake, 3),
        signed(t.filtered_steering, 3),
        signed(t.unfiltered_steering, 3),
        num(t.filtered_clutch, 3),
        num(t.unfiltered_clutch, 3),
    ));
    s.line(format!(
        "Fuel     {} L of capacity {} L",
        num(t.fuel, 2),
        num(t.fuel_capacity, 2)
    ));
    let (sector, in_pit) = split_current_sector(t.current_sector);
    let sector_text = if sector <= 2 {
        format!("S{}", sector + 1)
    } else {
        format!("S? (index {sector})")
    };
    s.line(format!(
        "Lap      lap {} | sector {sector_text} (mCurrentSector raw {:#010x}) | in pit lane {}",
        t.lap_number,
        t.current_sector,
        yes_no(in_pit),
    ));
}

fn split(cumulative: f64, previous: f64) -> f64 {
    if cumulative > 0.0 && previous > 0.0 {
        cumulative - previous
    } else {
        0.0
    }
}

fn scoring_lines(s: &mut Screen, snap: &RawSnapshot, t: &RawTelemetry) {
    let Some(row) = snap.player_scoring() else {
        s.line(format!(
            "Scoring  {UNAVAILABLE}: no mIsPlayer scoring row with telemetry mID {} (rows {})",
            t.id,
            snap.scoring.vehicles.len()
        ));
        return;
    };
    s.line(format!(
        "Scoring  place {} | class \"{}\" | laps {} | pit state {} | in pits {} | pitstops {}",
        row.place,
        safe_text(&row.vehicle_class),
        row.total_laps,
        row.pit_state,
        yes_no(row.in_pits),
        row.num_pitstops,
    ));
    s.line(format!(
        "         last lap {} (S1 {} S2 {} S3 {})",
        lap_time(row.last_lap_time),
        lap_time(row.last_sector1),
        lap_time(split(row.last_sector2, row.last_sector1)),
        lap_time(split(row.last_lap_time, row.last_sector2)),
    ));
    s.line(format!(
        "         best lap {} | best S1 {} | best S1+S2 {} (cumulative) | current S1 {} S1+S2 {}",
        lap_time(row.best_lap_time),
        lap_time(row.best_sector1),
        lap_time(row.best_sector2),
        lap_time(row.cur_sector1),
        lap_time(row.cur_sector2),
    ));
}

/// Which unit makes a raw `mBrakeTemp` plausible (A-LMU-015: the header says °C, the community K).
pub(crate) fn brake_unit_hint(raw: f64, ambient_c: f64) -> &'static str {
    if !raw.is_finite() {
        return UNAVAILABLE;
    }
    if raw < BRAKE_ONLY_CELSIUS_BELOW {
        return "C plausible (as K below -123 C)";
    }
    if raw > BRAKE_ONLY_KELVIN_ABOVE {
        return "K plausible (as C above 1500 C)";
    }
    if ambient_c.is_finite()
        && (raw - (ambient_c + KELVIN_OFFSET)).abs() <= BRAKE_AMBIENT_TOLERANCE_K
    {
        return "K plausible (cold, near ambient)";
    }
    "ambiguous: check cold"
}

fn wheel_row(s: &mut Screen, label: &str, cell: impl Fn(&RawWheel) -> String, wheels: &[RawWheel]) {
    let mut line = format!("{label:<WHEEL_LABEL$}");
    for w in wheels {
        line.push_str(&format!("{:<WHEEL_COL$}", cell(w)));
    }
    s.line(line.trim_end());
}

fn triple(v: [f64; 3], f: impl Fn(f64) -> f64) -> String {
    format!(
        "{}/{}/{}",
        num(f(v[0]), 1),
        num(f(v[1]), 1),
        num(f(v[2]), 1)
    )
}

fn wheel_table(s: &mut Screen, t: &RawTelemetry, ambient_c: f64) {
    let wheels = &t.wheels;
    let mut header = format!("{:<WHEEL_LABEL$}", "Wheels");
    for name in WHEEL_NAMES {
        header.push_str(&format!("{name:<WHEEL_COL$}"));
    }
    s.line(header.trim_end());
    wheel_row(s, "pressure kPa", |w| num(w.pressure, 1), wheels);
    wheel_row(s, "tread K L/C/R", |w| triple(w.temperature, |k| k), wheels);
    wheel_row(
        s,
        " -> C L/C/R",
        |w| triple(w.temperature, convert::kelvin_to_celsius),
        wheels,
    );
    wheel_row(
        s,
        "carcass K -> C",
        |w| {
            format!(
                "{} -> {}",
                num(w.tire_carcass_temperature, 1),
                num(convert::kelvin_to_celsius(w.tire_carcass_temperature), 1)
            )
        },
        wheels,
    );
    wheel_row(
        s,
        "inner K L/C/R",
        |w| triple(w.tire_inner_layer_temperature, |k| k),
        wheels,
    );
    wheel_row(s, "brake raw", |w| num(w.brake_temp, 1), wheels);
    wheel_row(
        s,
        " if K -> C",
        |w| num(convert::kelvin_to_celsius(w.brake_temp), 1),
        wheels,
    );
    wheel_row(s, " if C (header)", |w| num(w.brake_temp, 1), wheels);
    wheel_row(
        s,
        " unit hint",
        |w| brake_unit_hint(w.brake_temp, ambient_c).to_owned(),
        wheels,
    );
    wheel_row(s, "brake press 0..1", |w| num(w.brake_pressure, 3), wheels);
    wheel_row(s, "wear (fraction)", |w| num(w.wear, 3), wheels);
    wheel_row(s, "ride height m", |w| num(w.ride_height, 3), wheels);
    wheel_row(
        s,
        "flat / detached",
        |w| format!("{} / {}", yes_no(w.flat), yes_no(w.detached)),
        wheels,
    );
}

fn hybrid_line(t: &RawTelemetry) -> String {
    let state = t.electric_boost_motor_state;
    if state == 0 {
        return format!(
            "Hybrid   {UNAVAILABLE} (motor state 0 = no hybrid system) | raw battery {} SoC {} VE {} regen {}",
            num(t.battery_charge_fraction, 3),
            num32(t.soc, 3),
            num32(t.virtual_energy, 3),
            num32(t.regen, 1),
        );
    }
    format!(
        "Hybrid   motor state {state} {} | battery {} (fraction) | SoC {} (unit?) | virtual energy {} (unit?) | regen {} kW | motor {} rpm {} Nm",
        motor_state_name(state),
        num(t.battery_charge_fraction, 3),
        num32(t.soc, 3),
        num32(t.virtual_energy, 3),
        num32(t.regen, 1),
        num(t.electric_boost_motor_rpm, 0),
        num(t.electric_boost_motor_torque, 1),
    )
}

fn aid(level: u8, max: u8) -> String {
    if max == 0 {
        format!("{UNAVAILABLE} (max 0)")
    } else {
        format!("{level}/{max}")
    }
}

fn aids_line(t: &RawTelemetry) -> String {
    format!(
        "Aids     TC {} active {} (slip {} cut {}) | ABS {} active {} | motor map {} | brake bias rear {}",
        aid(t.tc, t.tc_max),
        yes_no(t.tc_active),
        aid(t.tc_slip, t.tc_slip_max),
        aid(t.tc_cut, t.tc_cut_max),
        aid(t.abs, t.abs_max),
        yes_no(t.abs_active),
        aid(t.motor_map, t.motor_map_max),
        num(t.rear_brake_bias, 3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmu_reader::offsets::{telem_info, telem_wheel};
    use lmu_reader::testutil::LayoutBuilder;
    use lmu_reader::{
        CopyPlan, FrameMeta, FrameOutcome, FrameProcessor, FrameSource, RawBlocks, ReaderStats,
    };

    fn snapshot_of(b: &LayoutBuilder) -> RawSnapshot {
        let mut blocks = RawBlocks::new();
        lmu_reader::blocks::copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        let meta = FrameMeta {
            capture_mono_ns: 0,
            connection_id: 1,
            source: FrameSource {
                sync: None,
                consistency: lmu_reader::Consistency::Replayed { locked: true },
            },
            mapping_size: None,
        };
        match FrameProcessor::new()
            .process(&mut blocks, &meta, &mut ReaderStats::default())
            .expect("process")
        {
            FrameOutcome::Snapshot(s) => *s,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn render(snapshot: &RawSnapshot) -> String {
        render_live(&LiveInput {
            status: None,
            snapshot: Some(snapshot),
            now_ns: 0,
            errors: &[],
            log: None,
        })
    }

    #[test]
    fn renders_values_with_raw_units_and_conversions() {
        let text = render(&snapshot_of(&LayoutBuilder::on_track_frame()));
        assert!(
            text.contains("|mLocalVel| 50.00 m/s -> 180.0 km/h"),
            "{text}"
        );
        assert!(text.contains("rpm 7500"), "{text}");
        assert!(text.contains("353.1/353.1/353.1"), "{text}");
        assert!(text.contains("80.0/80.0/80.0"), "{text}");
        assert!(
            text.contains("Hybrid   UNAVAILABLE (motor state 0"),
            "{text}"
        );
        assert!(text.contains("place 1"), "{text}");
    }

    #[test]
    fn nan_channels_print_unavailable() {
        let mut b = LayoutBuilder::on_track_frame();
        b.set(
            LayoutBuilder::telem_base(0),
            telem_info::ENGINE_RPM,
            f64::NAN,
        );
        b.set(
            LayoutBuilder::wheel_base(0, 0),
            telem_wheel::PRESSURE,
            f64::INFINITY,
        );
        let text = render(&snapshot_of(&b));
        assert!(text.contains("rpm UNAVAILABLE (NaN)"), "{text}");
        assert!(text.contains("UNAVAILABLE (+inf)"), "{text}");
    }

    #[test]
    fn missing_player_vehicle_is_unavailable() {
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(2, 0, 0);
        let text = render(&snapshot_of(&b));
        assert!(
            text.contains("Player   UNAVAILABLE: no player vehicle"),
            "{text}"
        );
    }

    #[test]
    fn unverified_player_is_unavailable_not_another_car() {
        let mut b = LayoutBuilder::on_track_frame();
        b.set(LayoutBuilder::telem_base(0), telem_info::ID, 9i32);
        let text = render(&snapshot_of(&b));
        assert!(
            text.contains("Player   UNAVAILABLE: not verified (IdNotFound)"),
            "{text}"
        );
        assert!(
            !text.contains("rpm 7500"),
            "no telemetry of another car: {text}"
        );
    }

    #[test]
    fn lmu_strings_cannot_inject_escape_sequences() {
        let mut b = LayoutBuilder::on_track_frame();
        b.set_raw(
            LayoutBuilder::scoring_base(),
            lmu_reader::offsets::scoring_info::TRACK_NAME,
            b"Ring\x1b[2J\x1b]52;c;aGk=\x07\0",
        );
        b.set_raw(
            LayoutBuilder::telem_base(0),
            telem_info::VEHICLE_NAME,
            b"Car\x1b[31m\0",
        );
        let text = render(&snapshot_of(&b));
        assert!(!text.contains('\x1b'), "{text:?}");
        assert!(!text.contains('\x07'), "{text:?}");
        assert!(text.contains(r"Ring\u{1b}[2J"), "{text}");
    }

    #[test]
    fn brake_unit_hints() {
        assert_eq!(
            brake_unit_hint(25.0, 20.0),
            "C plausible (as K below -123 C)"
        );
        assert_eq!(
            brake_unit_hint(296.0, 20.0),
            "K plausible (cold, near ambient)"
        );
        assert_eq!(brake_unit_hint(700.0, 20.0), "ambiguous: check cold");
        assert_eq!(
            brake_unit_hint(1_600.0, 20.0),
            "K plausible (as C above 1500 C)"
        );
        assert_eq!(brake_unit_hint(f64::NAN, 20.0), UNAVAILABLE);
    }

    /// Prints a sample screen built from the synthetic frame, for eyeballing the layout:
    /// `cargo test -p lmu-diag sample_screen -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual inspection of the rendered layout"]
    fn sample_screen() {
        eprintln!("{}", render(&snapshot_of(&LayoutBuilder::on_track_frame())));
    }
}
