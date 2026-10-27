//! Vertical section cuts through point clouds: profile polylines, clear heights, and DXF export.
//!
//! A section is defined by a 2D line segment between `p1` and `p2` and a cut `thickness`.
//! Points within half the thickness from the vertical section plane are projected onto `(s, z)`
//! coordinates, where `s` is the distance along the section line and `z` is the world elevation.
//!
//! Floor and ceiling profile polylines are extracted by binning points along `s`, taking the
//! 5th and 95th percentiles of elevation in each bin, and simplifying with Douglas-Peucker.
//! Global floor and ceiling elevations and clear height are computed from the medians of
//! the profile elevations.

use std::collections::HashSet;
use std::path::Path;

use cadcraft_dxf::{Tag, Value, write_ascii};

use crate::{Error, Result};

/// One projected point along the vertical section plane.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionPoint {
    /// Station distance from `p1` along the section line (metres).
    pub s: f64,
    /// World elevation (metres).
    pub z: f64,
    /// Original 3D coordinate `[x, y, z]`.
    pub orig: [f64; 3],
    /// LAS intensity value (0..65535).
    pub intensity: u16,
    /// RGB colour channels if present in the source cloud.
    pub rgb: Option<[u16; 3]>,
    /// ASPRS classification code (e.g. 2 = ground, 1 = unclassified).
    pub class: u8,
}

/// A vertical section cut through a point cloud.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionCut {
    /// Start point in world XY.
    pub p1: [f64; 2],
    /// End point in world XY.
    pub p2: [f64; 2],
    /// 2D length of the section line (metres).
    pub length: f64,
    /// Total thickness of the section cut (metres).
    pub thickness: f64,
    /// Projected points within the section volume.
    pub points: Vec<SectionPoint>,
    /// Architectural floor elevation (metres).
    pub floor_z: f64,
    /// Architectural ceiling elevation (metres).
    pub ceiling_z: f64,
    /// Clear height: `ceiling_z - floor_z` (metres).
    pub clear_height: f64,
    /// Polyline of the floor profile as `[s, z]` coordinates.
    pub floor_profile: Vec<[f64; 2]>,
    /// Polyline of the ceiling profile as `[s, z]` coordinates.
    pub ceiling_profile: Vec<[f64; 2]>,
}

/// Bin width along `s` for floor and ceiling profile extraction (metres).
const PROFILE_BIN_WIDTH_M: f64 = 0.05;

/// Douglas-Peucker simplification tolerance for section profile polylines (metres).
const PROFILE_SIMPLIFY_EPSILON_M: f64 = 0.02;

/// Simplify an open 2D polyline using the Ramer-Douglas-Peucker algorithm.
///
/// Keeps the first and last vertices and recursively selects the vertex with the greatest
/// perpendicular distance from the line segment between endpoints until all intermediate
/// vertices are within `epsilon` metres.
pub fn simplify_polyline(pts: &[[f64; 2]], epsilon: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    if n < 3 || !epsilon.is_finite() || epsilon <= 0.0 {
        return pts.to_vec();
    }
    let usable = |p: &[f64; 2]| p[0].is_finite() && p[1].is_finite() && p[0].abs() <= 1e150 && p[1].abs() <= 1e150;
    if !pts.iter().all(usable) {
        return pts.to_vec();
    }

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    let limit = epsilon * epsilon;
    let mut stack: Vec<(usize, usize)> = vec![(0, n - 1)];

    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (pts[a], pts[b]);
        let mut worst = limit;
        let mut at = None;
        for (i, &p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = line_distance_sq(p, pa, pb);
            if d > worst {
                worst = d;
                at = Some(i);
            }
        }
        if let Some(i) = at {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }

    pts.iter().zip(keep.iter()).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

/// Squared perpendicular distance from point `p` to line segment `a`–`b`.
fn line_distance_sq(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 > 1e-12 {
        let cross = (p[0] - a[0]) * dy - (p[1] - a[1]) * dx;
        (cross * cross) / len2
    } else {
        (p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)
    }
}

/// Compute median value of a slice of finite floats.
fn median(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 { sorted[mid] } else { (sorted[mid - 1] + sorted[mid]) / 2.0 }
}

/// Extract floor and ceiling profiles and global clear height from projected section points.
fn generate_profiles_and_clear_height(points: &[SectionPoint], length: f64) -> (f64, f64, f64, Vec<[f64; 2]>, Vec<[f64; 2]>) {
    if points.is_empty() {
        return (0.0, 0.0, 0.0, Vec::new(), Vec::new());
    }

    let bin_w = PROFILE_BIN_WIDTH_M;
    let num_bins = ((length / bin_w).ceil() as usize).max(1);
    let mut bins_z: Vec<Vec<f64>> = vec![Vec::new(); num_bins];
    let mut bins_s: Vec<Vec<f64>> = vec![Vec::new(); num_bins];

    for pt in points {
        let b = ((pt.s / bin_w).floor() as usize).min(num_bins - 1);
        bins_z[b].push(pt.z);
        bins_s[b].push(pt.s);
    }

    let mut raw_floor: Vec<[f64; 2]> = Vec::new();
    let mut raw_ceiling: Vec<[f64; 2]> = Vec::new();
    let mut floor_zs: Vec<f64> = Vec::new();
    let mut ceiling_zs: Vec<f64> = Vec::new();

    for b in 0..num_bins {
        if bins_z[b].is_empty() {
            continue;
        }
        let zs = &mut bins_z[b];
        zs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = zs.len();
        let s_avg = bins_s[b].iter().sum::<f64>() / n as f64;

        // 5th percentile bottom elevation
        let idx_bot = ((n - 1) as f64 * 0.05).round() as usize;
        let z_bot = zs[idx_bot.min(n - 1)];

        // 95th percentile top elevation
        let idx_top = ((n - 1) as f64 * 0.95).round() as usize;
        let z_top = zs[idx_top.min(n - 1)];

        raw_floor.push([s_avg, z_bot]);
        raw_ceiling.push([s_avg, z_top]);
        floor_zs.push(z_bot);
        ceiling_zs.push(z_top);
    }

    let floor_profile = simplify_polyline(&raw_floor, PROFILE_SIMPLIFY_EPSILON_M);
    let ceiling_profile = simplify_polyline(&raw_ceiling, PROFILE_SIMPLIFY_EPSILON_M);

    let floor_z = median(&floor_zs);
    let ceiling_z = median(&ceiling_zs).max(floor_z);
    let clear_height = ceiling_z - floor_z;

    (floor_z, ceiling_z, clear_height, floor_profile, ceiling_profile)
}

/// Cut a vertical section from an in-memory slice of 3D points.
pub fn cut_section_from_points(points: &[[f64; 3]], p1: [f64; 2], p2: [f64; 2], thickness: f64, decimate: Option<f64>) -> Result<SectionCut> {
    let dx = p2[0] - p1[0];
    let dy = p2[1] - p1[1];
    let length = dx.hypot(dy);
    if length < 1e-6 || !length.is_finite() {
        return Err(Error::ZeroLengthSection);
    }
    if thickness <= 0.0 || !thickness.is_finite() {
        return Err(Error::BadSectionThickness);
    }
    let dec_cell = match decimate {
        Some(c) if c > 0.0 && c.is_finite() => Some(c),
        Some(_) => return Err(Error::BadDecimation),
        None => None,
    };

    let ux = dx / length;
    let uy = dy / length;
    let nx = -uy;
    let ny = ux;
    let half_t = thickness / 2.0;

    let mut kept: Vec<SectionPoint> = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();

    for &orig in points {
        let (x, y, z) = (orig[0], orig[1], orig[2]);
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            continue;
        }
        let vx = x - p1[0];
        let vy = y - p1[1];
        let s = vx * ux + vy * uy;
        if s < 0.0 || s > length {
            continue;
        }
        let t = vx * nx + vy * ny;
        if t.abs() > half_t {
            continue;
        }
        if let Some(c) = dec_cell {
            let key = ((s / c).floor() as i64, (z / c).floor() as i64);
            if !seen.insert(key) {
                continue;
            }
        }
        kept.push(SectionPoint {
            s,
            z,
            orig,
            intensity: 0,
            rgb: None,
            class: 1, // CLASS_UNCLASSIFIED
        });
    }

    if kept.is_empty() {
        return Err(Error::NoSectionPoints);
    }

    let (floor_z, ceiling_z, clear_height, floor_profile, ceiling_profile) = generate_profiles_and_clear_height(&kept, length);

    Ok(SectionCut { p1, p2, length, thickness, points: kept, floor_z, ceiling_z, clear_height, floor_profile, ceiling_profile })
}

/// Stream a vertical section cut directly from a LAS/LAZ file.
pub fn cut_section_from_las(path: &Path, p1: [f64; 2], p2: [f64; 2], thickness: f64, decimate: Option<f64>, max_points: usize) -> Result<SectionCut> {
    let dx = p2[0] - p1[0];
    let dy = p2[1] - p1[1];
    let length = dx.hypot(dy);
    if length < 1e-6 || !length.is_finite() {
        return Err(Error::ZeroLengthSection);
    }
    if thickness <= 0.0 || !thickness.is_finite() {
        return Err(Error::BadSectionThickness);
    }
    let mut cell = match decimate {
        Some(c) if c > 0.0 && c.is_finite() => Some(c),
        Some(_) => return Err(Error::BadDecimation),
        None => None,
    };

    let ux = dx / length;
    let uy = dy / length;
    let nx = -uy;
    let ny = ux;
    let half_t = thickness / 2.0;

    let corners = [
        [p1[0] + nx * half_t, p1[1] + ny * half_t],
        [p1[0] - nx * half_t, p1[1] - ny * half_t],
        [p2[0] + nx * half_t, p2[1] + ny * half_t],
        [p2[0] - nx * half_t, p2[1] - ny * half_t],
    ];
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for c in &corners {
        min_x = min_x.min(c[0]);
        max_x = max_x.max(c[0]);
        min_y = min_y.min(c[1]);
        max_y = max_y.max(c[1]);
    }

    let mut reader = las::Reader::from_path(path).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
    let total = reader.header().number_of_points();
    let limit = max_points.saturating_mul(3) / 2;
    let mut points: Vec<SectionPoint> = Vec::with_capacity(total.min(limit as u64) as usize);
    let mut batch = las::PointDataBuilder::new().for_header(reader.header()).build();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();

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
                if x < min_x || x > max_x || y < min_y || y > max_y {
                    continue;
                }
                let vx = x - p1[0];
                let vy = y - p1[1];
                let s = vx * ux + vy * uy;
                if s < 0.0 || s > length {
                    continue;
                }
                let t = vx * nx + vy * ny;
                if t.abs() > half_t {
                    continue;
                }
                if let Some(c) = cell {
                    let key = ((s / c).floor() as i64, (z / c).floor() as i64);
                    if !seen.insert(key) {
                        continue;
                    }
                }
                points.push(SectionPoint { s, z, orig: [x, y, z], intensity, rgb: rgb.map(|(r, g, b)| [r, g, b]), class });
            }
        }
        if points.len() > limit {
            let c = cell.map_or(0.01, |c| c * 2.0);
            cell = Some(c);
            let mut kept: Vec<SectionPoint> = Vec::with_capacity(points.len() / 2);
            let mut cells: HashSet<(i64, i64)> = HashSet::with_capacity(points.len() / 2);
            for pt in points.drain(..) {
                let key = ((pt.s / c).floor() as i64, (pt.z / c).floor() as i64);
                if cells.insert(key) {
                    kept.push(pt);
                }
            }
            points = kept;
            seen = cells;
        }
    }

    if points.is_empty() {
        return Err(Error::NoSectionPoints);
    }

    let (floor_z, ceiling_z, clear_height, floor_profile, ceiling_profile) = generate_profiles_and_clear_height(&points, length);

    Ok(SectionCut { p1, p2, length, thickness, points, floor_z, ceiling_z, clear_height, floor_profile, ceiling_profile })
}

/// Helper to append a string-valued tag.
fn s_tag(tags: &mut Vec<Tag>, code: i32, text: &str) {
    tags.push(Tag::new(code, Value::Str(text.to_string())));
}

/// Helper to append an LWPOLYLINE entity to DXF tags.
fn append_lwpolyline(tags: &mut Vec<Tag>, layer: &str, pts: &[[f64; 2]]) {
    if pts.is_empty() {
        return;
    }
    s_tag(tags, 0, "LWPOLYLINE");
    s_tag(tags, 8, layer);
    s_tag(tags, 100, "AcDbEntity");
    s_tag(tags, 100, "AcDbPolyline");
    tags.push(Tag::new(90, Value::Int(pts.len() as i64)));
    tags.push(Tag::new(70, Value::Int(0))); // Open polyline
    for p in pts {
        tags.push(Tag::new(10, Value::Real(p[0])));
        tags.push(Tag::new(20, Value::Real(p[1])));
    }
}

/// Helper to append a LINE entity to DXF tags.
fn append_line(tags: &mut Vec<Tag>, layer: &str, a: [f64; 2], b: [f64; 2]) {
    s_tag(tags, 0, "LINE");
    s_tag(tags, 8, layer);
    tags.push(Tag::new(10, Value::Real(a[0])));
    tags.push(Tag::new(20, Value::Real(a[1])));
    tags.push(Tag::new(30, Value::Real(0.0)));
    tags.push(Tag::new(11, Value::Real(b[0])));
    tags.push(Tag::new(21, Value::Real(b[1])));
    tags.push(Tag::new(31, Value::Real(0.0)));
}

/// Helper to append a TEXT entity to DXF tags.
fn append_text(tags: &mut Vec<Tag>, layer: &str, p: [f64; 2], height: f64, text: &str) {
    s_tag(tags, 0, "TEXT");
    s_tag(tags, 8, layer);
    tags.push(Tag::new(10, Value::Real(p[0])));
    tags.push(Tag::new(20, Value::Real(p[1])));
    tags.push(Tag::new(30, Value::Real(0.0)));
    tags.push(Tag::new(40, Value::Real(height)));
    s_tag(tags, 1, text);
}

/// Helper to append a POINT entity to DXF tags.
fn append_point(tags: &mut Vec<Tag>, layer: &str, p: [f64; 2], rgb: Option<[u16; 3]>, sixteen_bit: bool) {
    s_tag(tags, 0, "POINT");
    s_tag(tags, 8, layer);
    tags.push(Tag::new(10, Value::Real(p[0])));
    tags.push(Tag::new(20, Value::Real(p[1])));
    tags.push(Tag::new(30, Value::Real(0.0)));
    if let Some([r, g, b]) = rgb {
        let rgb_int = (i64::from(crate::rgb_channel(r, sixteen_bit)) << 16)
            | (i64::from(crate::rgb_channel(g, sixteen_bit)) << 8)
            | i64::from(crate::rgb_channel(b, sixteen_bit));
        tags.push(Tag::new(420, Value::Int(rgb_int)));
    }
}

/// Export a vertical section cut to an AutoCAD R2000 ASCII DXF file.
///
/// Written layers:
/// - `SECTION_POINTS`: POINT entities at `(s, z, 0)`
/// - `SECTION_FLOOR`: LWPOLYLINE of the floor profile
/// - `SECTION_CEILING`: LWPOLYLINE of the ceiling profile
/// - `SECTION_LEVELS`: horizontal lines and text labels for floor (`±0.00 / floor_z`) and ceiling (`+H / ceiling_z`)
/// - `SECTION_DIMS`: vertical dimension line showing clear height H
pub fn export_section_dxf(cut: &SectionCut, path: &Path) -> Result<()> {
    let sixteen_bit = cut.points.iter().filter_map(|p| p.rgb.as_ref()).flatten().any(|c| *c > 255);
    let mut tags: Vec<Tag> = Vec::with_capacity(128 + cut.points.len() * 8);

    // HEADER: R2000, metres ($INSUNITS = 6)
    s_tag(&mut tags, 0, "SECTION");
    s_tag(&mut tags, 2, "HEADER");
    s_tag(&mut tags, 9, "$ACADVER");
    s_tag(&mut tags, 1, "AC1015");
    s_tag(&mut tags, 9, "$INSUNITS");
    tags.push(Tag::new(70, Value::Int(6)));
    s_tag(&mut tags, 0, "ENDSEC");

    // TABLES: Layer definitions
    s_tag(&mut tags, 0, "SECTION");
    s_tag(&mut tags, 2, "TABLES");
    s_tag(&mut tags, 0, "TABLE");
    s_tag(&mut tags, 2, "LAYER");
    let layers = [
        ("0", 7),
        ("SECTION_POINTS", 8),
        ("SECTION_FLOOR", 3),   // green
        ("SECTION_CEILING", 4), // cyan
        ("SECTION_LEVELS", 2),  // yellow
        ("SECTION_DIMS", 1),    // red
    ];
    tags.push(Tag::new(70, Value::Int(layers.len() as i64)));
    for (name, color) in layers {
        s_tag(&mut tags, 0, "LAYER");
        s_tag(&mut tags, 2, name);
        tags.push(Tag::new(70, Value::Int(0)));
        tags.push(Tag::new(62, Value::Int(color)));
        s_tag(&mut tags, 6, "CONTINUOUS");
    }
    s_tag(&mut tags, 0, "ENDTAB");
    s_tag(&mut tags, 0, "ENDSEC");

    // ENTITIES
    s_tag(&mut tags, 0, "SECTION");
    s_tag(&mut tags, 2, "ENTITIES");

    // 1. SECTION_POINTS
    for pt in &cut.points {
        append_point(&mut tags, "SECTION_POINTS", [pt.s, pt.z], pt.rgb, sixteen_bit);
    }

    // 2. SECTION_FLOOR
    append_lwpolyline(&mut tags, "SECTION_FLOOR", &cut.floor_profile);

    // 3. SECTION_CEILING
    append_lwpolyline(&mut tags, "SECTION_CEILING", &cut.ceiling_profile);

    // 4. SECTION_LEVELS
    let x_left = -1.0;
    let x_right = cut.length + 1.0;
    append_line(&mut tags, "SECTION_LEVELS", [x_left, cut.floor_z], [x_right, cut.floor_z]);
    append_line(&mut tags, "SECTION_LEVELS", [x_left, cut.ceiling_z], [x_right, cut.ceiling_z]);
    append_text(&mut tags, "SECTION_LEVELS", [x_left - 0.2, cut.floor_z], 0.2, &format!("±0.00 / {:.2}m", cut.floor_z));
    append_text(&mut tags, "SECTION_LEVELS", [x_left - 0.2, cut.ceiling_z], 0.2, &format!("+{:.2}m / {:.2}m", cut.clear_height, cut.ceiling_z));

    // 5. SECTION_DIMS
    let dim_x = cut.length + 0.5;
    append_line(&mut tags, "SECTION_DIMS", [dim_x, cut.floor_z], [dim_x, cut.ceiling_z]);
    append_line(&mut tags, "SECTION_DIMS", [dim_x - 0.15, cut.floor_z], [dim_x + 0.15, cut.floor_z]);
    append_line(&mut tags, "SECTION_DIMS", [dim_x - 0.15, cut.ceiling_z], [dim_x + 0.15, cut.ceiling_z]);
    append_text(&mut tags, "SECTION_DIMS", [dim_x + 0.25, (cut.floor_z + cut.ceiling_z) / 2.0], 0.2, &format!("H = {:.2}m", cut.clear_height));

    s_tag(&mut tags, 0, "ENDSEC");
    s_tag(&mut tags, 0, "EOF");

    std::fs::write(path, write_ascii(&tags)).map_err(|e| Error::Io { path: path.display().to_string(), source: e })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_synthetic_room(length: f64, width: f64, height: f64) -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        let step = 0.05;
        let mut x = 0.0;
        while x <= length {
            let mut y = -width / 2.0;
            while y <= width / 2.0 {
                // Floor at z = 0.0
                pts.push([x, y, 0.0]);
                // Ceiling at z = height
                pts.push([x, y, height]);
                y += step;
            }
            x += step;
        }
        // Walls along y = ±width/2
        x = 0.0;
        while x <= length {
            let mut z = 0.0;
            while z <= height {
                pts.push([x, -width / 2.0, z]);
                pts.push([x, width / 2.0, z]);
                z += step;
            }
            x += step;
        }
        pts
    }

    #[test]
    fn synthetic_room_section_clear_height() {
        let room = generate_synthetic_room(10.0, 4.0, 2.80);
        let p1 = [0.0, 0.0];
        let p2 = [10.0, 0.0];
        let cut = cut_section_from_points(&room, p1, p2, 0.4, None).unwrap();

        assert!((cut.length - 10.0).abs() < 1e-6);
        assert!((cut.floor_z - 0.0).abs() < 0.05);
        assert!((cut.ceiling_z - 2.80).abs() < 0.05);
        assert!((cut.clear_height - 2.80).abs() < 0.05);
        assert!(!cut.floor_profile.is_empty());
        assert!(!cut.ceiling_profile.is_empty());
        assert!(cut.points.len() > 100);
    }

    #[test]
    fn oblique_section_cut() {
        let room = generate_synthetic_room(10.0, 10.0, 3.20);
        // Diagonal cut from [1.0, 1.0] to [7.0, 9.0]
        let p1 = [1.0, 1.0];
        let p2 = [7.0, 9.0];
        let expected_len = (6.0f64.powi(2) + 8.0f64.powi(2)).sqrt(); // 10.0m
        let cut = cut_section_from_points(&room, p1, p2, 0.5, None).unwrap();

        assert!((cut.length - expected_len).abs() < 1e-6);
        assert!((cut.floor_z - 0.0).abs() < 0.05);
        assert!((cut.ceiling_z - 3.20).abs() < 0.05);
        assert!((cut.clear_height - 3.20).abs() < 0.05);
    }

    #[test]
    fn section_decimation_thins_points() {
        let room = generate_synthetic_room(5.0, 2.0, 2.50);
        let p1 = [0.0, 0.0];
        let p2 = [5.0, 0.0];
        let raw_cut = cut_section_from_points(&room, p1, p2, 0.3, None).unwrap();
        let dec_cut = cut_section_from_points(&room, p1, p2, 0.3, Some(0.10)).unwrap();

        assert!(dec_cut.points.len() < raw_cut.points.len());
        assert!((dec_cut.clear_height - 2.50).abs() < 0.05);
    }

    #[test]
    fn error_handling_invalid_inputs() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        // Zero length
        let err1 = cut_section_from_points(&pts, [1.0, 1.0], [1.0, 1.0], 0.2, None);
        assert!(matches!(err1, Err(Error::ZeroLengthSection)));

        // Bad thickness
        let err2 = cut_section_from_points(&pts, [0.0, 0.0], [1.0, 0.0], -0.1, None);
        assert!(matches!(err2, Err(Error::BadSectionThickness)));

        // Bad decimation
        let err3 = cut_section_from_points(&pts, [0.0, 0.0], [1.0, 0.0], 0.2, Some(-0.5));
        assert!(matches!(err3, Err(Error::BadDecimation)));

        // No points in range
        let err4 = cut_section_from_points(&pts, [100.0, 100.0], [110.0, 100.0], 0.2, None);
        assert!(matches!(err4, Err(Error::NoSectionPoints)));
    }

    #[test]
    fn polyline_simplification() {
        let noisy = vec![
            [0.0, 1.0],
            [1.0, 1.005],
            [2.0, 0.998],
            [3.0, 1.002],
            [4.0, 2.500], // spike
            [5.0, 1.001],
            [6.0, 1.0],
        ];
        let simplified = simplify_polyline(&noisy, 0.02);
        assert!(simplified.len() < noisy.len());
        // Spike at x=4.0 must be kept
        assert!(simplified.iter().any(|p| (p[0] - 4.0).abs() < 1e-6));
    }

    #[test]
    fn dxf_export_generates_valid_content() {
        let room = generate_synthetic_room(4.0, 2.0, 2.70);
        let cut = cut_section_from_points(&room, [0.0, 0.0], [4.0, 0.0], 0.3, None).unwrap();

        let dir = std::env::temp_dir().join("cadcraft-section-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test_section.dxf");

        export_section_dxf(&cut, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("SECTION_POINTS"));
        assert!(content.contains("SECTION_FLOOR"));
        assert!(content.contains("SECTION_CEILING"));
        assert!(content.contains("SECTION_LEVELS"));
        assert!(content.contains("SECTION_DIMS"));
        assert!(content.contains("LWPOLYLINE"));
        assert!(content.contains("H = 2.70m"));

        std::fs::remove_file(&path).ok();
    }
}
