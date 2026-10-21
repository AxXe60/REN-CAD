//! Point clouds for CADCraft.
//!
//! Reads LAS/LAZ files, cuts horizontal slices (the drafting habit of keeping a band of Z
//! between, say, 2.04 m and 2.20 m so furniture drops out) and writes the slice as a DXF the
//! drawing database already understands.
//!
//! The crate is deliberately independent of the UI and of the drawing model: it turns a point
//! cloud file into plain points, and plain points into DXF tags. Display and editing live
//! higher up.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod cloud;
mod crs;
mod dxf;
mod ground;
mod measure;
mod read;
mod sketch;
mod slice;
mod snap;

pub use cloud::{ALL_POINTS_LIMIT, Clip, Cloud, CloudColor, CloudLevel, CloudVtx, DEFAULT_MAX_POINTS};
pub use crs::{Crs, epsg_from_wkt};
pub use dxf::{SKETCH_RGB, write_points_dxf, write_strokes_dxf};
pub use ground::{CLASS_GROUND, CLASS_NOISE, CLASS_UNCLASSIFIED, GroundOptions, GroundResult, classify, classify_with};
pub use measure::{Measure, measure, readout};
pub use read::{CloudInfo, read_info};
pub use sketch::{DEFAULT_LAYER, Sketch3d, Stroke3};
pub use slice::{Pt, Slice, slice, slice_capped};
pub use snap::{FRONT_LAYER_M, SMOOTH_RADIUS_M, SnapHit, SnapMode, axis_locked, front_layer, nearest, smooth_centroid, z_locked};

/// An axis-aligned box in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds3 {
    /// An empty box (`min` above `max`), so the first point seeds it.
    pub const EMPTY: Self = Self { min: [f64::INFINITY; 3], max: [f64::NEG_INFINITY; 3] };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1] || self.min[2] > self.max[2]
    }

    pub fn add(&mut self, x: f64, y: f64, z: f64) {
        for (i, v) in [x, y, z].into_iter().enumerate() {
            if v < self.min[i] {
                self.min[i] = v;
            }
            if v > self.max[i] {
                self.max[i] = v;
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: {source}")]
    Las { path: String, source: las::Error },
    #[error("the Z band is empty: z1 ({z1}) must be greater than z0 ({z0})")]
    EmptyBand { z0: f64, z1: f64 },
    #[error("no points in the Z band {z0}..{z1}")]
    NoPoints { z0: f64, z1: f64 },
    #[error("decimation cell must be a positive number of metres")]
    BadDecimation,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Read a whole point cloud file into memory. Kept for tests and small files; `slice` streams.
pub fn read_all(path: &std::path::Path) -> Result<Vec<Pt>> {
    slice(path, f64::NEG_INFINITY, f64::INFINITY, None).map(|s| s.points)
}
