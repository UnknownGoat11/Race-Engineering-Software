//! Track engine tests.

use std::f64::consts::PI;

use super::*;

const R: f64 = 500.0;

fn length() -> f64 {
    2.0 * PI * R
}

fn on_circle(d: f64, radial_offset: f64) -> (f64, f64) {
    let a = d / R;
    ((R + radial_offset) * a.cos(), (R + radial_offset) * a.sin())
}

fn id() -> TrackId {
    TrackId::from_lmu("Test Ring", length()).expect("id")
}

/// One lap sampled every metre with small deterministic noise; `spike` adds a 50 m excursion near 1000 m.
fn feed_lap(b: &mut TrackMapBuilder, spike: bool, eligible: bool) {
    let mut d = 0.0;
    while d < length() {
        let noise = 0.3 * (d * 0.37).sin();
        let off = if spike && (995.0..1005.0).contains(&d) {
            50.0
        } else {
            noise
        };
        let (x, z) = on_circle(d, off);
        let sector = if d < 1000.0 {
            0
        } else if d < 2000.0 {
            1
        } else {
            2
        };
        b.add_sample(&TrackSample {
            lap_dist_m: Some(d),
            x: Some(x),
            z: Some(z),
            in_pit_lane: Some(false),
            sector_index: Some(sector),
        });
        d += 1.0;
    }
    b.lap_completed(eligible);
}

fn built_map() -> TrackMap {
    let mut b = TrackMapBuilder::new(id(), length(), BuildOptions::default()).expect("builder");
    // Grid samples before the first crossing: negative lap distance, ignored.
    for i in 0..50 {
        b.add_sample(&TrackSample {
            lap_dist_m: Some(-100.0 + f64::from(i)),
            x: Some(9_999.0),
            z: Some(9_999.0),
            in_pit_lane: Some(false),
            sector_index: Some(2),
        });
    }
    b.lap_completed(false);
    feed_lap(&mut b, false, true);
    feed_lap(&mut b, true, true);
    feed_lap(&mut b, false, true);
    // An in-lap (ineligible) with the pit entry and the exit on the next out-lap.
    for (d, pit) in [(2990.0, false), (3000.0, true), (3050.0, true)] {
        let (x, z) = on_circle(d, 8.0);
        b.add_sample(&TrackSample {
            lap_dist_m: Some(d),
            x: Some(x),
            z: Some(z),
            in_pit_lane: Some(pit),
            sector_index: Some(2),
        });
    }
    b.lap_completed(false);
    for (d, pit) in [(40.0, true), (60.0, false)] {
        let (x, z) = on_circle(d, 8.0);
        b.add_sample(&TrackSample {
            lap_dist_m: Some(d),
            x: Some(x),
            z: Some(z),
            in_pit_lane: Some(pit),
            sector_index: Some(0),
        });
    }
    b.lap_completed(false);
    assert_eq!(b.laps_collected(), 3);
    b.build().expect("map")
}

#[test]
fn track_id_is_layout_unique() {
    let spa = TrackId::from_lmu("Circuit de Spa-Francorchamps", 7004.3).expect("id");
    assert_eq!(spa.as_str(), "CIRCUIT_DE_SPA_FRANCORCHAMPS_7004");
    let other = TrackId::from_lmu("Circuit de Spa-Francorchamps", 6995.0).expect("id");
    assert_ne!(spa, other);
    assert!(matches!(
        TrackId::from_lmu(" - ", 5000.0),
        Err(TrackError::EmptyName)
    ));
    assert!(matches!(
        TrackId::from_lmu("Spa", 100.0),
        Err(TrackError::ImplausibleLength(_))
    ));
    assert!(matches!(
        TrackId::from_lmu("Spa", f64::NAN),
        Err(TrackError::ImplausibleLength(_))
    ));
    let json = serde_json::to_string(&spa).expect("json");
    assert_eq!(json, "\"CIRCUIT_DE_SPA_FRANCORCHAMPS_7004\"");
    let back: TrackId = serde_json::from_str(&json).expect("parse");
    assert_eq!(back, spa);
    assert!(serde_json::from_str::<TrackId>("\"../evil\"").is_err());
    assert!(serde_json::from_str::<TrackId>("\"SPA\"").is_err());
}

#[test]
fn generated_map_is_valid_smooth_and_outlier_free() {
    let map = built_map();
    map.validate().expect("valid");
    assert_eq!(map.version, TRACK_MAP_FORMAT_VERSION);
    assert_eq!(map.source, TrackMapSource::Generated);
    assert_eq!(map.generated_from_laps, 3);
    let expected_points = (length() / DEFAULT_BIN_M).ceil() as usize;
    assert!(
        map.points.len() + 2 >= expected_points,
        "{}",
        map.points.len()
    );
    // Every point lies on the circle: the spike lap was outvoted, the grid samples ignored.
    for p in &map.points {
        let r = (p.x * p.x + p.z * p.z).sqrt();
        assert!((r - R).abs() < 1.0, "point at {} has radius {r}", p.dist_m);
    }
    let (sx, sz) = (map.start_finish.x, map.start_finish.z);
    assert!((sx - R).abs() < 1.0 && sz.abs() < 1.0, "{sx} {sz}");
}

#[test]
fn sectors_and_pit_markers() {
    let map = built_map();
    assert_eq!(map.sectors.len(), 2);
    assert_eq!(map.sectors[0].index, 1);
    assert!((map.sectors[0].dist_m - 999.5).abs() < 1e-9);
    assert!((map.sectors[1].dist_m - 1999.5).abs() < 1e-9);
    assert_eq!(map.sector_at(10.0), 0);
    assert_eq!(map.sector_at(1500.0), 1);
    assert_eq!(map.sector_at(2500.0), 2);
    assert_eq!(map.sector_at(length() + 1500.0), 1);
    let entry = map.pit_entry.expect("pit entry");
    assert_eq!(entry.dist_m, 3000.0);
    let exit = map.pit_exit.expect("pit exit");
    assert_eq!(exit.dist_m, 60.0);
}

#[test]
fn projection_gives_distance_offset_and_sector() {
    let map = built_map();
    for d in [0.5, 123.0, 1234.0, 2000.0, 3100.0, length() - 1.0] {
        let (x, z) = on_circle(d, 0.0);
        let p = map.project(x, z, None).expect("projection");
        let err = (p.dist_m - d).abs().min(length() - (p.dist_m - d).abs());
        assert!(err < 1.5, "d {d} projected {}", p.dist_m);
        assert!(p.lateral_offset_m.abs() < 1.0);
        assert!((0.0..1.0).contains(&p.progress));
        // The hinted search agrees with the full search.
        let hinted = map.project(x, z, Some(p.segment)).expect("hinted");
        assert_eq!(hinted.segment, p.segment);
        // A wrong hint still finds the right place.
        let far = map.project(
            x,
            z,
            Some((p.segment + map.points.len() / 2) % map.points.len()),
        );
        assert_eq!(far.expect("far hint").segment, p.segment);
    }
    // Outside the counter-clockwise circle is to the right: negative offset.
    let (x, z) = on_circle(1500.0, 5.0);
    let p = map.project(x, z, None).expect("projection");
    assert!(
        (p.lateral_offset_m + 5.0).abs() < 1.0,
        "{}",
        p.lateral_offset_m
    );
    assert_eq!(p.sector, 1);
    assert!(map.project(f64::NAN, 0.0, None).is_none());
    let m = map.projected_distance(Some(x), None, None);
    assert_eq!(m.value, None);
    assert_eq!(m.formula, formula::TRACK_PROJECTION);
}

#[test]
fn position_at_distance_round_trips() {
    let map = built_map();
    for d in [0.0, 1.0, 777.0, 2999.0, length() - 0.5, -10.0] {
        let (x, z) = map.position_at(d).expect("position");
        let (ex, ez) = on_circle(d, 0.0);
        assert!(planar(x, z, ex, ez) < 1.5, "d {d}: {x} {z} vs {ex} {ez}");
    }
    assert!(map.position_at(f64::INFINITY).is_none());
}

#[test]
fn not_enough_data_is_an_error() {
    let b = TrackMapBuilder::new(id(), length(), BuildOptions::default()).expect("builder");
    assert!(matches!(
        b.build(),
        Err(TrackError::InsufficientData { laps: 0, .. })
    ));
    let mut b = TrackMapBuilder::new(id(), length(), BuildOptions::default()).expect("builder");
    let mut d = 0.0;
    while d < 0.5 * length() {
        let (x, z) = on_circle(d, 0.0);
        b.add_sample(&TrackSample {
            lap_dist_m: Some(d),
            x: Some(x),
            z: Some(z),
            in_pit_lane: Some(false),
            sector_index: None,
        });
        d += 1.0;
    }
    b.lap_completed(true);
    assert!(matches!(
        b.build(),
        Err(TrackError::InsufficientData { laps: 1, .. })
    ));
    assert!(matches!(
        TrackMapBuilder::new(
            id(),
            length(),
            BuildOptions {
                bin_m: 0.1,
                ..BuildOptions::default()
            }
        ),
        Err(TrackError::InvalidOption("bin_m"))
    ));
}

#[test]
fn validation_rejects_bad_maps() {
    let good = built_map();
    let mut m = good.clone();
    m.version = 99;
    assert!(matches!(
        m.validate(),
        Err(TrackError::UnsupportedVersion(99))
    ));
    let mut m = good.clone();
    m.points[10].x = f64::NAN;
    assert!(matches!(m.validate(), Err(TrackError::InvalidMap(_))));
    let mut m = good.clone();
    m.points[10].dist_m = m.points[9].dist_m;
    assert!(matches!(m.validate(), Err(TrackError::InvalidMap(_))));
    let mut m = good.clone();
    let half = m.points.len() / 2;
    m.points.truncate(half);
    assert!(
        matches!(m.validate(), Err(TrackError::InvalidMap(_))),
        "open circuit"
    );
    let mut m = good.clone();
    m.sectors.reverse();
    assert!(matches!(m.validate(), Err(TrackError::InvalidMap(_))));
    let mut m = good;
    m.length_m = 10.0;
    assert!(matches!(
        m.validate(),
        Err(TrackError::ImplausibleLength(_))
    ));
}

#[test]
fn file_format_uses_the_documented_keys() {
    let map = built_map();
    let v = serde_json::to_value(&map).expect("json");
    for key in [
        "trackId",
        "version",
        "points",
        "sectors",
        "startFinish",
        "pitEntry",
        "pitExit",
        "lengthM",
        "source",
    ] {
        assert!(v.get(key).is_some(), "missing {key}");
    }
    let back: TrackMap = serde_json::from_value(v).expect("parse");
    assert_eq!(back, map);
}

/// Test directory: `RACE_ENGINES_TEST_DIR` when set (the project keeps temporary files off the system
/// drive), else the system temporary directory.
fn test_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("RACE_ENGINES_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("race-engines-{name}-{}", std::process::id()))
}

#[test]
fn store_round_trip_prefers_verified_maps() {
    let dir = test_dir("track-store");
    let store = TrackMapStore::new(&dir);
    let map = built_map();
    assert!(store.load(&map.track_id).expect("load").is_none());
    let path = store.save(&map).expect("save");
    assert_eq!(path, store.path_for(&map.track_id, false));
    assert_eq!(store.load(&map.track_id).expect("load"), Some(map.clone()));

    let mut verified = map.clone();
    verified.source = TrackMapSource::Verified;
    verified.generated_from_laps = 0;
    store.save(&verified).expect("save verified");
    assert_eq!(
        store.load(&map.track_id).expect("load").map(|m| m.source),
        Some(TrackMapSource::Verified)
    );

    // A file holding another track is rejected.
    let other = TrackId::from_lmu("Other Ring", length()).expect("id");
    fs::copy(
        store.path_for(&map.track_id, false),
        store.path_for(&other, false),
    )
    .expect("copy");
    assert!(matches!(store.load(&other), Err(TrackError::InvalidMap(_))));
    // A corrupt file is an error, never a silently empty map.
    fs::write(store.path_for(&other, false), b"{not json").expect("write");
    assert!(matches!(store.load(&other), Err(TrackError::Json(_))));

    let mut invalid = map;
    invalid.points.clear();
    assert!(store.save(&invalid).is_err());
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn transform_fits_the_viewport_with_z_up() {
    let map = built_map();
    let t = MapTransform::fit(&map, 800.0, 600.0, 20.0, 0.0).expect("transform");
    for p in &map.points {
        let (u, v) = t.apply(p.x, p.z);
        assert!(
            (19.9..=780.1).contains(&u) && (19.9..=580.1).contains(&v),
            "{u} {v}"
        );
    }
    let (_, v_low) = t.apply(0.0, -R);
    let (_, v_high) = t.apply(0.0, R);
    assert!(v_high < v_low, "+z is up on screen");
    // Rotation by 90 degrees keeps the map inside the viewport.
    let r = MapTransform::fit(&map, 800.0, 600.0, 20.0, 0.5 * PI).expect("rotated");
    let (u, _) = r.apply(R, 0.0);
    assert!((u - 400.0).abs() < 1.0, "{u}");
    assert!(MapTransform::fit(&map, 30.0, 30.0, 20.0, 0.0).is_none());
}
