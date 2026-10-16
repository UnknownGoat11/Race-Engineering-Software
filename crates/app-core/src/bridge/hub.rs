//! `UiHub`: telemetry history and UI subscriptions (blueprint §3, §12 Lane A).
//!
//! Holds encoded TLV blocks as `Arc<[u8]>` with a keyframe flag (a full snapshot at session start,
//! every 1 s and after a discontinuity), trimmed only at keyframe boundaries (60 s retention,
//! hard cap 48 MiB).
//!
//! A subscribe takes a consistent cut under the hub lock and sends `RESET | BACKFILL` batches
//! (oldest keyframe first, each at most 4 MiB) before any live batch. Each subscription has a
//! bounded queue (1024 blocks, 8 MiB); overflow drops the oldest block and flags `GAP_BEFORE`.
//! An invisible sink pauses its subscription, which is re-primed when visible again.
//! Envelopes come from [`telemetry_model::uibatch`].

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use telemetry_model::uibatch::{
    self, FLAG_BACKFILL, FLAG_GAP_BEFORE, FLAG_RESET, MAX_BATCH_BYTES, UiBatchEncodeError,
};
use tracing::{debug, warn};

use crate::bridge::sink::{SinkClosed, SubscriptionId, UiSink};

/// Maximum duration of history kept in the hub (60 seconds).
pub const MAX_HISTORY_DURATION_NS: u64 = 60 * 1_000_000_000;

/// Hard byte cap for the history buffer (48 MiB).
pub const MAX_HISTORY_BYTES: usize = 48 * 1024 * 1024;

/// Bounded capacity for per-subscription pending block queues (1024 blocks).
pub const MAX_SUB_QUEUE_BLOCKS: usize = 1024;

/// Bounded byte limit for per-subscription pending block queues (8 MiB).
pub const MAX_SUB_QUEUE_BYTES: usize = 8 * 1024 * 1024;

/// One historical entry retained in the [`UiHub`].
#[derive(Clone)]
struct HistoryEntry {
    block: Arc<[u8]>,
    is_keyframe: bool,
    capture_mono_ns: u64,
    gap_before: bool,
}

/// State of an active frontend subscription.
struct SubscriptionState {
    sink: Arc<dyn UiSink>,
    pending_blocks: VecDeque<Arc<[u8]>>,
    pending_bytes: usize,
    gap_pending: bool,
    batch_seq: u32,
    is_paused: bool,
    needs_reprime: bool,
    is_backfilling: bool,
    drops: u64,
}

struct HubState {
    history: VecDeque<HistoryEntry>,
    history_bytes: usize,
    subscriptions: HashMap<SubscriptionId, SubscriptionState>,
    total_ui_dropped: u64,
}

impl HubState {
    /// Trims history strictly at keyframe boundaries.
    ///
    /// Retains at least 60 seconds of history ending at the newest block, while guaranteeing
    /// the oldest retained block is a keyframe. If memory exceeds the 48 MiB cap, older keyframe
    /// segments are pruned until under the cap.
    fn trim_history(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let newest_ts = self.history.back().map_or(0, |e| e.capture_mono_ns);
        let cutoff_ts = newest_ts.saturating_sub(MAX_HISTORY_DURATION_NS);

        loop {
            // Find the index of the next keyframe after index 0:
            let next_kf_idx = self
                .history
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, e)| e.is_keyframe)
                .map(|(i, _)| i);

            match next_kf_idx {
                Some(idx) => {
                    let next_kf_ts = self.history[idx].capture_mono_ns;
                    if next_kf_ts <= cutoff_ts || self.history_bytes > MAX_HISTORY_BYTES {
                        for _ in 0..idx {
                            if let Some(entry) = self.history.pop_front() {
                                self.history_bytes =
                                    self.history_bytes.saturating_sub(entry.block.len());
                            }
                        }
                    } else {
                        break;
                    }
                }
                None => {
                    // If memory hard cap is exceeded with only one keyframe left:
                    if self.history_bytes > MAX_HISTORY_BYTES && self.history.len() > 1 {
                        if let Some(entry) = self.history.pop_front() {
                            self.history_bytes =
                                self.history_bytes.saturating_sub(entry.block.len());
                        }
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

/// Hub managing telemetry history and fan-out to UI subscriptions.
pub struct UiHub {
    inner: Mutex<HubState>,
    next_sub_id: AtomicU64,
}

impl Default for UiHub {
    fn default() -> Self {
        Self::new()
    }
}

impl UiHub {
    /// Creates a new, empty telemetry UI hub.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HubState {
                history: VecDeque::new(),
                history_bytes: 0,
                subscriptions: HashMap::new(),
                total_ui_dropped: 0,
            }),
            next_sub_id: AtomicU64::new(1),
        }
    }

    /// Registers a new UI subscription and immediately delivers backfill batches to it.
    ///
    /// The backfill contains a consistent history starting at the oldest available keyframe,
    /// split into batches of at most 4 MiB. The first batch carries `RESET | BACKFILL`.
    /// While backfilling is in flight, incoming live blocks are buffered in the subscription's
    /// queue without skipping.
    pub fn subscribe(&self, sink: Arc<dyn UiSink>) -> SubscriptionId {
        let sub_id = SubscriptionId(self.next_sub_id.fetch_add(1, Ordering::Relaxed));

        // 1. Under lock, take a consistent cut of history and register subscription in backfilling state.
        let (backfill_batches, is_sink_visible) = {
            let mut state = self.inner.lock();

            // Locate the oldest keyframe in history:
            let kf_start = state.history.iter().position(|e| e.is_keyframe);
            let backfill_entries: Vec<HistoryEntry> = match kf_start {
                Some(idx) => state.history.iter().skip(idx).cloned().collect(),
                None => Vec::new(),
            };

            let visible = sink.is_visible();
            let initial_seq = 0u32;
            let encoded_batches = match pack_history_batches(&backfill_entries, initial_seq) {
                Ok(batches) => batches,
                Err(err) => {
                    warn!(
                        component = "ui-hub",
                        error = %err,
                        "failed to encode backfill batches during subscribe"
                    );
                    Vec::new()
                }
            };

            let batch_count = encoded_batches.len() as u32;
            let sub_state = SubscriptionState {
                sink: Arc::clone(&sink),
                pending_blocks: VecDeque::new(),
                pending_bytes: 0,
                gap_pending: false,
                batch_seq: initial_seq.wrapping_add(batch_count),
                is_paused: !visible,
                needs_reprime: !visible,
                is_backfilling: true,
                drops: 0,
            };

            state.subscriptions.insert(sub_id, sub_state);
            (encoded_batches, visible)
        };

        // 2. Outside lock, send the backfill batches to the sink.
        if is_sink_visible {
            for batch in backfill_batches {
                if let Err(SinkClosed) = sink.send_batch(sub_id, batch) {
                    debug!(
                        component = "ui-hub",
                        sub = %sub_id,
                        "sink closed during initial backfill delivery"
                    );
                    self.unsubscribe(sub_id);
                    return sub_id;
                }
            }
        }

        // 3. Mark backfill complete.
        {
            let mut state = self.inner.lock();
            if let Some(sub) = state.subscriptions.get_mut(&sub_id) {
                sub.is_backfilling = false;
            }
        }

        sub_id
    }

    /// Removes an existing UI subscription.
    pub fn unsubscribe(&self, id: SubscriptionId) {
        let mut state = self.inner.lock();
        state.subscriptions.remove(&id);
    }

    /// Pushes a newly encoded canonical TLV telemetry block to history and all subscriptions.
    pub fn push_block(&self, block: Arc<[u8]>, is_keyframe: bool) {
        self.push_block_with_gap(block, is_keyframe, false);
    }

    /// Pushes a newly encoded block, flagging whether a gap preceded it (e.g. transport reconnect).
    pub fn push_block_with_gap(&self, block: Arc<[u8]>, is_keyframe: bool, gap_before: bool) {
        let capture_mono_ns = extract_capture_mono_ns(&block);
        let block_len = block.len();

        let mut state = self.inner.lock();

        // 1. History update: only accept blocks once the first keyframe is present (or if this is one).
        if is_keyframe || !state.history.is_empty() {
            state.history_bytes = state.history_bytes.saturating_add(block_len);
            state.history.push_back(HistoryEntry {
                block: Arc::clone(&block),
                is_keyframe,
                capture_mono_ns,
                gap_before,
            });
            state.trim_history();
        }

        // 2. Queue into active subscriptions:
        let mut dropped_count: u64 = 0;
        for sub in state.subscriptions.values_mut() {
            // Check visibility state:
            let visible = sub.sink.is_visible();
            if !visible {
                if !sub.is_paused {
                    sub.is_paused = true;
                    sub.needs_reprime = true;
                    sub.pending_blocks.clear();
                    sub.pending_bytes = 0;
                }
                continue;
            } else if sub.is_paused {
                sub.is_paused = false;
                sub.needs_reprime = true;
            }

            if gap_before {
                sub.gap_pending = true;
            }

            // Bounded per-subscription queue (1024 blocks / 8 MiB):
            while sub.pending_blocks.len() >= MAX_SUB_QUEUE_BLOCKS
                || (sub.pending_bytes.saturating_add(block_len) > MAX_SUB_QUEUE_BYTES
                    && !sub.pending_blocks.is_empty())
            {
                if let Some(dropped) = sub.pending_blocks.pop_front() {
                    sub.pending_bytes = sub.pending_bytes.saturating_sub(dropped.len());
                    sub.gap_pending = true;
                    sub.drops = sub.drops.saturating_add(1);
                    dropped_count = dropped_count.saturating_add(1);
                }
            }

            if block_len <= MAX_SUB_QUEUE_BYTES {
                sub.pending_blocks.push_back(Arc::clone(&block));
                sub.pending_bytes = sub.pending_bytes.saturating_add(block_len);
            } else {
                sub.gap_pending = true;
                sub.drops = sub.drops.saturating_add(1);
                dropped_count = dropped_count.saturating_add(1);
            }
        }
        state.total_ui_dropped = state.total_ui_dropped.saturating_add(dropped_count);
    }

    /// Flushes all pending blocks for a specific subscription as one or more `UiBatch`es.
    pub fn flush_subscription(&self, id: SubscriptionId) -> Result<(), SinkClosed> {
        let (sink, batches) = {
            let mut state = self.inner.lock();

            // Check if subscription exists and whether it's backfilling or invisible:
            let (visible, needs_reprime, is_backfilling) = {
                let sub = match state.subscriptions.get(&id) {
                    Some(s) => s,
                    None => return Ok(()),
                };
                (sub.sink.is_visible(), sub.needs_reprime, sub.is_backfilling)
            };

            if is_backfilling {
                return Ok(());
            }

            if !visible {
                if let Some(sub) = state.subscriptions.get_mut(&id) {
                    sub.is_paused = true;
                    sub.needs_reprime = true;
                    sub.pending_blocks.clear();
                    sub.pending_bytes = 0;
                }
                return Ok(());
            }

            // Re-prime after being invisible:
            if needs_reprime {
                let kf_start = state.history.iter().position(|e| e.is_keyframe);
                let backfill_entries: Vec<HistoryEntry> = match kf_start {
                    Some(idx) => state.history.iter().skip(idx).cloned().collect(),
                    None => Vec::new(),
                };

                let sub = match state.subscriptions.get_mut(&id) {
                    Some(s) => s,
                    None => return Ok(()),
                };

                sub.needs_reprime = false;
                sub.is_paused = false;
                sub.pending_blocks.clear();
                sub.pending_bytes = 0;

                let batches = match pack_history_batches(&backfill_entries, sub.batch_seq) {
                    Ok(b) => b,
                    Err(err) => {
                        warn!(
                            component = "ui-hub",
                            sub = %id,
                            error = %err,
                            "failed to encode reprime backfill batches"
                        );
                        Vec::new()
                    }
                };

                sub.batch_seq = sub.batch_seq.wrapping_add(batches.len() as u32);
                (Arc::clone(&sub.sink), batches)
            } else {
                let sub = match state.subscriptions.get_mut(&id) {
                    Some(s) => s,
                    None => return Ok(()),
                };

                if sub.pending_blocks.is_empty() {
                    return Ok(());
                }

                let blocks: Vec<Arc<[u8]>> = sub.pending_blocks.drain(..).collect();
                sub.pending_bytes = 0;
                let gap_before = sub.gap_pending;
                sub.gap_pending = false;

                let batches = match pack_live_batches(&blocks, gap_before, sub.batch_seq) {
                    Ok(b) => b,
                    Err(err) => {
                        warn!(
                            component = "ui-hub",
                            sub = %id,
                            error = %err,
                            "failed to encode live batches"
                        );
                        Vec::new()
                    }
                };

                sub.batch_seq = sub.batch_seq.wrapping_add(batches.len() as u32);
                (Arc::clone(&sub.sink), batches)
            }
        };

        // Outside lock:
        for batch in batches {
            if let Err(SinkClosed) = sink.send_batch(id, batch) {
                self.unsubscribe(id);
                return Err(SinkClosed);
            }
        }

        Ok(())
    }

    /// Flushes all active subscriptions. Typically called at 60 Hz by the bridge pacer.
    pub fn flush_all(&self) {
        let ids: Vec<SubscriptionId> = {
            let state = self.inner.lock();
            state.subscriptions.keys().copied().collect()
        };

        for id in ids {
            let _ = self.flush_subscription(id);
        }
    }

    /// Clears the history buffer (e.g. upon session reset or disconnect).
    pub fn reset_history(&self) {
        let mut state = self.inner.lock();
        state.history.clear();
        state.history_bytes = 0;
    }

    /// Returns the number of blocks and total payload bytes currently stored in history.
    pub fn history_stats(&self) -> (usize, usize) {
        let state = self.inner.lock();
        (state.history.len(), state.history_bytes)
    }

    /// Returns the number of currently active subscriptions.
    pub fn subscription_count(&self) -> usize {
        let state = self.inner.lock();
        state.subscriptions.len()
    }

    /// Returns the cumulative number of blocks dropped across all subscriptions due to queue overflow.
    pub fn total_ui_dropped(&self) -> u64 {
        let state = self.inner.lock();
        state.total_ui_dropped
    }

    /// Returns the number of dropped blocks for a specific subscription.
    pub fn subscription_dropped(&self, id: SubscriptionId) -> Option<u64> {
        let state = self.inner.lock();
        state.subscriptions.get(&id).map(|s| s.drops)
    }
}

/// Extracts monotonic nanoseconds from the TLV block header (offsets 2..10).
fn extract_capture_mono_ns(block: &[u8]) -> u64 {
    if block.len() >= 10 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&block[2..10]);
        u64::from_le_bytes(bytes)
    } else {
        0
    }
}

/// Packs historical entries into one or more batches of at most [`MAX_BATCH_BYTES`].
fn pack_history_batches(
    entries: &[HistoryEntry],
    mut seq: u32,
) -> Result<Vec<Vec<u8>>, UiBatchEncodeError> {
    if entries.is_empty() {
        let flags = FLAG_RESET | FLAG_BACKFILL;
        let batch = uibatch::encode(flags, seq, &[])?;
        return Ok(vec![batch]);
    }

    let mut batches = Vec::new();
    let mut current_blocks: Vec<&[u8]> = Vec::new();
    let mut current_bytes = uibatch::HEADER_BYTES;
    let mut is_first_batch = true;
    let mut current_gap = entries[0].gap_before;

    for entry in entries {
        let block_cost = uibatch::BLOCK_LEN_BYTES.saturating_add(entry.block.len());
        let would_overflow = current_bytes.saturating_add(block_cost) > MAX_BATCH_BYTES;
        let is_gap = entry.gap_before;

        if !current_blocks.is_empty() && (is_gap || would_overflow) {
            let mut flags = FLAG_BACKFILL;
            if is_first_batch {
                flags |= FLAG_RESET;
                is_first_batch = false;
            }
            if current_gap {
                flags |= FLAG_GAP_BEFORE;
            }
            let batch = uibatch::encode(flags, seq, &current_blocks)?;
            batches.push(batch);
            seq = seq.wrapping_add(1);
            current_blocks.clear();
            current_bytes = uibatch::HEADER_BYTES;
            current_gap = is_gap;
        }

        current_blocks.push(&entry.block);
        current_bytes = current_bytes.saturating_add(block_cost);
    }

    if !current_blocks.is_empty() {
        let mut flags = FLAG_BACKFILL;
        if is_first_batch {
            flags |= FLAG_RESET;
        }
        if current_gap {
            flags |= FLAG_GAP_BEFORE;
        }
        let batch = uibatch::encode(flags, seq, &current_blocks)?;
        batches.push(batch);
    }

    Ok(batches)
}

/// Packs live blocks into batches of at most [`MAX_BATCH_BYTES`].
fn pack_live_batches(
    blocks: &[Arc<[u8]>],
    gap_before: bool,
    mut seq: u32,
) -> Result<Vec<Vec<u8>>, UiBatchEncodeError> {
    if blocks.is_empty() {
        return Ok(Vec::new());
    }

    let mut batches = Vec::new();
    let mut current_blocks: Vec<&[u8]> = Vec::new();
    let mut current_bytes = uibatch::HEADER_BYTES;
    let mut is_first_batch = true;

    for block in blocks {
        let block_cost = uibatch::BLOCK_LEN_BYTES.saturating_add(block.len());
        if !current_blocks.is_empty() && current_bytes.saturating_add(block_cost) > MAX_BATCH_BYTES
        {
            let flags = if is_first_batch && gap_before {
                FLAG_GAP_BEFORE
            } else {
                0
            };
            is_first_batch = false;
            let batch = uibatch::encode(flags, seq, &current_blocks)?;
            batches.push(batch);
            seq = seq.wrapping_add(1);
            current_blocks.clear();
            current_bytes = uibatch::HEADER_BYTES;
        }

        current_blocks.push(block);
        current_bytes = current_bytes.saturating_add(block_cost);
    }

    if !current_blocks.is_empty() {
        let flags = if is_first_batch && gap_before {
            FLAG_GAP_BEFORE
        } else {
            0
        };
        let batch = uibatch::encode(flags, seq, &current_blocks)?;
        batches.push(batch);
    }

    Ok(batches)
}
