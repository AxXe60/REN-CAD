//! Synthetic scan fixtures shared by the slab and storey tests.
//!
//! Every box here is a closed, exact, axis-aligned building: horizontal planes and four vertical
//! wall faces, no noise unless the test adds it. The design doc §8 calls this Level 1 — synthetic,
//! in-memory, exact geometry — so a stage's test can assert millimetres instead of trends.
//!
//! The spacing is chosen, not incidental:
//!
//! * faces are gridded every [`FACE_STEP`] = 0.05 m, which is the default raster `cell`, so a face
//!   puts one point in every cell of its own raster and the density cut at `threshold` cannot
//!   mistake the plan for noise;
//! * wall levels are every [`WALL_STEP`] = 0.1 m, which is exactly two `z_step` bins. A bin then
//!   never holds two *different* heights, which is what makes a face's measured mean Z exact: the
//!   wall level that shares the bin of the face at 3.0 m sits at 3.0 m, and no neighbouring level
//!   falls into that bin either. A slab's thickness is therefore the truth, to the last bit.

/// The plan of every box: 6 m by 4 m, so the default `cell` of 0.05 m gives a 120 × 80 grid.
pub(crate) const PLAN_X: f64 = 6.0;
/// The other side of the plan.
pub(crate) const PLAN_Y: f64 = 4.0;
/// Points every 5 cm across a face: 121 × 81 = 9801 points.
pub(crate) const FACE_STEP: f64 = 0.05;
/// Wall levels every 10 cm; see the module doc for why that number is the exact one.
pub(crate) const WALL_STEP: f64 = 0.1;
/// The number of points in one full plane at [`FACE_STEP`].
pub(crate) const FACE_POINTS: usize = 121 * 81;
/// The number of points in one wall level: both x edges (121 each) and both y edges (81 each),
/// corners counted twice on purpose.
pub(crate) const WALL_POINTS_PER_LEVEL: usize = 2 * 121 + 2 * 81;

/// A horizontal plane at `z` spanning columns `ix_from ..= ix_to` of the plan, full depth. The
/// tests that need a *partial* face call this directly; [`plane`] is the whole-column case.
pub(crate) fn plane_between(z: f64, ix_from: usize, ix_to: usize, out: &mut Vec<[f64; 3]>) {
    for ix in ix_from..=ix_to {
        for iy in 0..=80 {
            out.push([ix as f64 * FACE_STEP, iy as f64 * FACE_STEP, z]);
        }
    }
}

/// A full horizontal plane at `z`.
fn plane(z: f64, out: &mut Vec<[f64; 3]>) {
    plane_between(z, 0, 120, out);
}

/// The four wall faces, one level every [`WALL_STEP`] from `z = 0` for `levels` levels.
fn walls(levels: usize, out: &mut Vec<[f64; 3]>) {
    for k in 0..levels {
        let z = k as f64 * WALL_STEP;
        for iy in 0..=80 {
            out.push([0.0, iy as f64 * FACE_STEP, z]);
            out.push([PLAN_X, iy as f64 * FACE_STEP, z]);
        }
        for ix in 0..=120 {
            out.push([ix as f64 * FACE_STEP, 0.0, z]);
            out.push([ix as f64 * FACE_STEP, PLAN_Y, z]);
        }
    }
}

/// Three faces: the floor at 0.0 and one slab whose faces are at 3.0 and 3.3, plus the walls up to
/// the slab. No ceiling — this is Task 2 Step 1's fixture, and it pairs into two slabs.
pub(crate) fn three_face_box() -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::new();
    plane(0.0, &mut out);
    plane(3.0, &mut out);
    plane(3.3, &mut out);
    // Wall levels 0.0 ..= 3.3: the scan stops with the slab.
    walls(34, &mut out);
    out
}

/// Four faces: the floor at 0.0, a slab at 3.0 and 3.3, and a ceiling at 6.3 — the case a real
/// single-storey scan is in, where the ceiling's far side was never seen. Three slabs, two storeys.
pub(crate) fn four_face_box() -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::new();
    plane(0.0, &mut out);
    plane(3.0, &mut out);
    plane(3.3, &mut out);
    plane(6.3, &mut out);
    // Wall levels 0.0 ..= 6.3.
    walls(64, &mut out);
    out
}

/// Three faces like [`three_face_box`] but with no walls, and the slab's *lower* face stops at
/// x = 4 m while its upper face is whole: the furniture case the paper merges both faces for. The
/// lower face alone would give a plan of 16 m², the merged one 24 m².
pub(crate) fn half_lower_face_box() -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::new();
    plane(0.0, &mut out);
    // Blocked beyond x = 4 m (column 80 of 120): 81 × 81 = 6561 points, still above the
    // `slab_ratio` cut of 0.5 × 9801.
    plane_between(3.0, 0, 80, &mut out);
    plane(3.3, &mut out);
    out
}

/// A deterministic stand-in for `rand`, which this crate does not depend on: xorshift64, seeded
/// with a fixed constant, so the noisy fixtures give the same cloud on every run and target.
pub(crate) struct Noise(u64);

impl Noise {
    /// The generator with the seed the noise fixtures use.
    pub(crate) fn new() -> Self {
        // Must be non-zero: xorshift sticks at zero forever.
        Noise(0x2545_F491_4F6C_DD1D)
    }

    /// The next unit fraction in `[0, 1)`: the top 53 bits, which is exactly an f64 mantissa.
    pub(crate) fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0 >> 11) as f64) / (1u64 << 53) as f64
    }
}
