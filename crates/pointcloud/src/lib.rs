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

mod dxf;
mod read;
mod slice;

pub use dxf::write_points_dxf;
pub use read::{CloudInfo, read_info};
pub use slice::{Pt, Slice, slice};

/// A band of points kept in the session and drawn in the canvas (the `LASOPEN` overlay).
///
/// It owns its points: a 2 cm decimated apartment slice is ~9k points, a few hundred KB, so
/// several clouds can stay open without touching the drawing database.
#[derive(Clone, Debug)]
pub struct Cloud {
    pub name: String,
    pub path: String,
    pub points: Vec<Pt>,
    pub bounds: Bounds3,
    pub visible: bool,
}

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
