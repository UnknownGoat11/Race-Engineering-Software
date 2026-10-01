//! Tyre temperature edge mapping: LMU left/centre/right (car frame) → inner/middle/outer (wheel frame).
//!
//! ASSUMPTION **A-LMU-016** (docs/ASSUMPTIONS.md, docs/LMU_CHANNEL_MAP.md §11), pending Gate-1:
//! `mTemperature[3]` (and `mTireInnerLayerTemperature[3]`) are ordered **left, centre, right in the car
//! frame**; the header explicitly says this is *not* inside/centre/outside. LMU's local +x points out of the
//! car's LEFT side, so index 0 is the edge on the car's left:
//!
//! | wheel | `[0]` (car-left edge) | `[1]` | `[2]` (car-right edge) |
//! |---|---|---|---|
//! | FL, RL (left side)  | **outer** | middle | **inner** |
//! | FR, RR (right side) | **inner** | middle | **outer** |
//!
//! On a left-side wheel the car-left edge faces away from the car (outboard); on a right-side wheel it faces
//! the car (inboard). The raw left/centre/right channels are always published as well, so a refuted
//! assumption only changes [`edge_indices`].

use telemetry_model::registry::Wheel;

/// Index of the car-frame left edge in LMU's 3-element tyre temperature arrays.
pub const LMU_TYRE_LEFT_INDEX: usize = 0;
/// Index of the centre in LMU's 3-element tyre temperature arrays.
pub const LMU_TYRE_CENTRE_INDEX: usize = 1;
/// Index of the car-frame right edge in LMU's 3-element tyre temperature arrays.
pub const LMU_TYRE_RIGHT_INDEX: usize = 2;

/// Side of the car a wheel is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarSide {
    Left,
    Right,
}

/// Side of the car `wheel` is on (`mWheel[]` order FL, FR, RL, RR, A-LMU-019).
pub const fn wheel_side(wheel: Wheel) -> CarSide {
    match wheel {
        Wheel::FrontLeft | Wheel::RearLeft => CarSide::Left,
        Wheel::FrontRight | Wheel::RearRight => CarSide::Right,
    }
}

/// Inner/middle/outer edge values of one tyre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TyreEdges<T> {
    /// Edge facing the car's centreline (inboard).
    pub inner: T,
    pub middle: T,
    /// Edge facing away from the car (outboard).
    pub outer: T,
}

/// `(inner, middle, outer)` indices into LMU's left/centre/right array for `wheel` (A-LMU-016). This is the
/// single place the side mapping is decided.
pub const fn edge_indices(wheel: Wheel) -> TyreEdges<usize> {
    match wheel_side(wheel) {
        // Left-side wheel: the car-left edge is outboard.
        CarSide::Left => TyreEdges {
            inner: LMU_TYRE_RIGHT_INDEX,
            middle: LMU_TYRE_CENTRE_INDEX,
            outer: LMU_TYRE_LEFT_INDEX,
        },
        // Right-side wheel: the car-left edge is inboard.
        CarSide::Right => TyreEdges {
            inner: LMU_TYRE_LEFT_INDEX,
            middle: LMU_TYRE_CENTRE_INDEX,
            outer: LMU_TYRE_RIGHT_INDEX,
        },
    }
}

/// Maps a left/centre/right array (car frame) of `wheel` to inner/middle/outer.
pub fn lcr_to_edges<T: Copy>(wheel: Wheel, left_centre_right: [T; 3]) -> TyreEdges<T> {
    let idx = edge_indices(wheel);
    TyreEdges {
        inner: left_centre_right[idx.inner],
        middle: left_centre_right[idx.middle],
        outer: left_centre_right[idx.outer],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_wheels_have_the_outer_edge_at_index_0() {
        for wheel in [Wheel::FrontLeft, Wheel::RearLeft] {
            let e = lcr_to_edges(wheel, [10.0, 20.0, 30.0]);
            assert_eq!(
                (e.outer, e.middle, e.inner),
                (10.0, 20.0, 30.0),
                "{wheel:?}"
            );
        }
    }

    #[test]
    fn right_wheels_have_the_inner_edge_at_index_0() {
        for wheel in [Wheel::FrontRight, Wheel::RearRight] {
            let e = lcr_to_edges(wheel, [10.0, 20.0, 30.0]);
            assert_eq!(
                (e.inner, e.middle, e.outer),
                (10.0, 20.0, 30.0),
                "{wheel:?}"
            );
        }
    }

    #[test]
    fn mapping_matches_the_channel_map_table() {
        // docs/LMU_CHANNEL_MAP.md §18: inner FL/RL [2], FR/RR [0]; outer FL/RL [0], FR/RR [2].
        let inner: Vec<usize> = Wheel::ALL.iter().map(|&w| edge_indices(w).inner).collect();
        let outer: Vec<usize> = Wheel::ALL.iter().map(|&w| edge_indices(w).outer).collect();
        assert_eq!(inner, vec![2, 0, 2, 0]);
        assert_eq!(outer, vec![0, 2, 0, 2]);
        assert!(Wheel::ALL.iter().all(|&w| edge_indices(w).middle == 1));
    }
}
