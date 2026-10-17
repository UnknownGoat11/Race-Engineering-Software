//! Weather drift: air/track temperature, cloud, rain shower, track wetness (wetting and drying) and wind.

use std::f64::consts::TAU;

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::scenario::{RainSpec, WeatherSpec};

/// Seconds per hour.
const SECONDS_PER_HOUR: f64 = 3_600.0;
/// Period and amplitude of the slow air-temperature oscillation (s, °C).
const AIR_WAVE_PERIOD_S: f64 = 1_800.0;
const AIR_WAVE_AMPLITUDE_C: f64 = 0.4;
/// Air cooling at full rain (°C).
const RAIN_AIR_COOLING_C: f64 = 3.0;
/// Track cooling at a fully wet surface (°C).
const WET_TRACK_COOLING_C: f64 = 8.0;
/// Share of the solar track offset removed by full dark cloud.
const CLOUD_SHADE_SHARE: f64 = 0.7;
/// Clouds build up this long before the rain arrives (s).
const CLOUD_LEAD_S: f64 = 120.0;
/// Baseline cloud cover (fraction).
const BASE_DARK_CLOUD: f64 = 0.1;
/// Wetting rate at full rain (1/s) and drying rate of a wet surface at 30 °C track (1/s).
const WETTING_PER_S: f64 = 0.02;
const DRYING_PER_S: f64 = 0.002;
/// Track temperature at which drying runs at its nominal rate (°C).
const DRYING_REFERENCE_C: f64 = 30.0;
/// Wetness at which the wet speed profile fully applies.
const FULL_WET_WETNESS: f64 = 0.6;
/// Wind: mean speed (m/s), gust amplitude (m/s), direction drift period (s).
const WIND_MEAN_MPS: f64 = 3.0;
const WIND_GUST_MPS: f64 = 1.0;
const WIND_DIRECTION_PERIOD_S: f64 = 2_400.0;
const WIND_GUST_PERIOD_S: f64 = 37.0;
/// Random-walk step of the air temperature per scoring update (°C).
const AIR_NOISE_STEP_C: f64 = 0.01;
/// Bound of the accumulated air-temperature noise (°C).
const AIR_NOISE_BOUND_C: f64 = 0.5;
/// Raw cloud-coverage scale used by the synthetic source (LMU's scale is unverified).
const CLOUD_COVERAGE_STEPS: f64 = 4.0;

/// Current weather.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherState {
    pub air_temp_c: f64,
    pub track_temp_c: f64,
    /// Rain intensity 0..1.
    pub rain: f64,
    pub dark_cloud: f64,
    /// Average path wetness 0..1.
    pub wetness: f64,
    /// Wind in world x / z (m/s).
    pub wind_x_mps: f64,
    pub wind_z_mps: f64,
    noise_c: f64,
    /// The linear drift stops here (the session end), so an endless run stays in range (s).
    drift_end_s: f64,
}

/// Extreme temperatures the weather model can produce (°C), see [`temperature_bounds`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureBounds {
    /// Lowest / highest air temperature.
    pub air_c: (f64, f64),
    /// Lowest / highest track temperature.
    pub track_c: (f64, f64),
}

/// Bounds of the air and track temperatures over a session of `session_duration_s` (the drift stops at the
/// session end): drift, slow wave, bounded noise, rain cooling, cloud shade and wet-track cooling.
pub fn temperature_bounds(spec: &WeatherSpec, session_duration_s: f64) -> TemperatureBounds {
    let drift_c = spec.drift_c_per_hour * session_duration_s / SECONDS_PER_HOUR;
    let rain_peak = spec.rain.map_or(0.0, |r| r.intensity);
    let swing_c = AIR_WAVE_AMPLITUDE_C + AIR_NOISE_BOUND_C;
    let air_min = spec.air_temp_c + drift_c.min(0.0) - swing_c - RAIN_AIR_COOLING_C * rain_peak;
    let air_max = spec.air_temp_c + drift_c.max(0.0) + swing_c;
    // The solar offset is scaled by 1 − share × cloud, cloud within BASE_DARK_CLOUD..=1.
    let offsets = [
        spec.track_temp_offset_c * (1.0 - CLOUD_SHADE_SHARE),
        spec.track_temp_offset_c * (1.0 - CLOUD_SHADE_SHARE * BASE_DARK_CLOUD),
    ];
    let wet_cooling_c = if rain_peak > 0.0 {
        WET_TRACK_COOLING_C
    } else {
        0.0
    };
    TemperatureBounds {
        air_c: (air_min, air_max),
        track_c: (
            air_min + offsets[0].min(offsets[1]) - wet_cooling_c,
            air_max + offsets[0].max(offsets[1]),
        ),
    }
}

impl WeatherState {
    /// Initial weather at session time 0 of a session lasting `session_duration_s`.
    pub fn new(spec: &WeatherSpec, session_duration_s: f64) -> Self {
        let mut w = Self {
            air_temp_c: spec.air_temp_c,
            track_temp_c: spec.air_temp_c + spec.track_temp_offset_c,
            rain: 0.0,
            dark_cloud: BASE_DARK_CLOUD,
            wetness: 0.0,
            wind_x_mps: 0.0,
            wind_z_mps: 0.0,
            noise_c: 0.0,
            drift_end_s: session_duration_s,
        };
        w.apply(spec, 0.0);
        w
    }

    /// Advances by `dt` to session time `et`. The temperature noise is a bounded random walk.
    pub fn step(&mut self, spec: &WeatherSpec, et_s: f64, dt_s: f64, rng: &mut ChaCha8Rng) {
        let step = rng.random_range(-AIR_NOISE_STEP_C..=AIR_NOISE_STEP_C) * (dt_s * 5.0).min(1.0);
        self.noise_c = (self.noise_c + step).clamp(-AIR_NOISE_BOUND_C, AIR_NOISE_BOUND_C);
        let drying_factor = (self.track_temp_c / DRYING_REFERENCE_C).max(0.1);
        let wet_rate = WETTING_PER_S * self.rain * (1.0 - self.wetness)
            - DRYING_PER_S * (1.0 - self.rain) * self.wetness * drying_factor;
        self.wetness = (self.wetness + wet_rate * dt_s).clamp(0.0, 1.0);
        self.apply(spec, et_s);
    }

    fn apply(&mut self, spec: &WeatherSpec, et_s: f64) {
        self.rain = rain_at(spec.rain.as_ref(), et_s);
        let upcoming = rain_at(spec.rain.as_ref(), et_s + CLOUD_LEAD_S);
        self.dark_cloud = (BASE_DARK_CLOUD + upcoming.max(self.rain)).clamp(0.0, 1.0);
        self.air_temp_c = spec.air_temp_c
            + spec.drift_c_per_hour * et_s.min(self.drift_end_s) / SECONDS_PER_HOUR
            + AIR_WAVE_AMPLITUDE_C * (TAU * et_s / AIR_WAVE_PERIOD_S).sin()
            - RAIN_AIR_COOLING_C * self.rain
            + self.noise_c;
        self.track_temp_c = self.air_temp_c
            + spec.track_temp_offset_c * (1.0 - CLOUD_SHADE_SHARE * self.dark_cloud)
            - WET_TRACK_COOLING_C * self.wetness;
        let direction = TAU * et_s / WIND_DIRECTION_PERIOD_S;
        let speed = WIND_MEAN_MPS + WIND_GUST_MPS * (TAU * et_s / WIND_GUST_PERIOD_S).sin();
        self.wind_x_mps = speed * direction.cos();
        self.wind_z_mps = speed * direction.sin();
    }

    /// Blend factor between the dry (0) and wet (1) speed profiles.
    pub fn wet_blend(&self) -> f64 {
        (self.wetness / FULL_WET_WETNESS).clamp(0.0, 1.0)
    }

    /// Minimum / maximum path wetness around the average.
    pub fn wetness_min_max(&self) -> (f64, f64) {
        (
            (self.wetness * 0.7).clamp(0.0, 1.0),
            (self.wetness * 1.25).clamp(0.0, 1.0),
        )
    }

    /// Raw cloud coverage on the synthetic 0..=4 scale.
    pub fn cloud_coverage_raw(&self) -> u8 {
        // 0..=4 after the clamp, so the cast cannot truncate.
        (self.dark_cloud * CLOUD_COVERAGE_STEPS)
            .round()
            .clamp(0.0, CLOUD_COVERAGE_STEPS) as u8
    }
}

/// Rain intensity of the shower at `et` (trapezoid).
fn rain_at(rain: Option<&RainSpec>, et_s: f64) -> f64 {
    let Some(r) = rain else {
        return 0.0;
    };
    let t = et_s - r.start_et_s;
    let ramp = r.ramp_s.max(f64::EPSILON);
    let level = if t <= 0.0 {
        0.0
    } else if t < r.ramp_s {
        t / ramp
    } else if t <= r.ramp_s + r.duration_s {
        1.0
    } else {
        1.0 - (t - r.ramp_s - r.duration_s) / ramp
    };
    level.clamp(0.0, 1.0) * r.intensity
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    /// Regression (review pass 2, F5): the drift ran without bound in an endless run.
    #[test]
    fn drift_stops_at_the_session_end_and_stays_within_the_bounds() {
        let spec = WeatherSpec {
            air_temp_c: 10.0,
            track_temp_offset_c: 12.0,
            drift_c_per_hour: -10.0,
            rain: Some(RainSpec {
                start_et_s: 100.0,
                ramp_s: 20.0,
                duration_s: 300.0,
                intensity: 1.0,
            }),
        };
        let session_s = 1_800.0;
        let bounds = temperature_bounds(&spec, session_s);
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let mut w = WeatherState::new(&spec, session_s);
        let dt = 1.0;
        // Ten hours, far beyond the session end.
        for i in 1..=36_000 {
            w.step(&spec, f64::from(i) * dt, dt, &mut rng);
            assert!(
                (bounds.air_c.0..=bounds.air_c.1).contains(&w.air_temp_c),
                "air {} outside {:?}",
                w.air_temp_c,
                bounds.air_c
            );
            assert!(
                (bounds.track_c.0..=bounds.track_c.1).contains(&w.track_temp_c),
                "track {} outside {:?}",
                w.track_temp_c,
                bounds.track_c
            );
        }
        // Drift frozen at −5 °C after 30 min; only the wave and the bounded noise remain.
        assert!((w.air_temp_c - 5.0).abs() <= AIR_WAVE_AMPLITUDE_C + AIR_NOISE_BOUND_C);
    }

    #[test]
    fn shower_wets_and_dries_the_track() {
        let spec = WeatherSpec {
            air_temp_c: 20.0,
            track_temp_offset_c: 10.0,
            drift_c_per_hour: 0.0,
            rain: Some(RainSpec {
                start_et_s: 10.0,
                ramp_s: 20.0,
                duration_s: 100.0,
                intensity: 0.8,
            }),
        };
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut w = WeatherState::new(&spec, 3_600.0);
        assert_eq!(w.rain, 0.0);
        let mut peak_wet: f64 = 0.0;
        let dt = 0.2;
        for i in 1..=5_000 {
            w.step(&spec, f64::from(i) * dt, dt, &mut rng);
            peak_wet = peak_wet.max(w.wetness);
            assert!((0.0..=1.0).contains(&w.wetness));
            assert!(w.track_temp_c > 0.0 && w.track_temp_c < 40.0);
        }
        assert!(peak_wet > 0.6, "{peak_wet}");
        assert_eq!(w.rain, 0.0);
        assert!(w.wetness < peak_wet / 2.0, "dried to {}", w.wetness);
        assert!(w.cloud_coverage_raw() <= 4);
    }
}
