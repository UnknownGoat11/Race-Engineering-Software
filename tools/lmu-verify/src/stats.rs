//! Streaming statistics with bounded memory, so a capture of any length is analysed in constant space.
//!
//! - [`Summary`]: exact count / min / max / mean, plus the 1st, 50th and 99th percentiles. Percentiles are
//!   **exact** (nearest rank over a value → count map) while a series has at most [`EXACT_DISTINCT_MAX`]
//!   distinct values — every integer, boolean and enum channel in practice — and otherwise **P² estimates**
//!   (Jain & Chlamtac, "The P² algorithm for dynamic calculation of quantiles and histograms without storing
//!   observations", CACM 28(10), 1985): five markers per quantile, O(1) memory, exact for the first five
//!   samples, deterministic for a given input order. [`SummaryStats::exact`] says which method applied.
//! - [`DurationHistogram`]: log-linear buckets for nanosecond intervals. Values below 1,024 ns are exact;
//!   above, each power of two is split into 512 buckets (relative bucket width ≤ 2⁻⁹ ≈ 0.2 %). Quantiles
//!   report the bucket midpoint clamped to the exact min/max.
//! - [`Correlation`] (Welford co-moments), [`Agreement`], [`Mean`] and a bounded [`TopK`].
//!
//! Non-finite inputs are ignored everywhere (they are counted by the callers, never folded into a statistic).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::Serialize;

/// Number of P² markers.
const P2_MARKERS: usize = 5;
/// Index of the P² marker that carries the quantile estimate.
const P2_ESTIMATE_MARKER: usize = 2;

/// Distinct values kept for exact percentiles; a series with more switches to P² estimates.
pub const EXACT_DISTINCT_MAX: usize = 1_024;

/// Percentiles reported by [`Summary`].
const P01: f64 = 0.01;
const P50: f64 = 0.50;
const P99: f64 = 0.99;

/// Nearest-rank quantile of an ascending slice (`None` when empty).
fn nearest_rank(sorted: &[f64], p: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    // Rank in 1..=n; bounded, so the casts cannot saturate.
    let rank = ((p * n as f64).ceil() as usize).clamp(1, n);
    sorted.get(rank - 1).copied()
}

/// One P² quantile estimator.
#[derive(Debug, Clone)]
pub struct P2Quantile {
    p: f64,
    count: u64,
    /// Marker heights (the first five samples until five have been seen).
    heights: [f64; P2_MARKERS],
    /// Actual marker positions (1-based ranks).
    positions: [f64; P2_MARKERS],
    /// Desired marker positions.
    desired: [f64; P2_MARKERS],
    /// Desired-position increments per sample.
    increments: [f64; P2_MARKERS],
}

impl P2Quantile {
    /// An estimator of quantile `p` (clamped to 0..=1).
    pub fn new(p: f64) -> Self {
        let p = if p.is_finite() {
            p.clamp(0.0, 1.0)
        } else {
            P50
        };
        Self {
            p,
            count: 0,
            heights: [0.0; P2_MARKERS],
            positions: [1.0, 2.0, 3.0, 4.0, 5.0],
            desired: [1.0, 1.0 + 2.0 * p, 1.0 + 4.0 * p, 3.0 + 2.0 * p, 5.0],
            increments: [0.0, p / 2.0, p, (1.0 + p) / 2.0, 1.0],
        }
    }

    /// Adds one observation (non-finite values are ignored).
    pub fn push(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        if self.count < P2_MARKERS as u64 {
            if let Some(slot) = usize::try_from(self.count)
                .ok()
                .and_then(|i| self.heights.get_mut(i))
            {
                *slot = x;
            }
            self.count += 1;
            if self.count == P2_MARKERS as u64 {
                self.heights.sort_by(f64::total_cmp);
            }
            return;
        }
        self.count += 1;
        let h = &mut self.heights;
        // Cell k such that heights[k] <= x < heights[k + 1], extending the extremes.
        let k = if x < h[0] {
            h[0] = x;
            0
        } else if x < h[1] {
            0
        } else if x < h[2] {
            1
        } else if x < h[3] {
            2
        } else if x <= h[4] {
            3
        } else {
            h[4] = x;
            3
        };
        for n in self.positions.iter_mut().skip(k + 1) {
            *n += 1.0;
        }
        for (d, inc) in self.desired.iter_mut().zip(self.increments) {
            *d += inc;
        }
        for i in 1..P2_MARKERS - 1 {
            self.adjust(i);
        }
    }

    /// Moves marker `i` (1..=3) towards its desired position (piecewise-parabolic, else linear).
    fn adjust(&mut self, i: usize) {
        let d = self.desired[i] - self.positions[i];
        let q = &mut self.heights;
        let n = &mut self.positions;
        let right_gap = n[i + 1] - n[i];
        let left_gap = n[i - 1] - n[i];
        if !((d >= 1.0 && right_gap > 1.0) || (d <= -1.0 && left_gap < -1.0)) {
            return;
        }
        let s = d.signum();
        let parabolic = q[i]
            + s / (n[i + 1] - n[i - 1])
                * ((n[i] - n[i - 1] + s) * (q[i + 1] - q[i]) / (n[i + 1] - n[i])
                    + (n[i + 1] - n[i] - s) * (q[i] - q[i - 1]) / (n[i] - n[i - 1]));
        q[i] = if q[i - 1] < parabolic && parabolic < q[i + 1] {
            parabolic
        } else {
            let j = if s > 0.0 { i + 1 } else { i - 1 };
            q[i] + s * (q[j] - q[i]) / (n[j] - n[i])
        };
        n[i] += s;
    }

    /// The current estimate (`None` before the first sample; exact for up to five samples).
    pub fn estimate(&self) -> Option<f64> {
        match self.count {
            0 => None,
            c if c < P2_MARKERS as u64 => {
                let len = usize::try_from(c).unwrap_or(0);
                let mut first: Vec<f64> = self.heights.get(..len).unwrap_or(&[]).to_vec();
                first.sort_by(f64::total_cmp);
                nearest_rank(&first, self.p)
            }
            _ => Some(self.heights[P2_ESTIMATE_MARKER]),
        }
    }
}

/// A finite `f64` with a total order (map key for exact percentiles).
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrdF64(f64);

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Count, extremes, mean and percentiles of a series (see the module docs for the method).
#[derive(Debug, Clone)]
pub struct Summary {
    count: u64,
    min: f64,
    max: f64,
    sum: f64,
    q01: P2Quantile,
    q50: P2Quantile,
    q99: P2Quantile,
    /// Value → count while the series has at most [`EXACT_DISTINCT_MAX`] distinct values.
    exact: Option<BTreeMap<OrdF64, u64>>,
}

impl Default for Summary {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable result of a [`Summary`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SummaryStats {
    /// Number of values.
    pub count: u64,
    /// Smallest value.
    pub min: f64,
    /// 1st percentile.
    pub p1: f64,
    /// Median.
    pub median: f64,
    /// 99th percentile.
    pub p99: f64,
    /// Largest value.
    pub max: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// `true`: percentiles are exact (nearest rank); `false`: P² estimates.
    pub exact: bool,
}

impl Summary {
    /// An empty summary.
    pub fn new() -> Self {
        Self {
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sum: 0.0,
            q01: P2Quantile::new(P01),
            q50: P2Quantile::new(P50),
            q99: P2Quantile::new(P99),
            exact: Some(BTreeMap::new()),
        }
    }

    /// Adds one value (non-finite values are ignored).
    pub fn push(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.count += 1;
        self.min = self.min.min(x);
        self.max = self.max.max(x);
        self.sum += x;
        self.q01.push(x);
        self.q50.push(x);
        self.q99.push(x);
        if let Some(map) = &mut self.exact {
            *map.entry(OrdF64(x)).or_insert(0) += 1;
            if map.len() > EXACT_DISTINCT_MAX {
                self.exact = None;
            }
        }
    }

    /// Number of values.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Smallest value.
    pub fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    /// Largest value.
    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    /// Arithmetic mean.
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }

    /// Exact nearest-rank quantile from the value map (`None` once the map was dropped).
    fn exact_quantile(&self, p: f64) -> Option<f64> {
        let map = self.exact.as_ref()?;
        if self.count == 0 {
            return None;
        }
        let target = ((p * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut cumulative = 0u64;
        for (value, count) in map {
            cumulative += count;
            if cumulative >= target {
                return Some(value.0);
            }
        }
        map.keys().next_back().map(|v| v.0)
    }

    fn quantile(&self, p: f64, estimator: &P2Quantile) -> Option<f64> {
        self.exact_quantile(p).or_else(|| estimator.estimate())
    }

    /// Median (exact or P²).
    pub fn median(&self) -> Option<f64> {
        self.quantile(P50, &self.q50)
    }

    /// 1st percentile (exact or P²).
    pub fn p01(&self) -> Option<f64> {
        self.quantile(P01, &self.q01)
    }

    /// 99th percentile (exact or P²).
    pub fn p99(&self) -> Option<f64> {
        self.quantile(P99, &self.q99)
    }

    /// Everything at once (`None` while empty).
    pub fn stats(&self) -> Option<SummaryStats> {
        Some(SummaryStats {
            count: self.count,
            min: self.min()?,
            p1: self.p01()?,
            median: self.median()?,
            p99: self.p99()?,
            max: self.max()?,
            mean: self.mean()?,
            exact: self.exact.is_some(),
        })
    }
}

/// Sub-bucket resolution of [`DurationHistogram`] (2^10 = 1,024 sub-buckets).
const SUB_BUCKET_BITS: u32 = 10;
/// Values below this are stored exactly.
const SUB_BUCKETS: u64 = 1 << SUB_BUCKET_BITS;
/// Buckets per power of two above [`SUB_BUCKETS`].
const HALF_BUCKETS: u64 = SUB_BUCKETS / 2;

/// Bucket index of a value (see the module docs).
fn bucket_of(v: u64) -> u32 {
    if v < SUB_BUCKETS {
        return u32::try_from(v).unwrap_or(u32::MAX);
    }
    // msb >= SUB_BUCKET_BITS here, so shift >= 1 and the mantissa lies in [HALF_BUCKETS, SUB_BUCKETS).
    let msb = 63 - v.leading_zeros();
    let shift = msb - (SUB_BUCKET_BITS - 1);
    let mantissa = v >> shift;
    let index = SUB_BUCKETS + u64::from(shift - 1) * HALF_BUCKETS + (mantissa - HALF_BUCKETS);
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// `(lower bound, width)` of a bucket.
fn bucket_range(index: u32) -> (u64, u64) {
    let i = u64::from(index);
    if i < SUB_BUCKETS {
        return (i, 1);
    }
    let k = i - SUB_BUCKETS;
    let shift = k / HALF_BUCKETS + 1;
    let mantissa = k % HALF_BUCKETS + HALF_BUCKETS;
    (mantissa << shift, 1u64 << shift)
}

/// Log-linear histogram of nanosecond durations (bounded: at most 28,672 buckets).
#[derive(Debug, Clone, Default)]
pub struct DurationHistogram {
    buckets: BTreeMap<u32, u64>,
    count: u64,
    min: Option<u64>,
    max: Option<u64>,
    sum: u128,
}

impl DurationHistogram {
    /// Records one duration.
    pub fn record(&mut self, ns: u64) {
        *self.buckets.entry(bucket_of(ns)).or_insert(0) += 1;
        self.count += 1;
        self.min = Some(self.min.map_or(ns, |m| m.min(ns)));
        self.max = Some(self.max.map_or(ns, |m| m.max(ns)));
        self.sum += u128::from(ns);
    }

    /// Number of recorded durations.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Shortest duration.
    pub fn min_ns(&self) -> Option<u64> {
        self.min
    }

    /// Longest duration.
    pub fn max_ns(&self) -> Option<u64> {
        self.max
    }

    /// Mean duration.
    pub fn mean_ns(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum as f64 / self.count as f64)
    }

    /// Representative value of a bucket: its exact value below 1,024 ns, else its midpoint, clamped to the
    /// observed extremes.
    fn representative(&self, index: u32) -> f64 {
        let (lower, width) = bucket_range(index);
        let mid = if width == 1 {
            lower as f64
        } else {
            lower as f64 + width as f64 / 2.0
        };
        let lo = self.min.map_or(mid, |m| m as f64);
        let hi = self.max.map_or(mid, |m| m as f64);
        mid.clamp(lo.min(hi), hi.max(lo))
    }

    /// Nearest-rank quantile `q` (0..=1) in nanoseconds (bucket resolution).
    pub fn quantile_ns(&self, q: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let q = if q.is_finite() {
            q.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let target = ((q * self.count as f64).ceil() as u64).clamp(1, self.count);
        // The extremes are known exactly.
        if target == self.count {
            return self.max.map(|m| m as f64);
        }
        if target == 1 {
            return self.min.map(|m| m as f64);
        }
        let mut cumulative = 0u64;
        for (&index, &count) in &self.buckets {
            cumulative += count;
            if cumulative >= target {
                return Some(self.representative(index));
            }
        }
        self.max.map(|m| m as f64)
    }

    /// Durations whose bucket representative exceeds `threshold_ns` (bucket resolution, see module docs).
    pub fn count_above(&self, threshold_ns: f64) -> u64 {
        self.buckets
            .iter()
            .filter(|&(&index, _)| self.representative(index) > threshold_ns)
            .map(|(_, &count)| count)
            .sum()
    }
}

/// Streaming Pearson correlation and least-squares slope (Welford co-moments).
#[derive(Debug, Clone, Default)]
pub struct Correlation {
    n: u64,
    mean_x: f64,
    mean_y: f64,
    m2x: f64,
    m2y: f64,
    cxy: f64,
}

impl Correlation {
    /// Adds one pair (pairs with a non-finite member are ignored).
    pub fn push(&mut self, x: f64, y: f64) {
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        self.n += 1;
        let n = self.n as f64;
        let dx = x - self.mean_x;
        self.mean_x += dx / n;
        let dy = y - self.mean_y;
        self.mean_y += dy / n;
        self.m2x += dx * (x - self.mean_x);
        self.m2y += dy * (y - self.mean_y);
        self.cxy += dx * (y - self.mean_y);
    }

    /// Number of pairs.
    pub fn count(&self) -> u64 {
        self.n
    }

    /// Pearson r (`None` with fewer than 3 pairs or a constant series).
    pub fn r(&self) -> Option<f64> {
        (self.n >= 3 && self.m2x > 0.0 && self.m2y > 0.0)
            .then(|| self.cxy / (self.m2x * self.m2y).sqrt())
    }

    /// Least-squares slope of y on x.
    pub fn slope(&self) -> Option<f64> {
        (self.n >= 2 && self.m2x > 0.0).then(|| self.cxy / self.m2x)
    }
}

/// Fraction of observations satisfying a condition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Agreement {
    /// Observations.
    pub total: u64,
    /// Observations that agreed.
    pub agree: u64,
}

impl Agreement {
    /// Records one observation.
    pub fn push(&mut self, agrees: bool) {
        self.total += 1;
        if agrees {
            self.agree += 1;
        }
    }

    /// `agree / total` (`None` while empty).
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| self.agree as f64 / self.total as f64)
    }
}

/// Streaming mean.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mean {
    n: u64,
    sum: f64,
}

impl Mean {
    /// Adds one value (non-finite values are ignored).
    pub fn push(&mut self, x: f64) {
        if x.is_finite() {
            self.n += 1;
            self.sum += x;
        }
    }

    /// Adds another mean's values.
    pub fn merge(&mut self, other: &Self) {
        self.n += other.n;
        self.sum += other.sum;
    }

    /// Number of values.
    pub fn count(&self) -> u64 {
        self.n
    }

    /// The mean (`None` while empty).
    pub fn mean(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum / self.n as f64)
    }
}

/// The `k` largest values seen, with a payload; ties keep the earliest. Bounded by `k`.
#[derive(Debug, Clone)]
pub struct TopK<T> {
    k: usize,
    seq: u64,
    items: Vec<(f64, u64, T)>,
}

impl<T> TopK<T> {
    /// Keeps the `k` largest values.
    pub fn new(k: usize) -> Self {
        Self {
            k,
            seq: 0,
            items: Vec::with_capacity(k),
        }
    }

    /// Offers one value (non-finite values are ignored).
    pub fn push(&mut self, value: f64, item: T) {
        if !value.is_finite() || self.k == 0 {
            return;
        }
        let seq = self.seq;
        self.seq += 1;
        if self.items.len() < self.k {
            self.items.push((value, seq, item));
            return;
        }
        // The smallest value; among equal values the latest one is evicted first.
        let weakest = self
            .items
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.0.total_cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)))
            .map(|(i, entry)| (i, entry.0));
        if let Some((i, smallest)) = weakest
            && value > smallest
            && let Some(slot) = self.items.get_mut(i)
        {
            *slot = (value, seq, item);
        }
    }

    /// Largest first; equal values in arrival order.
    pub fn into_sorted(mut self) -> Vec<(f64, T)> {
        self.items
            .sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.items.into_iter().map(|(v, _, t)| (v, t)).collect()
    }

    /// Largest first, by reference.
    pub fn sorted(&self) -> Vec<(f64, &T)> {
        let mut refs: Vec<&(f64, u64, T)> = self.items.iter().collect();
        refs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        refs.into_iter().map(|(v, _, t)| (*v, t)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random sequence (64-bit LCG, Knuth MMIX constants) in [0, 1).
    fn lcg(n: usize) -> Vec<f64> {
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 11) as f64 / (1u64 << 53) as f64
            })
            .collect()
    }

    #[test]
    fn p2_tracks_quantiles_of_a_uniform_stream() {
        let data = lcg(20_000);
        let mut p50 = P2Quantile::new(0.5);
        let mut p99 = P2Quantile::new(0.99);
        let mut p01 = P2Quantile::new(0.01);
        for &x in &data {
            p50.push(x);
            p99.push(x);
            p01.push(x);
        }
        let mut sorted = data.clone();
        sorted.sort_by(f64::total_cmp);
        for (est, p) in [(&p50, 0.5), (&p99, 0.99), (&p01, 0.01)] {
            let exact = nearest_rank(&sorted, p).expect("exact");
            let e = est.estimate().expect("estimate");
            assert!((e - exact).abs() < 0.01, "p{p}: P² {e} vs exact {exact}");
        }
    }

    #[test]
    fn p2_is_exact_for_tiny_inputs_and_ignores_non_finite() {
        let mut q = P2Quantile::new(0.5);
        assert_eq!(q.estimate(), None);
        for x in [5.0, f64::NAN, 1.0, 3.0, f64::INFINITY] {
            q.push(x);
        }
        assert_eq!(q.estimate(), Some(3.0));
    }

    #[test]
    fn summary_is_exact_for_discrete_series_and_falls_back_to_p2() {
        let mut s = Summary::new();
        for i in 0..1_000 {
            s.push(f64::from(i % 7));
        }
        let st = s.stats().expect("stats");
        assert!(st.exact);
        assert_eq!((st.min, st.median, st.max), (0.0, 3.0, 6.0));
        assert_eq!(st.p1, 0.0);
        assert_eq!(st.p99, 6.0);

        let mut c = Summary::new();
        for x in lcg(5_000) {
            c.push(x * 100.0);
        }
        let st = c.stats().expect("stats");
        assert!(!st.exact, "more than EXACT_DISTINCT_MAX distinct values");
        assert!((st.median - 50.0).abs() < 2.0, "{st:?}");
        assert!(st.min >= 0.0 && st.max < 100.0);
        assert_eq!(Summary::new().stats(), None);
    }

    #[test]
    fn duration_buckets_round_trip_and_bound_the_error() {
        for v in [
            0u64,
            1,
            1_023,
            1_024,
            1_025,
            2_047,
            2_048,
            9_999_999,
            10_000_000,
            u64::MAX,
        ] {
            let (lower, width) = bucket_range(bucket_of(v));
            assert!(lower <= v && v - lower < width, "{v}: [{lower}, +{width})");
            if v >= SUB_BUCKETS {
                assert!(width as f64 / lower as f64 <= 1.0 / HALF_BUCKETS as f64 + 1e-12);
            }
        }
        assert!(bucket_of(u64::MAX) < 28_672);
    }

    #[test]
    fn duration_histogram_quantiles_and_gap_counts() {
        let mut h = DurationHistogram::default();
        assert_eq!(h.quantile_ns(0.5), None);
        for _ in 0..990 {
            h.record(10_000_000);
        }
        for _ in 0..10 {
            h.record(50_000_000);
        }
        let median = h.quantile_ns(0.5).expect("median");
        assert!(
            (median - 10_000_000.0).abs() / 10_000_000.0 < 0.002,
            "{median}"
        );
        assert_eq!(h.quantile_ns(1.0), Some(50_000_000.0));
        assert_eq!(h.count_above(3.0 * median), 10);
        assert_eq!(h.min_ns(), Some(10_000_000));
        assert_eq!(h.count(), 1_000);
    }

    #[test]
    fn correlation_slope_agreement_mean_topk() {
        let mut c = Correlation::default();
        for i in 0..100 {
            let x = f64::from(i);
            c.push(x, 2.0 * x + 1.0);
        }
        assert!((c.r().expect("r") - 1.0).abs() < 1e-12);
        assert!((c.slope().expect("slope") - 2.0).abs() < 1e-12);
        let mut inverted = Correlation::default();
        for i in 0..100 {
            inverted.push(f64::from(i), -f64::from(i));
        }
        assert!((inverted.r().expect("r") + 1.0).abs() < 1e-12);

        let mut a = Agreement::default();
        a.push(true);
        a.push(false);
        assert_eq!(a.fraction(), Some(0.5));

        let mut m = Mean::default();
        m.push(1.0);
        m.push(f64::NAN);
        m.push(3.0);
        assert_eq!(m.mean(), Some(2.0));

        let mut top = TopK::new(2);
        for (v, tag) in [(1.0, "a"), (5.0, "b"), (3.0, "c"), (5.0, "d"), (0.5, "e")] {
            top.push(v, tag);
        }
        assert_eq!(top.into_sorted(), vec![(5.0, "b"), (5.0, "d")]);
    }
}
