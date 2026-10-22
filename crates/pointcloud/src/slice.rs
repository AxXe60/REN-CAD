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
    /// The LAS classification code the file gives the point (2 = ground, 1 = unclassified).
    pub class: u8,
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
    slice_capped(path, z0, z1, decimate, usize::MAX)
}

/// The same, but never keeping more than `max_points` points in memory.
///
/// A registered survey is 50–100 million points (1.8–3.3 GB). Holding them all would cost
/// gigabytes, so the cell grows while the file is read: when the kept points outgrow the cap, the
/// cell is coarsened by half again and the points kept so far are re-binned. The memory stays
/// flat, and the levels built from the result are the ones the whole file would have given.
pub fn slice_capped(path: &Path, z0: f64, z1: f64, decimate: Option<f64>, max_points: usize) -> Result<Slice> {
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
    // Re-binning is the expensive part, so it waits until the kept points are half again over the
    // cap, and the cell then doubles: a survey of 50 million points is read in a handful of passes.
    let limit = max_points.saturating_mul(3) / 2;
    // The reader fills one reused buffer instead of a fresh allocation per batch, so the kept
    // points reserve once, from what the header announces and the cap allows.
    let mut points: Vec<Pt> = Vec::with_capacity(total.min(limit as u64) as usize);
    let mut batch = las::PointDataBuilder::new().for_header(reader.header()).build();
    let mut bounds = Bounds3::EMPTY;
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::new();
    let mut in_band = 0u64;
    // The cell the points are binned on; it grows when the band holds too many.
    let mut cell = cell;
    // Batches of a million keep memory flat while streaming a 12-million-point scan.
    loop {
        let n = reader.fill_points(1_000_000, &mut batch).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
        if n == 0 {
            break;
        }
        // The columns walk the same records in the same order, so zipping them reads each field
        // off its row without decoding the whole record into an owned point first. A short batch
        // is not the end of the file: it is only re-binned and read through like any other.
        {
            let mut colors = batch.rgb();
            let rows = batch.x().zip(batch.y()).zip(batch.z()).zip(batch.intensity()).zip(batch.classification());
            for ((((x, y), z), intensity), class) in rows {
                if z < z0 || z > z1 {
                    continue;
                }
                in_band += 1;
                if let Some(c) = cell {
                    let key = ((x / c).floor() as i64, (y / c).floor() as i64, (z / c).floor() as i64);
                    if !seen.insert(key) {
                        continue;
                    }
                }
                let rgb = colors.as_mut().and_then(Iterator::next).map(|(r, g, b)| [to_u8(r), to_u8(g), to_u8(b)]);
                bounds.add(x, y, z);
                points.push(Pt { x, y, z, rgb, intensity, class: overlap_as_unclassified(class) });
            }
        }
        // Too many points kept: coarser cells, and the points kept so far are re-binned.
        if points.len() > limit {
            let c = cell.map_or(0.005, |c| c * 2.0);
            cell = Some(c);
            let mut kept: Vec<Pt> = Vec::with_capacity(points.len() / 2);
            let mut cells: HashSet<(i64, i64, i64)> = HashSet::with_capacity(points.len() / 2);
            for pt in points.drain(..) {
                let key = ((pt.x / c).floor() as i64, (pt.y / c).floor() as i64, (pt.z / c).floor() as i64);
                if cells.insert(key) {
                    kept.push(pt);
                }
            }
            points = kept;
            seen = cells;
        }
    }
    if points.is_empty() {
        return Err(Error::NoPoints { z0, z1 });
    }
    Ok(Slice { points, total, in_band, bounds })
}

/// The classification byte the owned-point path gives the caller. The crate's row view folds an
/// overlap point (12) into Unclassified (1) on ingest; the column accessor hands out the raw
/// byte, so the same fold happens here to keep the classes the same either way.
fn overlap_as_unclassified(class: u8) -> u8 {
    if class == 12 { crate::classify::CLASS_UNCLASSIFIED } else { class }
}

/// LAS colours are 16-bit; most producers use the full range, some only 8 bits.
fn to_u8(v: u16) -> u8 {
    if v > 255 { (v / 257) as u8 } else { v as u8 }
}
