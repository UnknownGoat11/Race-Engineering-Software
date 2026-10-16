//! The `UiBatch` v1 envelope (D-023), re-exported from [`telemetry_model::uibatch`].
//!
//! The encoder and the reference parser live in `telemetry-model` so the app-core bridge and this generator
//! share one implementation: the golden vectors are produced by the same code that feeds the WebView.
//! [`encode_vector`] is the generator's infallible wrapper for vectors that are known to fit.

pub use telemetry_model::uibatch::*;

/// [`encode`] for a golden vector. Every vector the generator builds is small and uses only defined flags,
/// so a refusal is a bug in the generator itself and aborts generation with the reason.
pub fn encode_vector(flags: u16, batch_seq: u32, blocks: &[&[u8]]) -> Vec<u8> {
    encode(flags, batch_seq, blocks)
        .unwrap_or_else(|e| panic!("golden UiBatch vector cannot be encoded: {e}"))
}
