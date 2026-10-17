//! The frame decoder never allocates more than `MAX_PAYLOAD_BYTES` in one allocation, whatever the
//! input (measured with a counting global allocator armed only around decoder calls).

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bytes::BytesMut;
use proptest::prelude::*;
use protocol::FrameDecoder;
use protocol::frame::{HEADER_BYTES, MAX_PAYLOAD_BYTES};

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn record(size: usize) {
    // `try_with` because the allocator may run during thread-local teardown.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LARGEST.try_with(|l| l.set(l.get().max(size)));
        }
    });
}

// SAFETY: delegates every call to the system allocator unchanged; only records sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: same layout contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs the decoder over `data` delivered in `chunk`-sized reads; returns the largest single
/// allocation made inside decoder calls.
fn largest_decoder_allocation(data: &[u8], chunk: usize) -> usize {
    LARGEST.with(|l| l.set(0));
    let mut dec = FrameDecoder::new();
    let mut buf = BytesMut::new();
    for piece in data.chunks(chunk.max(1)) {
        buf.extend_from_slice(piece); // test-side growth, not measured
        loop {
            ARMED.with(|a| a.set(true));
            let r = dec.decode_frame(&mut buf);
            ARMED.with(|a| a.set(false));
            match r {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        if dec.is_poisoned() {
            break;
        }
    }
    LARGEST.with(Cell::get)
}

#[test]
fn oversized_header_allocates_nothing_significant() {
    let f = common::delta_frame(1);
    let mut header = f.as_bytes()[..HEADER_BYTES].to_vec();
    for len in [u32::MAX, MAX_PAYLOAD_BYTES as u32 + 1, 0x8000_0000] {
        header[8..12].copy_from_slice(&len.to_le_bytes());
        let largest = largest_decoder_allocation(&header, HEADER_BYTES);
        assert!(
            largest < 1024,
            "decoder allocated {largest} bytes for payload_len {len:#x}"
        );
    }
}

#[test]
fn valid_traffic_stays_within_the_bound() {
    let mut data = Vec::new();
    for f in common::all_frames() {
        data.extend_from_slice(f.as_bytes());
    }
    for chunk in [1, 7, 512, data.len()] {
        let largest = largest_decoder_allocation(&data, chunk);
        assert!(largest <= MAX_PAYLOAD_BYTES, "largest allocation {largest}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn hostile_input_never_allocates_more_than_max_payload(
        mut data in proptest::collection::vec(any::<u8>(), 0..1024),
        chunk in 1usize..128,
        valid_prefix in any::<bool>(),
    ) {
        if valid_prefix && data.len() >= 3 {
            data[0] = b'L';
            data[1] = b'R';
            data[2] = protocol::PROTOCOL_MAJOR;
        }
        let largest = largest_decoder_allocation(&data, chunk);
        prop_assert!(largest <= MAX_PAYLOAD_BYTES, "largest allocation {}", largest);
    }

    #[test]
    fn mutated_frames_never_allocate_more_than_max_payload(
        flips in proptest::collection::vec((any::<prop::sample::Index>(), 1u8..=255), 1..4),
        chunk in 1usize..4096,
    ) {
        let mut data = Vec::new();
        for f in common::all_frames() {
            data.extend_from_slice(f.as_bytes());
        }
        for (idx, xor) in &flips {
            let i = idx.index(data.len());
            data[i] ^= xor;
        }
        let largest = largest_decoder_allocation(&data, chunk);
        prop_assert!(largest <= MAX_PAYLOAD_BYTES, "largest allocation {}", largest);
    }
}
