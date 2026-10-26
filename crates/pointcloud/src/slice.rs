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
    /// Raw LAS RGB channels when the file carries colour.
    pub rgb: Option<[u16; 3]>,
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
            // The colour column walks the file's rows like every other column, so it is zipped in
            // rather than pulled when a point is kept: pulling on keep advances it once per kept
            // point while the rows advance once per record, and every later point then carries the
            // colour of an earlier one — the clutter's grey painted onto the floor.
            let colors: Box<dyn Iterator<Item = Option<(u16, u16, u16)>> + '_> = match batch.rgb() {
                Some(iter) => Box::new(iter.map(Some)),
                None => Box::new(std::iter::repeat(None)),
            };
            let rows = batch.x().zip(batch.y()).zip(batch.z()).zip(batch.intensity()).zip(batch.classification()).zip(colors);
            for (((((x, y), z), intensity), class), rgb) in rows {
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
                bounds.add(x, y, z);
                points.push(Pt { x, y, z, rgb: rgb.map(|(r, g, b)| [r, g, b]), intensity, class: overlap_as_unclassified(class) });
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

/// The most wall bands one streaming read will carry; the plan's wall budget is far above it.
pub const MAX_WALL_BANDS: usize = 256;

/// A world box a fine band is read from: XY bounds and a Z range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub z0: f64,
    pub z1: f64,
}

/// One streaming read for many wall bands, each thinned to its own cube and capped on its own.
///
/// The openings stage needs every wall of a storey at ~5 mm, a resolution the in-memory working
/// level does not carry. Reading the file once per wall would be dozens of passes over a 41
/// million point scan, so the bands are filled together in one read, each deduplicated and capped
/// on its own cell. A band whose box is empty or not a number comes back empty rather than as an
/// error: the caller's wall geometry has already been validated.
pub fn slice_bands_capped(path: &Path, bands: &[Band], decimate: f64, max_points_per_band: usize) -> Result<Vec<Vec<Pt>>> {
    if !decimate.is_finite() || decimate <= 0.0 {
        return Err(Error::BadDecimation);
    }
    if bands.len() > MAX_WALL_BANDS {
        return Err(Error::TooManyBands { limit: MAX_WALL_BANDS });
    }
    struct Filled {
        points: Vec<Pt>,
        seen: HashSet<(i64, i64, i64)>,
        cell: f64,
    }
    // A band whose box cannot be read takes no points; it is not a reason to refuse the file.
    let usable = |b: &Band| {
        [b.min[0], b.min[1], b.max[0], b.max[1], b.z0, b.z1].iter().all(|v| v.is_finite())
            && b.max[0] >= b.min[0]
            && b.max[1] >= b.min[1]
            && b.z1 > b.z0
    };
    let mut filled: Vec<Option<Filled>> = bands
        .iter()
        .map(|b| usable(b).then(|| Filled { points: Vec::with_capacity(max_points_per_band.min(1_000_000)), seen: HashSet::new(), cell: decimate }))
        .collect();
    let mut reader = las::Reader::from_path(path).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
    let mut batch = las::PointDataBuilder::new().for_header(reader.header()).build();
    // The same growth rule `slice_capped` uses, applied per band: a band that outgrows half again
    // the cap doubles its own cell and re-bins what it kept, so the read stays flat in memory.
    let limit = max_points_per_band.saturating_mul(3) / 2;
    loop {
        let n = reader.fill_points(1_000_000, &mut batch).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
        if n == 0 {
            break;
        }
        {
            let colors: Box<dyn Iterator<Item = Option<(u16, u16, u16)>> + '_> = match batch.rgb() {
                Some(iter) => Box::new(iter.map(Some)),
                None => Box::new(std::iter::repeat(None)),
            };
            let rows = batch.x().zip(batch.y()).zip(batch.z()).zip(batch.intensity()).zip(batch.classification()).zip(colors);
            for (((((x, y), z), intensity), class), rgb) in rows {
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    continue;
                }
                for (band, slot) in bands.iter().zip(filled.iter_mut()) {
                    let Some(f) = slot.as_mut() else { continue };
                    if x < band.min[0] || x > band.max[0] || y < band.min[1] || y > band.max[1] || z < band.z0 || z > band.z1 {
                        continue;
                    }
                    let key = ((x / f.cell).floor() as i64, (y / f.cell).floor() as i64, (z / f.cell).floor() as i64);
                    if !f.seen.insert(key) {
                        continue;
                    }
                    f.points.push(Pt { x, y, z, rgb: rgb.map(|(r, g, b)| [r, g, b]), intensity, class: overlap_as_unclassified(class) });
                }
            }
        }
        for slot in &mut filled {
            let Some(f) = slot.as_mut() else { continue };
            // Coarsen until the band is back under the cap: one doubling can leave it above the
            // limit when the cap is small, and a band is handed to the detector as it stands. A
            // doubling that keeps no point fewer means the points are already one per cell and
            // larger cells cannot help, so the loop stops rather than spin.
            while f.points.len() > limit {
                let before = f.points.len();
                f.cell *= 2.0;
                let mut kept: Vec<Pt> = Vec::with_capacity(f.points.len() / 2);
                let mut cells: HashSet<(i64, i64, i64)> = HashSet::with_capacity(f.points.len() / 2);
                for pt in f.points.drain(..) {
                    let key = ((pt.x / f.cell).floor() as i64, (pt.y / f.cell).floor() as i64, (pt.z / f.cell).floor() as i64);
                    if cells.insert(key) {
                        kept.push(pt);
                    }
                }
                f.points = kept;
                f.seen = cells;
                if f.points.len() >= before {
                    break;
                }
            }
        }
    }
    Ok(filled.into_iter().map(|slot| slot.map(|f| f.points).unwrap_or_default()).collect())
}

/// Normalize every RGB channel with one decision for the whole point set.
pub fn normalize_rgb(points: &mut [Pt]) {
    let sixteen_bit = points.iter().filter_map(|point| point.rgb.as_ref()).flatten().any(|channel| *channel > 255);
    for point in points {
        if let Some(rgb) = point.rgb.as_mut() {
            for channel in rgb {
                *channel = u16::from(rgb_channel(*channel, sixteen_bit));
            }
        }
    }
}

/// Convert one channel using the selected 8-bit or 16-bit decision.
pub fn rgb_channel(v: u16, sixteen_bit: bool) -> u8 {
    if sixteen_bit { (v / 257) as u8 } else { v as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A LAS 1.2 file whose points carry colour: point format 2, the 20 bytes of format 0 plus the
    /// RGB triplet. The points are (x, y, z, rgb).
    fn write_rgb_las(path: &Path, points: &[(f64, f64, f64, [u16; 3])]) {
        fn padded(text: &[u8]) -> [u8; 32] {
            let mut out = [0u8; 32];
            let n = text.len().min(out.len());
            out[..n].copy_from_slice(&text[..n]);
            out
        }
        let scale = 0.001f64;
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for (x, y, z, _) in points {
            for (k, v) in [x, y, z].iter().enumerate() {
                min[k] = min[k].min(**v);
                max[k] = max[k].max(**v);
            }
        }
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"LASF");
        b.extend_from_slice(&0u16.to_le_bytes()); // file source id
        b.extend_from_slice(&0u16.to_le_bytes()); // global encoding
        b.extend_from_slice(&[0u8; 16]); // project id
        b.push(1); // version 1.2
        b.push(2);
        b.extend_from_slice(&padded(b"CADCraft")); // system identifier
        b.extend_from_slice(&padded(b"CADCraft test")); // generating software
        b.extend_from_slice(&1u16.to_le_bytes()); // creation day
        b.extend_from_slice(&2026u16.to_le_bytes()); // creation year
        b.extend_from_slice(&227u16.to_le_bytes()); // header size
        b.extend_from_slice(&227u32.to_le_bytes()); // the points start right after the header
        b.extend_from_slice(&0u32.to_le_bytes()); // VLRs
        b.push(2); // point format 2: format 0 plus the colour
        b.extend_from_slice(&26u16.to_le_bytes()); // record length
        b.extend_from_slice(&(points.len() as u32).to_le_bytes());
        b.extend_from_slice(&(points.len() as u32).to_le_bytes()); // points of the first return
        b.extend_from_slice(&[0u8; 16]); // the other four returns are empty
        for _ in 0..3 {
            b.extend_from_slice(&scale.to_le_bytes()); // scale
        }
        for _ in 0..3 {
            b.extend_from_slice(&0f64.to_le_bytes()); // offset
        }
        for v in [max[0], min[0], max[1], min[1], max[2], min[2]] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for &(x, y, z, rgb) in points {
            b.extend_from_slice(&((x / scale).round() as i32).to_le_bytes());
            b.extend_from_slice(&((y / scale).round() as i32).to_le_bytes());
            b.extend_from_slice(&((z / scale).round() as i32).to_le_bytes());
            b.extend_from_slice(&100u16.to_le_bytes()); // intensity
            b.push(0x11); // one return, the first
            b.push(2); // ground
            b.push(0); // scan angle
            b.push(0); // user data
            b.extend_from_slice(&0u16.to_le_bytes()); // point source id
            for channel in rgb {
                b.extend_from_slice(&channel.to_le_bytes());
            }
        }
        std::fs::write(path, b).ok();
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        dir.join(name)
    }

    /// The band cut drops records. The colour column walks the file's rows, so a kept point must
    /// keep the colour of its own row — a colour taken from a dropped row paints the wall of one
    /// storey with the colour of another.
    #[test]
    fn a_kept_point_carries_its_own_colour_when_the_band_drops_records() {
        let path = temp("band-rgb.las");
        let mut points: Vec<(f64, f64, f64, [u16; 3])> = Vec::new();
        for i in 0..8u16 {
            points.push((f64::from(i), 0.0, 1.0, [i * 3 + 1, 7, 11])); // the floor, in band
            points.push((f64::from(i), 0.0, 3.0, [99, 99, 99])); // clutter, out of band
        }
        write_rgb_las(&path, &points);
        let slice = slice(&path, 0.5, 1.5, None).unwrap();
        let colors: Vec<[u16; 3]> = slice.points.iter().map(|p| p.rgb.unwrap_or([0, 0, 0])).collect();
        assert_eq!(colors.len(), 8, "the band keeps the floor alone");
        assert_eq!(colors, (0..8u16).map(|i| [i * 3 + 1, 7, 11]).collect::<Vec<[u16; 3]>>(), "each kept point carries its own colour");
    }

    /// The same through the grid: a decimated cell keeps one of the points it holds, and the
    /// points it drops still cost a row of colour.
    #[test]
    fn a_kept_point_carries_its_own_colour_when_the_grid_drops_records() {
        let path = temp("grid-rgb.las");
        let points: Vec<(f64, f64, f64, [u16; 3])> = (0..4u16).map(|i| (f64::from(i) * 0.25, 0.0, 1.0, [i * 10 + 1, i, i])).collect();
        write_rgb_las(&path, &points);
        let slice = slice(&path, 0.5, 1.5, Some(0.5)).unwrap();
        let colors: Vec<[u16; 3]> = slice.points.iter().map(|p| p.rgb.unwrap_or([0, 0, 0])).collect();
        assert_eq!(colors, vec![[1, 0, 0], [21, 2, 2]], "the two points the grid keeps carry their own colours");
    }

    /// One read fills every wall band, each deduplicated on its own cell and capped on its own;
    /// a band that outgrows the cap coarsens instead of holding everything.
    #[test]
    fn a_band_read_keeps_only_its_own_points_and_the_cap_coarsens_it() {
        let path = temp("bands.las");
        let mut pts = Vec::new();
        for i in 0..4000 {
            let x = f64::from(i) * 0.005; // 20 m of points at 5 mm
            pts.push((x, 0.0, 1.0, [1u16, 2, 3]));
            pts.push((x, 10.0, 1.0, [4u16, 5, 6]));
        }
        write_rgb_las(&path, &pts);
        let bands = [Band { min: [-0.1, -0.2], max: [20.1, 0.2], z0: 0.0, z1: 2.0 }, Band { min: [-0.1, 9.8], max: [20.1, 10.2], z0: 0.0, z1: 2.0 }];
        let got = slice_bands_capped(&path, &bands, 0.005, 1_000).expect("the file reads");
        assert_eq!(got.len(), 2);
        assert!(got[0].iter().all(|p| p.y.abs() < 0.2) && got[1].iter().all(|p| (p.y - 10.0).abs() < 0.2));
        assert!(got[0].len() <= 1_500, "the cap coarsens instead of holding every point: {}", got[0].len());
        assert!(got[0].len() >= 500, "and the wall is still there: {}", got[0].len());
        assert!(slice_bands_capped(&path, &bands, 0.0, 1_000).is_err(), "a zero cell is refused");
        let too_many = vec![bands[0]; 257];
        assert!(matches!(slice_bands_capped(&path, &too_many, 0.005, 1_000), Err(Error::TooManyBands { .. })));
    }
}
