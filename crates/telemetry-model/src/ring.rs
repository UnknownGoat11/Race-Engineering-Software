//! Fixed-capacity time-series ring buffer for live graphs.
//!
//! Storage is columnar: one `f64` column per selected channel plus one timestamp column, all of the same
//! capacity, allocated once. Appending overwrites the oldest row in O(1) (head index arithmetic, never
//! element shifting). A missing or non-valid sample is stored as NaN and is never returned as a value, so
//! graphs break the line there instead of drawing a fabricated zero. Timestamps are non-decreasing (the
//! caller clears the buffer on a session restart / ET regression).

use std::ops::Range;

use thiserror::Error;

use crate::channel::{ChannelId, Interpolation};
use crate::frame::TelemetryFrame;
use crate::registry;

/// Errors from ring buffer construction and appends.
#[derive(Debug, Error, PartialEq)]
pub enum RingError {
    #[error("ring buffer capacity must be at least 1")]
    ZeroCapacity,
    #[error("channel 0x{0:04X} is not in the registry")]
    UnknownChannel(u16),
    #[error("channel 0x{0:04X} is listed twice")]
    DuplicateChannel(u16),
    #[error("channel 0x{0:04X} is not numeric and cannot be graphed")]
    NonNumericChannel(u16),
    #[error("timestamp {t_s} is not finite")]
    NonFiniteTime { t_s: f64 },
    #[error(
        "timestamp {t_s} is earlier than the newest stored timestamp {newest_s}; clear the buffer first"
    )]
    TimeRegression { t_s: f64, newest_s: f64 },
    #[error("row has {actual} values, the buffer has {expected} channels")]
    RowWidth { expected: usize, actual: usize },
}

/// One selected channel.
#[derive(Debug, Clone)]
struct Column {
    id: ChannelId,
    interpolation: Interpolation,
    values: Vec<f64>,
}

/// Fixed-capacity columnar ring buffer of timestamped samples.
#[derive(Debug, Clone)]
pub struct TelemetryRingBuffer {
    capacity: usize,
    /// Physical index of the oldest row.
    head: usize,
    len: usize,
    times: Vec<f64>,
    columns: Vec<Column>,
}

impl TelemetryRingBuffer {
    /// Creates a buffer holding `capacity` rows of the given numeric channels.
    pub fn new(capacity: usize, channels: &[ChannelId]) -> Result<Self, RingError> {
        if capacity == 0 {
            return Err(RingError::ZeroCapacity);
        }
        let mut columns: Vec<Column> = Vec::with_capacity(channels.len());
        for &id in channels {
            let def = registry::by_id(id).ok_or(RingError::UnknownChannel(id.0))?;
            if !def.is_numeric() {
                return Err(RingError::NonNumericChannel(id.0));
            }
            if columns.iter().any(|c| c.id == id) {
                return Err(RingError::DuplicateChannel(id.0));
            }
            columns.push(Column {
                id,
                interpolation: def.interpolation,
                values: vec![f64::NAN; capacity],
            });
        }
        Ok(Self {
            capacity,
            head: 0,
            len: 0,
            times: vec![f64::NAN; capacity],
            columns,
        })
    }

    /// Maximum number of rows.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Stored rows.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no row is stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Selected channels, in column order.
    pub fn channels(&self) -> impl Iterator<Item = ChannelId> + '_ {
        self.columns.iter().map(|c| c.id)
    }

    /// Physical slot of logical row `i` (0 = oldest).
    fn slot(&self, i: usize) -> usize {
        (self.head + i) % self.capacity
    }

    fn column(&self, id: ChannelId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    fn check_time(&self, t_s: f64) -> Result<(), RingError> {
        if !t_s.is_finite() {
            return Err(RingError::NonFiniteTime { t_s });
        }
        match self.newest_time() {
            Some(newest_s) if t_s < newest_s => Err(RingError::TimeRegression { t_s, newest_s }),
            _ => Ok(()),
        }
    }

    /// Slot for a new row, evicting the oldest row when full.
    fn claim_slot(&mut self) -> usize {
        if self.len < self.capacity {
            let slot = self.slot(self.len);
            self.len += 1;
            slot
        } else {
            let slot = self.head;
            self.head = (self.head + 1) % self.capacity;
            slot
        }
    }

    /// Appends a row from a frame at time `t_s`; channels whose sample is not valid are stored as gaps.
    pub fn push_frame(&mut self, t_s: f64, frame: &TelemetryFrame) -> Result<(), RingError> {
        self.check_time(t_s)?;
        let slot = self.claim_slot();
        self.times[slot] = t_s;
        for col in &mut self.columns {
            col.values[slot] = frame.f64(col.id).unwrap_or(f64::NAN);
        }
        Ok(())
    }

    /// Appends a row of values in column order (`None` = gap).
    pub fn push_row(&mut self, t_s: f64, values: &[Option<f64>]) -> Result<(), RingError> {
        if values.len() != self.columns.len() {
            return Err(RingError::RowWidth {
                expected: self.columns.len(),
                actual: values.len(),
            });
        }
        self.check_time(t_s)?;
        let slot = self.claim_slot();
        self.times[slot] = t_s;
        for (col, v) in self.columns.iter_mut().zip(values) {
            col.values[slot] = v.filter(|x| x.is_finite()).unwrap_or(f64::NAN);
        }
        Ok(())
    }

    /// Drops every row (capacity and channels are kept).
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Changes the capacity, keeping the newest rows that fit.
    pub fn resize(&mut self, capacity: usize) -> Result<(), RingError> {
        if capacity == 0 {
            return Err(RingError::ZeroCapacity);
        }
        let keep = self.len.min(capacity);
        let skip = self.len - keep;
        let order: Vec<usize> = (skip..self.len).map(|i| self.slot(i)).collect();
        let mut times = vec![f64::NAN; capacity];
        for (dst, &src) in order.iter().enumerate() {
            times[dst] = self.times[src];
        }
        for col in &mut self.columns {
            let mut values = vec![f64::NAN; capacity];
            for (dst, &src) in order.iter().enumerate() {
                values[dst] = col.values[src];
            }
            col.values = values;
        }
        self.times = times;
        self.capacity = capacity;
        self.head = 0;
        self.len = keep;
        Ok(())
    }

    /// Timestamp of logical row `i` (0 = oldest).
    pub fn time_at(&self, i: usize) -> Option<f64> {
        (i < self.len).then(|| self.times[self.slot(i)])
    }

    /// Value of `id` at logical row `i`; `None` for gaps.
    pub fn value_at(&self, id: ChannelId, i: usize) -> Option<f64> {
        if i >= self.len {
            return None;
        }
        let v = self.column(id)?.values[self.slot(i)];
        v.is_finite().then_some(v)
    }

    /// Oldest stored timestamp.
    pub fn oldest_time(&self) -> Option<f64> {
        self.time_at(0)
    }

    /// Newest stored timestamp.
    pub fn newest_time(&self) -> Option<f64> {
        self.len.checked_sub(1).and_then(|i| self.time_at(i))
    }

    /// Newest valid `(time, value)` of `id` (skips trailing gaps).
    pub fn newest(&self, id: ChannelId) -> Option<(f64, f64)> {
        (0..self.len)
            .rev()
            .find_map(|i| Some((self.time_at(i)?, self.value_at(id, i)?)))
    }

    /// Oldest valid `(time, value)` of `id`.
    pub fn oldest(&self, id: ChannelId) -> Option<(f64, f64)> {
        (0..self.len).find_map(|i| Some((self.time_at(i)?, self.value_at(id, i)?)))
    }

    /// Every stored timestamp, oldest first (renderers use these to detect time gaps).
    pub fn times(&self) -> impl Iterator<Item = f64> + '_ {
        (0..self.len).map(|i| self.times[self.slot(i)])
    }

    /// First logical row with time `>= t_s` (binary search; timestamps are non-decreasing).
    fn lower_bound(&self, t_s: f64) -> usize {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.times[self.slot(mid)] < t_s {
                lo = mid + 1
            } else {
                hi = mid
            }
        }
        lo
    }

    /// First logical row with time `> t_s`.
    fn upper_bound(&self, t_s: f64) -> usize {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.times[self.slot(mid)] <= t_s {
                lo = mid + 1
            } else {
                hi = mid
            }
        }
        lo
    }

    /// Logical rows with `t0_s <= time <= t1_s`.
    pub fn range_indices(&self, t0_s: f64, t1_s: f64) -> Range<usize> {
        if t0_s.is_nan() || t1_s.is_nan() || t1_s < t0_s {
            return 0..0;
        }
        self.lower_bound(t0_s)..self.upper_bound(t1_s)
    }

    /// `(time, value)` rows of `id` with `t0_s <= time <= t1_s`; gaps are `None`.
    pub fn range(
        &self,
        id: ChannelId,
        t0_s: f64,
        t1_s: f64,
    ) -> impl Iterator<Item = (f64, Option<f64>)> + '_ {
        let rows = if self.column(id).is_some() {
            self.range_indices(t0_s, t1_s)
        } else {
            0..0
        };
        rows.map(move |i| (self.times[self.slot(i)], self.value_at(id, i)))
    }

    /// Logical row whose timestamp is nearest to `t_s` (earlier row on ties).
    pub fn nearest_index(&self, t_s: f64) -> Option<usize> {
        if self.len == 0 || t_s.is_nan() {
            return None;
        }
        let after = self.lower_bound(t_s);
        if after == 0 {
            return Some(0);
        }
        if after == self.len {
            return Some(self.len - 1);
        }
        let before = after - 1;
        let d_before = t_s - self.times[self.slot(before)];
        let d_after = self.times[self.slot(after)] - t_s;
        Some(if d_after < d_before { after } else { before })
    }

    /// `(time, value)` of the row nearest to `t_s` (value `None` if that row is a gap).
    pub fn nearest(&self, id: ChannelId, t_s: f64) -> Option<(f64, Option<f64>)> {
        self.column(id)?;
        let i = self.nearest_index(t_s)?;
        Some((self.time_at(i)?, self.value_at(id, i)))
    }

    /// Value of `id` at time `t_s`. Continuous channels are linearly interpolated between the bracketing
    /// rows (both must be valid); discrete channels return the last row at or before `t_s` (never
    /// interpolated). Outside the stored time span, or next to a gap, the result is `None`.
    pub fn sample_at_time(&self, id: ChannelId, t_s: f64) -> Option<f64> {
        let col = self.column(id)?;
        if self.len == 0 || !t_s.is_finite() {
            return None;
        }
        let newest = self.newest_time()?;
        let oldest = self.oldest_time()?;
        if t_s < oldest || t_s > newest {
            return None;
        }
        // Last row at or before t_s (exists because t_s >= oldest).
        let prev = self.upper_bound(t_s).checked_sub(1)?;
        let (t0, v0) = (self.times[self.slot(prev)], self.value_at(id, prev)?);
        if col.interpolation == Interpolation::Discrete || t0 == t_s {
            return Some(v0);
        }
        let next = prev + 1;
        let (t1, v1) = (self.time_at(next)?, self.value_at(id, next)?);
        if t1 <= t0 {
            return Some(v0);
        }
        Some(v0 + (v1 - v0) * (t_s - t0) / (t1 - t0))
    }

    /// Contiguous runs of valid values of `id` within `[t0_s, t1_s]`, split at gaps (missing values) and
    /// wherever consecutive timestamps are more than `max_gap_s` apart. Renderers draw one polyline per run.
    pub fn segments(
        &self,
        id: ChannelId,
        t0_s: f64,
        t1_s: f64,
        max_gap_s: f64,
    ) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        if self.column(id).is_none() {
            return out;
        }
        let mut current: Option<Range<usize>> = None;
        for i in self.range_indices(t0_s, t1_s) {
            let valid = self.value_at(id, i).is_some();
            let contiguous = current.as_ref().is_some_and(|r| {
                let prev_t = self.times[self.slot(r.end - 1)];
                self.times[self.slot(i)] - prev_t <= max_gap_s
            });
            match (valid, contiguous) {
                (true, true) => {
                    if let Some(r) = current.as_mut() {
                        r.end = i + 1;
                    }
                }
                (true, false) => {
                    out.extend(current.take());
                    current = Some(i..i + 1);
                }
                (false, _) => out.extend(current.take()),
            }
        }
        out.extend(current);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ch;

    fn ring(cap: usize) -> TelemetryRingBuffer {
        TelemetryRingBuffer::new(cap, &[ch::SPEED, ch::GEAR]).expect("ring")
    }

    #[test]
    fn construction_errors() {
        assert_eq!(
            TelemetryRingBuffer::new(0, &[ch::SPEED]).err(),
            Some(RingError::ZeroCapacity)
        );
        assert_eq!(
            TelemetryRingBuffer::new(4, &[ch::SPEED, ch::SPEED]).err(),
            Some(RingError::DuplicateChannel(ch::SPEED.0))
        );
        assert_eq!(
            TelemetryRingBuffer::new(4, &[ChannelId(0xFFFF)]).err(),
            Some(RingError::UnknownChannel(0xFFFF))
        );
        assert_eq!(
            TelemetryRingBuffer::new(4, &[ch::VEHICLE_NAME]).err(),
            Some(RingError::NonNumericChannel(ch::VEHICLE_NAME.0))
        );
    }

    #[test]
    fn wraps_and_keeps_newest() {
        let mut r = ring(3);
        for i in 0..5 {
            let t = f64::from(i);
            r.push_row(t, &[Some(t * 10.0), Some(t)]).expect("push");
        }
        assert_eq!(r.len(), 3);
        assert_eq!(r.times().collect::<Vec<_>>(), vec![2.0, 3.0, 4.0]);
        assert_eq!(r.oldest(ch::SPEED), Some((2.0, 20.0)));
        assert_eq!(r.newest(ch::SPEED), Some((4.0, 40.0)));
        assert_eq!(
            r.push_row(3.5, &[None, None]),
            Err(RingError::TimeRegression {
                t_s: 3.5,
                newest_s: 4.0
            })
        );
        assert!(matches!(
            r.push_row(f64::NAN, &[None, None]),
            Err(RingError::NonFiniteTime { .. })
        ));
        assert_eq!(
            r.push_row(5.0, &[None]),
            Err(RingError::RowWidth {
                expected: 2,
                actual: 1
            })
        );
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.newest(ch::SPEED), None);
    }

    #[test]
    fn range_and_nearest() {
        let mut r = ring(10);
        for (t, v) in [(0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (3.0, 4.0)] {
            r.push_row(t, &[Some(v), Some(v)]).expect("push");
        }
        assert_eq!(r.range_indices(0.5, 2.0), 1..3);
        assert_eq!(
            r.range(ch::SPEED, 1.0, 2.0).collect::<Vec<_>>(),
            vec![(1.0, Some(2.0)), (2.0, Some(3.0))]
        );
        assert_eq!(
            r.range(ch::RPM, 0.0, 3.0).count(),
            0,
            "unselected channel yields nothing"
        );
        assert_eq!(r.nearest_index(1.4), Some(1));
        assert_eq!(r.nearest_index(1.6), Some(2));
        assert_eq!(r.nearest_index(-5.0), Some(0));
        assert_eq!(r.nearest(ch::SPEED, 99.0), Some((3.0, Some(4.0))));
    }

    #[test]
    fn interpolation_respects_channel_kind_and_gaps() {
        let mut r = ring(10);
        r.push_row(0.0, &[Some(10.0), Some(2.0)]).expect("push");
        r.push_row(1.0, &[Some(20.0), Some(3.0)]).expect("push");
        r.push_row(2.0, &[None, None]).expect("push");
        let speed = r.sample_at_time(ch::SPEED, 0.25).expect("interpolated");
        assert!((speed - 12.5).abs() < 1e-12);
        assert_eq!(
            r.sample_at_time(ch::GEAR, 0.75),
            Some(2.0),
            "discrete: previous sample, never 2.75"
        );
        assert_eq!(r.sample_at_time(ch::SPEED, 1.5), None, "next to a gap");
        assert_eq!(r.sample_at_time(ch::SPEED, 5.0), None, "outside the span");
        assert_eq!(r.sample_at_time(ch::SPEED, 1.0), Some(20.0));
    }

    #[test]
    fn segments_split_on_missing_values_and_time_gaps() {
        let mut r = ring(16);
        let rows = [
            (0.0, Some(1.0)),
            (0.1, Some(1.0)),
            (0.2, None),
            (0.3, Some(1.0)),
            (5.0, Some(1.0)),
            (5.1, Some(1.0)),
        ];
        for (t, v) in rows {
            r.push_row(t, &[v, Some(1.0)]).expect("push");
        }
        assert_eq!(
            r.segments(ch::SPEED, 0.0, 10.0, 0.5),
            vec![0..2, 3..4, 4..6]
        );
    }

    #[test]
    fn resize_keeps_newest_rows_in_order() {
        let mut r = ring(4);
        for i in 0..6 {
            let t = f64::from(i);
            r.push_row(t, &[Some(t), Some(t)]).expect("push");
        }
        r.resize(2).expect("shrink");
        assert_eq!(r.times().collect::<Vec<_>>(), vec![4.0, 5.0]);
        r.resize(5).expect("grow");
        assert_eq!(r.capacity(), 5);
        r.push_row(6.0, &[Some(6.0), Some(6.0)]).expect("push");
        assert_eq!(r.times().collect::<Vec<_>>(), vec![4.0, 5.0, 6.0]);
        assert_eq!(r.value_at(ch::SPEED, 2), Some(6.0));
        assert_eq!(r.resize(0), Err(RingError::ZeroCapacity));
    }

    #[test]
    fn push_frame_stores_only_valid_values() {
        let mut r = ring(4);
        let mut f = TelemetryFrame::new(1, 0, Some(1.0));
        f.set_valid(ch::SPEED, crate::value::Value::F32(50.0))
            .expect("set");
        r.push_frame(1.0, &f).expect("push");
        assert_eq!(r.value_at(ch::SPEED, 0), Some(50.0));
        assert_eq!(
            r.value_at(ch::GEAR, 0),
            None,
            "unavailable gear is a gap, not 0"
        );
    }
}
