//! Replay clock: maps wall-clock time to a position on a recording's capture timeline.
//!
//! Pure and deterministic: every method takes the current `Instant` from the caller, so the clock holds no
//! hidden time source and is trivially testable. The position is exact integer arithmetic (speeds are
//! ratios, not floats), so long replays do not drift.

use std::time::{Duration, Instant};

/// Supported playback speeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlaybackSpeed {
    /// 0.25×
    Quarter,
    /// 0.5×
    Half,
    /// 1×
    #[default]
    Normal,
    /// 2×
    Double,
    /// 4×
    Quadruple,
}

impl PlaybackSpeed {
    /// Every speed, slowest first.
    pub const ALL: [PlaybackSpeed; 5] = [
        PlaybackSpeed::Quarter,
        PlaybackSpeed::Half,
        PlaybackSpeed::Normal,
        PlaybackSpeed::Double,
        PlaybackSpeed::Quadruple,
    ];

    /// Capture nanoseconds per wall nanosecond as `(numerator, denominator)`.
    pub const fn ratio(self) -> (u64, u64) {
        match self {
            PlaybackSpeed::Quarter => (1, 4),
            PlaybackSpeed::Half => (1, 2),
            PlaybackSpeed::Normal => (1, 1),
            PlaybackSpeed::Double => (2, 1),
            PlaybackSpeed::Quadruple => (4, 1),
        }
    }

    /// The speed as a factor (for display).
    pub fn factor(self) -> f64 {
        let (num, den) = self.ratio();
        num as f64 / den as f64
    }

    /// The next faster speed (saturating).
    pub fn faster(self) -> Self {
        match self {
            PlaybackSpeed::Quarter => PlaybackSpeed::Half,
            PlaybackSpeed::Half => PlaybackSpeed::Normal,
            PlaybackSpeed::Normal => PlaybackSpeed::Double,
            PlaybackSpeed::Double | PlaybackSpeed::Quadruple => PlaybackSpeed::Quadruple,
        }
    }

    /// The next slower speed (saturating).
    pub fn slower(self) -> Self {
        match self {
            PlaybackSpeed::Quadruple => PlaybackSpeed::Double,
            PlaybackSpeed::Double => PlaybackSpeed::Normal,
            PlaybackSpeed::Normal => PlaybackSpeed::Half,
            PlaybackSpeed::Half | PlaybackSpeed::Quarter => PlaybackSpeed::Quarter,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Paused {
        position_ns: u64,
    },
    /// Playing since `anchor_wall`, when the position was `anchor_ns`.
    Playing {
        anchor_ns: u64,
        anchor_wall: Instant,
    },
}

/// Playback clock over the capture-time interval `[start_ns, end_ns]` of a recording.
///
/// The position never leaves the interval: it stops at `end_ns` while playing, and seeks are clamped.
/// Changing the speed or pausing re-anchors the clock, so the position is continuous across changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayClock {
    start_ns: u64,
    end_ns: u64,
    speed: PlaybackSpeed,
    state: State,
}

impl ReplayClock {
    /// A paused clock at `start_ns`, at normal speed. `None` when `start_ns > end_ns`.
    pub fn new(start_ns: u64, end_ns: u64) -> Option<Self> {
        (start_ns <= end_ns).then_some(Self {
            start_ns,
            end_ns,
            speed: PlaybackSpeed::Normal,
            state: State::Paused {
                position_ns: start_ns,
            },
        })
    }

    /// First capture time of the interval.
    pub fn start_ns(&self) -> u64 {
        self.start_ns
    }

    /// Last capture time of the interval.
    pub fn end_ns(&self) -> u64 {
        self.end_ns
    }

    /// Current speed.
    pub fn speed(&self) -> PlaybackSpeed {
        self.speed
    }

    /// Whether the clock is running (it keeps "playing" at `end_ns` until paused).
    pub fn is_playing(&self) -> bool {
        matches!(self.state, State::Playing { .. })
    }

    /// Capture time shown at wall time `now`.
    pub fn position(&self, now: Instant) -> u64 {
        match self.state {
            State::Paused { position_ns } => position_ns,
            State::Playing {
                anchor_ns,
                anchor_wall,
            } => {
                let (num, den) = self.speed.ratio();
                let wall_ns = now.saturating_duration_since(anchor_wall).as_nanos();
                // u128: wall_ns × 4 cannot overflow for any realistic duration (< 2^126).
                let advanced = wall_ns.saturating_mul(u128::from(num)) / u128::from(den);
                let pos = u128::from(anchor_ns).saturating_add(advanced);
                u64::try_from(pos).unwrap_or(u64::MAX).min(self.end_ns)
            }
        }
    }

    /// Whether the position has reached the end of the interval.
    pub fn is_at_end(&self, now: Instant) -> bool {
        self.position(now) >= self.end_ns
    }

    /// Starts (or keeps) playing from the current position.
    pub fn play(&mut self, now: Instant) {
        if let State::Paused { position_ns } = self.state {
            self.state = State::Playing {
                anchor_ns: position_ns,
                anchor_wall: now,
            };
        }
    }

    /// Pauses at the current position.
    pub fn pause(&mut self, now: Instant) {
        self.state = State::Paused {
            position_ns: self.position(now),
        };
    }

    /// Jumps to `target_ns` (clamped to the interval), keeping the play/pause state.
    pub fn seek(&mut self, target_ns: u64, now: Instant) {
        let position_ns = target_ns.clamp(self.start_ns, self.end_ns);
        self.state = match self.state {
            State::Paused { .. } => State::Paused { position_ns },
            State::Playing { .. } => State::Playing {
                anchor_ns: position_ns,
                anchor_wall: now,
            },
        };
    }

    /// Changes the speed; the position is continuous across the change.
    pub fn set_speed(&mut self, speed: PlaybackSpeed, now: Instant) {
        if let State::Playing { .. } = self.state {
            self.state = State::Playing {
                anchor_ns: self.position(now),
                anchor_wall: now,
            };
        }
        self.speed = speed;
    }

    /// Wall time until the position reaches `target_ns`: zero if it already has, `None` while paused before
    /// it or when it lies beyond the end of the interval. Lets a replay loop sleep until the next record.
    pub fn wall_time_until(&self, target_ns: u64, now: Instant) -> Option<Duration> {
        let pos = self.position(now);
        if target_ns <= pos {
            return Some(Duration::ZERO);
        }
        if target_ns > self.end_ns || !self.is_playing() {
            return None;
        }
        let (num, den) = self.speed.ratio();
        // Rounded up so that sleeping this long always reaches the target.
        let wall_ns = (u128::from(target_ns - pos) * u128::from(den)).div_ceil(u128::from(num));
        Some(Duration::from_nanos(
            u64::try_from(wall_ns).unwrap_or(u64::MAX),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn plays_at_every_speed() {
        let t0 = Instant::now();
        for (speed, expected_after_1s) in [
            (PlaybackSpeed::Quarter, S / 4),
            (PlaybackSpeed::Half, S / 2),
            (PlaybackSpeed::Normal, S),
            (PlaybackSpeed::Double, 2 * S),
            (PlaybackSpeed::Quadruple, 4 * S),
        ] {
            let mut c = ReplayClock::new(10 * S, 100 * S).expect("interval");
            c.set_speed(speed, t0);
            c.play(t0);
            assert_eq!(c.position(t0), 10 * S);
            assert_eq!(
                c.position(t0 + ms(1_000)),
                10 * S + expected_after_1s,
                "{speed:?}"
            );
            assert_eq!(speed.factor() * 1e9, expected_after_1s as f64);
        }
    }

    #[test]
    fn pause_seek_and_speed_changes_are_continuous() {
        let t0 = Instant::now();
        let mut c = ReplayClock::new(0, 60 * S).expect("interval");
        assert!(!c.is_playing());
        assert_eq!(c.position(t0 + ms(5_000)), 0, "paused clocks do not move");
        c.play(t0);
        c.set_speed(PlaybackSpeed::Double, t0 + ms(1_000)); // at 1 s
        assert_eq!(c.position(t0 + ms(2_000)), 3 * S); // 1 s + 1 s × 2
        c.pause(t0 + ms(2_000));
        assert_eq!(c.position(t0 + ms(9_000)), 3 * S);
        c.seek(30 * S, t0 + ms(9_000));
        assert_eq!(
            c.position(t0 + ms(9_500)),
            30 * S,
            "seek keeps the paused state"
        );
        c.play(t0 + ms(10_000));
        assert_eq!(c.position(t0 + ms(10_250)), 30 * S + S / 2);
        c.seek(5 * S, t0 + ms(11_000));
        assert_eq!(
            c.position(t0 + ms(11_000)),
            5 * S,
            "seek while playing re-anchors"
        );
        c.set_speed(PlaybackSpeed::Quarter, t0 + ms(11_000));
        assert_eq!(c.position(t0 + ms(13_000)), 5 * S + S / 2);
        c.play(t0 + ms(14_000)); // already playing: no re-anchor
        assert_eq!(c.position(t0 + ms(15_000)), 6 * S);
    }

    #[test]
    fn position_is_clamped_to_the_interval() {
        let t0 = Instant::now();
        let mut c = ReplayClock::new(10 * S, 12 * S).expect("interval");
        c.seek(0, t0);
        assert_eq!(c.position(t0), 10 * S);
        c.seek(u64::MAX, t0);
        assert_eq!(c.position(t0), 12 * S);
        c.seek(11 * S, t0);
        c.set_speed(PlaybackSpeed::Quadruple, t0);
        c.play(t0);
        assert_eq!(c.position(t0 + ms(10_000)), 12 * S);
        assert!(c.is_at_end(t0 + ms(10_000)));
        let decade = Duration::from_secs(10 * 365 * 24 * 3600);
        assert_eq!(
            c.position(t0 + decade),
            12 * S,
            "no overflow over long wall spans"
        );
        // An instant before the anchor does not move the clock backwards.
        let mut c = ReplayClock::new(0, 10 * S).expect("interval");
        c.play(t0 + ms(1_000));
        assert_eq!(c.position(t0), 0);
        assert!(ReplayClock::new(2, 1).is_none());
        assert!(ReplayClock::new(5, 5).is_some());
    }

    #[test]
    fn wall_time_until_targets() {
        let t0 = Instant::now();
        let mut c = ReplayClock::new(0, 10 * S).expect("interval");
        assert_eq!(c.wall_time_until(0, t0), Some(Duration::ZERO));
        assert_eq!(c.wall_time_until(S, t0), None, "paused before the target");
        c.set_speed(PlaybackSpeed::Quarter, t0);
        c.play(t0);
        assert_eq!(c.wall_time_until(S, t0), Some(Duration::from_secs(4)));
        c.set_speed(PlaybackSpeed::Quadruple, t0);
        assert_eq!(c.wall_time_until(S, t0), Some(ms(250)));
        assert_eq!(
            c.wall_time_until(1, t0),
            Some(Duration::from_nanos(1)),
            "rounded up"
        );
        assert_eq!(c.wall_time_until(11 * S, t0), None, "beyond the end");
        let at = t0 + c.wall_time_until(3 * S + 1, t0).expect("playing");
        assert!(c.position(at) > 3 * S);
    }

    #[test]
    fn speed_steps_saturate() {
        assert_eq!(PlaybackSpeed::Quadruple.faster(), PlaybackSpeed::Quadruple);
        assert_eq!(PlaybackSpeed::Quarter.slower(), PlaybackSpeed::Quarter);
        let mut s = PlaybackSpeed::Quarter;
        for expected in &PlaybackSpeed::ALL[1..] {
            s = s.faster();
            assert_eq!(s, *expected);
        }
        assert_eq!(PlaybackSpeed::default(), PlaybackSpeed::Normal);
    }
}
