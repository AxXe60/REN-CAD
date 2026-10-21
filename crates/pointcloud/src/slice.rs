//! Horizontal slicing: keep the points between two Z values, optionally thinned to a grid.

use std::collections::HashSet;
use std::path::Path;

use crate::{Bounds3, Error, Result};

/// One point of a slice, in world metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// 8-bit RGB when the file carries colour.
    pub rgb: Option<[u8; 3]>,
    pub intensity: u16,
}

/// A slice and what it cost.
#[derive(Clone, Debug)]
pub struct Slice {
    pub points: Vec<Pt>,
    /// Points the file holds.
    pub total: u64,
    /// Points inside the band, before decimation.
    pub in_band: u64,
    pub bounds: Bounds3,
}

/// Keep the points with `z0 <= z <= z1`, thinning to one point per `decimate` metre cube.
///
/// `decimate` is the drafting answer to a 12-million-point scan: 1–5 cm keeps the wall outline
/// and drops the noise. The cell is a cube, not an XY square, so a wall keeps points up its
/// height. Streaming: the whole file is never held in memory.
pub fn slice(path: &Path, z0: f64, z1: f64, decimate: Option<f64>) -> Result<Slice> {
    if z1 <= z0 {
        return Err(Error::EmptyBand { z0, z1 });
    }
    let cell = match decimate {
        Some(c) if c > 0.0 && c.is_finite() => Some(c),
        Some(_) => return Err(Error::BadDecimation),
        None => None,
    };
    let mut reader = las::Reader::from_path(path).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
    let total = reader.header().number_of_points();
    let mut points = Vec::new();
    let mut bounds = Bounds3::EMPTY;
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::new();
    let mut in_band = 0u64;
    // Batches of a million keep memory flat while streaming a 12-million-point scan.
    loop {
        let batch = reader.read_points(1_000_000).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
        if batch.is_empty() {
            break;
        }
        for p in batch.points() {
            let p = p.map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
            if p.z < z0 || p.z > z1 {
                continue;
            }
            in_band += 1;
            if let Some(c) = cell {
                let key = ((p.x / c).floor() as i64, (p.y / c).floor() as i64, (p.z / c).floor() as i64);
                if !seen.insert(key) {
                    continue;
                }
            }
            let rgb = p.color.map(|c| [to_u8(c.red), to_u8(c.green), to_u8(c.blue)]);
            bounds.add(p.x, p.y, p.z);
            points.push(Pt { x: p.x, y: p.y, z: p.z, rgb, intensity: p.intensity });
        }
    }
    if points.is_empty() {
        return Err(Error::NoPoints { z0, z1 });
    }
    Ok(Slice { points, total, in_band, bounds })
}

/// LAS colours are 16-bit; most producers use the full range, some only 8 bits.
fn to_u8(v: u16) -> u8 {
    if v > 255 { (v / 257) as u8 } else { v as u8 }
}
