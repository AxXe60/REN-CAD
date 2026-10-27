//! Structural column detection from point clouds.
//!
//! Columns are standalone vertical structural elements (IfcColumn) with either rectangular or
//! circular cross-sections. They stand within the storey volume between floor and ceiling slabs.
//!
//! The pipeline detects columns by:
//! 1. Excluding points inside wall corridors (points closer than `wall.thickness * 0.5 + 0.10` m
//!    to any wall axis).
//! 2. Taking horizontal slice(s) across the clear height of the storey to find compact closed candidate
//!    contours.
//! 3. Filtering contours by metric bounding dimensions (`opts.min_column_size`..=`opts.max_column_size`)
//!    and aspect ratio (`<= opts.max_column_aspect_ratio`).
//! 4. Verifying 3-tier vertical continuity across lower [0.10, 0.35], middle [0.40, 0.65], and upper
//!    [0.70, 0.95] tiers of clear height, spanning >= 80% of storey height.
//! 5. Classifying shape as Circular (diameter) or Rectangular (width, depth, rotation) using
//!    2D PCA oriented bounding boxes, radial distance RMSE, edge distance RMSE, and convex hull area.

use std::f64::consts::FRAC_PI_2;

use crate::bim::raster::{self, Raster};
use crate::bim::storeys::{self, Storey};
use crate::bim::walls::Wall;
use crate::bim::{BimError, BimOptions};

/// The geometric shape of a structural column.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnShape {
    /// Rectangular column with principal cross-section dimensions and orientation angle.
    Rectangular {
        /// Major cross-section dimension in metres.
        width: f64,
        /// Minor cross-section dimension in metres.
        depth: f64,
        /// Orientation angle in degrees from world X axis (-90.0..90.0).
        rotation_deg: f64,
    },
    /// Circular cylindrical column.
    Circular {
        /// Outer diameter in metres.
        diameter: f64,
    },
}

/// A structural column standing in a storey.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// 2D centroid / center of the column cross-section in world metres.
    pub center: [f64; 2],
    /// Bottom elevation of the column (matches the storey base).
    pub base_z: f64,
    /// Vertical height of the column in metres.
    pub height: f64,
    /// Cross-section profile shape.
    pub shape: ColumnShape,
}

/// Detect structural columns standing in `storey`.
///
/// # Errors
///
/// * [`BimError::InvalidCell`] for invalid parameters in `opts`.
/// * [`BimError::TooLarge`] when raster dimensions exceed the memory budget.
pub fn detect_columns(points: &[[f64; 3]], storey: &Storey, walls: &[Wall], opts: &BimOptions) -> Result<Vec<Column>, BimError> {
    if !opts.cell.is_finite() || opts.cell <= 0.0 {
        return Err(BimError::InvalidCell);
    }
    let height = storey.top_z - storey.base_z;
    if !height.is_finite() || height <= 0.0 {
        return Ok(Vec::new());
    }

    let min_col_size = if opts.min_column_size.is_finite() && opts.min_column_size > 0.0 { opts.min_column_size } else { 0.15 };
    let max_col_size = if opts.max_column_size.is_finite() && opts.max_column_size >= min_col_size { opts.max_column_size } else { 1.00 };
    let max_aspect =
        if opts.max_column_aspect_ratio.is_finite() && opts.max_column_aspect_ratio >= 1.0 { opts.max_column_aspect_ratio } else { 2.50 };

    // Step 1: Filter points in the storey and exclude points within wall corridors.
    let storey_pts = storeys::points_in(points, storey, opts);
    let non_wall_points: Vec<[f64; 3]> = storey_pts
        .into_iter()
        .filter(|p| {
            if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
                return false;
            }
            !is_in_any_wall_corridor(p[0], p[1], walls)
        })
        .collect();

    if non_wall_points.is_empty() {
        return Ok(Vec::new());
    }

    // Step 2: Extract candidate horizontal slice across the middle tier of clear height.
    // Taking the middle window [0.30, 0.70] avoids floor clutter and ceiling equipment.
    let slice_lo = storey.base_z + 0.30 * height;
    let slice_hi = storey.base_z + 0.70 * height;
    let slice_pts: Vec<[f64; 3]> = non_wall_points.iter().filter(|p| p[2] >= slice_lo && p[2] <= slice_hi).copied().collect();

    if slice_pts.is_empty() {
        return Ok(Vec::new());
    }

    // Determine bounding rectangle for the raster grid.
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for p in &slice_pts {
        low[0] = low[0].min(p[0]);
        low[1] = low[1].min(p[1]);
        high[0] = high[0].max(p[0]);
        high[1] = high[1].max(p[1]);
    }

    let cell = opts.cell.max(0.02);
    let pad = 2.0 * cell;
    let origin = [(low[0] / cell).floor() * cell - pad, (low[1] / cell).floor() * cell - pad];
    let far = [high[0] + pad, high[1] + pad];

    let Some(mut raster) = Raster::new(origin, far, cell) else {
        return Err(BimError::TooLarge { what: "column raster", limit: raster::MAX_RASTER_CELLS });
    };

    for p in &slice_pts {
        raster.add(*p);
    }
    if raster.total() == 0 {
        return Ok(Vec::new());
    }

    let (cols, rows) = raster.dims();
    let threshold = if opts.threshold.is_finite() && opts.threshold > 0.0 { opts.threshold } else { 0.01 };
    let set = raster::mask(&raster, threshold);
    let close_rad = opts.close_radius.clamp(1, 2);
    let closed = raster::close(&set, cols, rows, close_rad);
    let (rings, _) = raster::trace(&closed, cols, rows);

    let mut candidate_columns: Vec<Column> = Vec::new();

    for contour in &rings {
        if !contour.outer || contour.ring.len() < 3 {
            continue;
        }

        let world_ring = raster.to_world(&contour.ring);
        let mut r_min = [f64::INFINITY; 2];
        let mut r_max = [f64::NEG_INFINITY; 2];
        for pt in &world_ring {
            r_min[0] = r_min[0].min(pt[0]);
            r_min[1] = r_min[1].min(pt[1]);
            r_max[0] = r_max[0].max(pt[0]);
            r_max[1] = r_max[1].max(pt[1]);
        }

        let aabb_w = r_max[0] - r_min[0];
        let aabb_d = r_max[1] - r_min[1];
        if aabb_w.max(aabb_d) > max_col_size * 1.5 || aabb_w.min(aabb_d) < min_col_size * 0.5 {
            continue;
        }

        // Collect all non-wall points falling within this candidate's expanded 2D footprint.
        let margin = cell * 1.5;
        let c_pts: Vec<[f64; 3]> = non_wall_points
            .iter()
            .filter(|p| p[0] >= r_min[0] - margin && p[0] <= r_max[0] + margin && p[1] >= r_min[1] - margin && p[1] <= r_max[1] + margin)
            .copied()
            .collect();

        if c_pts.len() < 6 {
            continue;
        }

        // Step 4: Vertical Continuity Filter.
        // Candidate points must exist in all 3 vertical tiers:
        // lower [0.10, 0.35], middle [0.40, 0.65], upper [0.70, 0.95] of clear height.
        let tier_lower_lo = storey.base_z + 0.10 * height;
        let tier_lower_hi = storey.base_z + 0.35 * height;
        let tier_mid_lo = storey.base_z + 0.40 * height;
        let tier_mid_hi = storey.base_z + 0.65 * height;
        let tier_upper_lo = storey.base_z + 0.70 * height;
        let tier_upper_hi = storey.base_z + 0.95 * height;

        let count_lower = c_pts.iter().filter(|p| p[2] >= tier_lower_lo && p[2] <= tier_lower_hi).count();
        let count_mid = c_pts.iter().filter(|p| p[2] >= tier_mid_lo && p[2] <= tier_mid_hi).count();
        let count_upper = c_pts.iter().filter(|p| p[2] >= tier_upper_lo && p[2] <= tier_upper_hi).count();
        if count_lower < 6 || count_mid < 6 || count_upper < 6 || (count_lower + count_mid + count_upper) < 25 {
            continue;
        }

        // Total vertical span must be at least 80% of storey clear height.
        let min_z = c_pts.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let max_z = c_pts.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        if (max_z - min_z) < 0.80 * height {
            continue;
        }

        // Step 3 & 5: Oriented Bounding Box (PCA) and Shape Classification.
        let pts_2d: Vec<[f64; 2]> = c_pts.iter().map(|p| [p[0], p[1]]).collect();
        let Some(obb) = compute_obb_pca(&pts_2d) else {
            continue;
        };

        // Check that column center obb.center satisfies point-to-segment clearance from ALL wall lines.
        let col_radius = obb.width.max(obb.depth) * 0.5;
        let mut too_close_to_wall = false;
        for w in walls {
            let min_clearance = 0.35_f64.max(w.thickness * 0.5 + col_radius + 0.25);
            let d = dist_point_to_segment_2d(obb.center[0], obb.center[1], w.from[0], w.from[1], w.to[0], w.to[1]);
            if d < min_clearance {
                too_close_to_wall = true;
                break;
            }
        }
        if too_close_to_wall {
            continue;
        }

        let dim_min = obb.width.min(obb.depth);
        let dim_max = obb.width.max(obb.depth);
        if dim_min < min_col_size || dim_max > max_col_size {
            continue;
        }
        if dim_min <= 0.0 || (dim_max / dim_min) > max_aspect {
            continue;
        }

        let shape = classify_shape(&pts_2d, &obb);

        candidate_columns.push(Column { center: obb.center, base_z: storey.base_z, height, shape });
    }

    // Deduplicate candidate columns by center distance.
    let mut columns: Vec<Column> = Vec::new();
    for col in candidate_columns {
        let is_dup = columns.iter().any(|c| {
            let dx = c.center[0] - col.center[0];
            let dy = c.center[1] - col.center[1];
            (dx * dx + dy * dy).sqrt() < min_col_size * 0.75
        });
        if !is_dup {
            columns.push(col);
        }
    }

    columns.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]).then(a.center[1].total_cmp(&b.center[1])));

    Ok(columns)
}

/// Whether 2D point (px, py) lies within the corridor of any wall.
///
/// A wall corridor extends `w.thickness * 0.5 + 0.10` m perpendicularly from its centerline axis.
pub fn is_in_any_wall_corridor(px: f64, py: f64, walls: &[Wall]) -> bool {
    for w in walls {
        let corridor = w.thickness * 0.5 + 0.10;
        let d = dist_point_to_segment_2d(px, py, w.from[0], w.from[1], w.to[0], w.to[1]);
        if d <= corridor {
            return true;
        }
    }
    false
}

/// Distance from a point (px, py) to a 2D line segment (ax, ay) -> (bx, by).
pub fn dist_point_to_segment_2d(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= 1e-12 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = ax + t * dx;
    let proj_y = ay + t * dy;
    (px - proj_x).hypot(py - proj_y)
}

/// 2D Oriented Bounding Box computed via Principal Component Analysis (PCA).
#[derive(Debug, Clone, PartialEq)]
pub struct Obb2d {
    /// Center of the box in world coordinates.
    pub center: [f64; 2],
    /// Dimension along major axis in metres.
    pub width: f64,
    /// Dimension along minor axis in metres.
    pub depth: f64,
    /// Rotation angle in degrees (-90.0..90.0).
    pub rotation_deg: f64,
    /// Unit vector along major axis.
    pub u: [f64; 2],
    /// Unit vector along minor axis.
    pub v: [f64; 2],
}

/// Compute oriented bounding box of 2D points using Principal Component Analysis.
pub fn compute_obb_pca(pts: &[[f64; 2]]) -> Option<Obb2d> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let cx = pts.iter().map(|p| p[0]).sum::<f64>() / nf;
    let cy = pts.iter().map(|p| p[1]).sum::<f64>() / nf;

    let mut cxx = 0.0;
    let mut cyy = 0.0;
    let mut cxy = 0.0;
    for p in pts {
        let dx = p[0] - cx;
        let dy = p[1] - cy;
        cxx += dx * dx;
        cyy += dy * dy;
        cxy += dx * dy;
    }
    cxx /= nf;
    cyy /= nf;
    cxy /= nf;

    // Principal angle from 2x2 covariance matrix:
    let theta = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    let u0 = [theta.cos(), theta.sin()];
    let v0 = [-theta.sin(), theta.cos()];

    let mut u_min = f64::INFINITY;
    let mut u_max = f64::NEG_INFINITY;
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;

    for p in pts {
        let dx = p[0] - cx;
        let dy = p[1] - cy;
        let pu = dx * u0[0] + dy * u0[1];
        let pv = dx * v0[0] + dy * v0[1];
        u_min = u_min.min(pu);
        u_max = u_max.max(pu);
        v_min = v_min.min(pv);
        v_max = v_max.max(pv);
    }

    let dim_u = (u_max - u_min).max(0.0);
    let dim_v = (v_max - v_min).max(0.0);

    let (width, depth, u, v, rot_rad) =
        if dim_u >= dim_v { (dim_u, dim_v, u0, v0, theta) } else { (dim_v, dim_u, v0, [-u0[0], -u0[1]], theta + FRAC_PI_2) };

    let cu = (u_min + u_max) / 2.0;
    let cv = (v_min + v_max) / 2.0;
    let center = [cx + cu * u0[0] + cv * v0[0], cy + cu * u0[1] + cv * v0[1]];

    // Normalize rotation angle to [-90.0, 90.0)
    let mut deg = rot_rad.to_degrees() % 180.0;
    if deg > 90.0 {
        deg -= 180.0;
    } else if deg <= -90.0 {
        deg += 180.0;
    }

    Some(Obb2d { center, width, depth, rotation_deg: deg, u, v })
}

/// Classify whether candidate points represent a Circular or Rectangular column shape.
pub fn classify_shape(pts: &[[f64; 2]], obb: &Obb2d) -> ColumnShape {
    let nf = pts.len() as f64;
    if nf < 3.0 {
        return ColumnShape::Rectangular { width: obb.width, depth: obb.depth, rotation_deg: obb.rotation_deg };
    }

    // 1. Radial distance RMSE relative to centroid.
    let mut sum_r = 0.0;
    let mut rs = Vec::with_capacity(pts.len());
    for p in pts {
        let r = (p[0] - obb.center[0]).hypot(p[1] - obb.center[1]);
        rs.push(r);
        sum_r += r;
    }
    let r_mean = sum_r / nf;
    let mut sum_r_sq_diff = 0.0;
    for &r in &rs {
        sum_r_sq_diff += (r - r_mean).powi(2);
    }
    let radial_rmse = (sum_r_sq_diff / nf).sqrt();

    // 2. Distance RMSE to OBB edges.
    let hw = obb.width / 2.0;
    let hd = obb.depth / 2.0;
    let mut sum_edge_sq = 0.0;
    for p in pts {
        let dx = p[0] - obb.center[0];
        let dy = p[1] - obb.center[1];
        let pu = (dx * obb.u[0] + dy * obb.u[1]).abs();
        let pv = (dx * obb.v[0] + dy * obb.v[1]).abs();
        let dist_u = (pu - hw).abs();
        let dist_v = (pv - hd).abs();
        let dist = dist_u.min(dist_v);
        sum_edge_sq += dist * dist;
    }
    let obb_rmse = (sum_edge_sq / nf).sqrt();

    // 3. Convex hull area vs OBB area.
    let hull = convex_hull_2d(pts);
    let hull_area = polygon_area_2d(&hull);
    let obb_area = obb.width * obb.depth;

    let aspect = if obb.depth > 0.0 { obb.width / obb.depth } else { 1.0 };

    let is_circular = aspect <= 1.20 && (radial_rmse < obb_rmse || (obb_area > 0.0 && (hull_area / obb_area) < 0.85));

    if is_circular {
        let diameter = (2.0 * r_mean).max((obb.width + obb.depth) / 2.0);
        ColumnShape::Circular { diameter }
    } else {
        ColumnShape::Rectangular { width: obb.width, depth: obb.depth, rotation_deg: obb.rotation_deg }
    }
}

/// 2D Convex Hull using Andrew's monotone chain algorithm.
pub fn convex_hull_2d(pts: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut sorted = pts.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    sorted.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if sorted.len() <= 2 {
        return sorted;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| -> f64 { (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]) };
    let mut lower = Vec::new();
    for &p in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper = Vec::new();
    for &p in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Area of a 2D polygon using the Shoelace formula.
pub fn polygon_area_2d(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        sum += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    (sum / 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangular_obb_extraction() {
        // Create a 0.30 x 0.50 m rectangular column rotated 30 degrees around (2.0, 3.0)
        let cx = 2.0;
        let cy = 3.0;
        let angle_rad = 30.0_f64.to_radians();
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let hw = 0.25; // width = 0.50
        let hd = 0.15; // depth = 0.30

        let mut pts = Vec::new();
        // Generate points along the 4 edges
        for i in 0..=20 {
            let t = -hw + 2.0 * hw * (i as f64 / 20.0);
            for &side in &[-hd, hd] {
                let lx = t;
                let ly = side;
                let gx = cx + lx * cos_a - ly * sin_a;
                let gy = cy + lx * sin_a + ly * cos_a;
                pts.push([gx, gy]);
            }
        }
        for j in 0..=12 {
            let t = -hd + 2.0 * hd * (j as f64 / 12.0);
            for &side in &[-hw, hw] {
                let lx = side;
                let ly = t;
                let gx = cx + lx * cos_a - ly * sin_a;
                let gy = cy + lx * sin_a + ly * cos_a;
                pts.push([gx, gy]);
            }
        }

        let obb = compute_obb_pca(&pts).expect("OBB should compute successfully");
        assert!((obb.center[0] - cx).abs() < 0.02, "center X: {}", obb.center[0]);
        assert!((obb.center[1] - cy).abs() < 0.02, "center Y: {}", obb.center[1]);
        assert!((obb.width - 0.50).abs() < 0.03, "width: {}", obb.width);
        assert!((obb.depth - 0.30).abs() < 0.03, "depth: {}", obb.depth);
        assert!((obb.rotation_deg - 30.0).abs() < 3.0, "rotation: {}", obb.rotation_deg);

        let shape = classify_shape(&pts, &obb);
        match shape {
            ColumnShape::Rectangular { width, depth, rotation_deg } => {
                assert!((width - 0.50).abs() < 0.03);
                assert!((depth - 0.30).abs() < 0.03);
                assert!((rotation_deg - 30.0).abs() < 3.0);
            }
            ColumnShape::Circular { .. } => panic!("expected rectangular shape"),
        }
    }

    #[test]
    fn circular_fitting() {
        // Create a circular column with diameter 0.40 m at (5.0, 5.0)
        let cx = 5.0;
        let cy = 5.0;
        let radius = 0.20;
        let mut pts = Vec::new();
        for i in 0..36 {
            let angle = (i as f64 * 10.0).to_radians();
            let x = cx + radius * angle.cos();
            let y = cy + radius * angle.sin();
            pts.push([x, y]);
        }

        let obb = compute_obb_pca(&pts).expect("OBB should compute");
        assert!((obb.center[0] - cx).abs() < 0.02);
        assert!((obb.center[1] - cy).abs() < 0.02);
        assert!((obb.width - 0.40).abs() < 0.03);
        assert!((obb.depth - 0.40).abs() < 0.03);

        let shape = classify_shape(&pts, &obb);
        match shape {
            ColumnShape::Circular { diameter } => {
                assert!((diameter - 0.40).abs() < 0.03, "diameter: {diameter}");
            }
            ColumnShape::Rectangular { .. } => panic!("expected circular shape"),
        }
    }

    #[test]
    fn three_tier_vertical_continuity() {
        let storey = Storey { index: 0, base_z: 0.0, top_z: 2.80 };
        let opts = BimOptions::default();

        // 1. Column points spanning full height (z from 0.15 to 2.65 in 3 tiers)
        let mut col_pts = Vec::new();
        for k in 0..25 {
            let z = 0.15 + (k as f64) * 0.10;
            // Cross-section points around (2.0, 2.0)
            for i in 0..10 {
                let ang = (i as f64 * 36.0).to_radians();
                col_pts.push([2.0 + 0.15 * ang.cos(), 2.0 + 0.15 * ang.sin(), z]);
            }
        }

        let detected = detect_columns(&col_pts, &storey, &[], &opts).expect("should succeed");
        assert_eq!(detected.len(), 1, "column with points in all 3 tiers must be detected");
        assert!((detected[0].center[0] - 2.0).abs() < 0.05);
        assert!((detected[0].center[1] - 2.0).abs() < 0.05);

        // 2. Low furniture cluster (only z in [0.15, 0.65], missing upper tier [1.96, 2.66])
        let mut furn_pts = Vec::new();
        for k in 0..5 {
            let z = 0.20 + (k as f64) * 0.08;
            for i in 0..10 {
                let ang = (i as f64 * 36.0).to_radians();
                furn_pts.push([2.0 + 0.15 * ang.cos(), 2.0 + 0.15 * ang.sin(), z]);
            }
        }
        let furn_detected = detect_columns(&furn_pts, &storey, &[], &opts).expect("should succeed");
        assert_eq!(furn_detected.len(), 0, "low furniture missing upper tier must be rejected");
    }

    #[test]
    fn wall_exclusion() {
        let storey = Storey { index: 0, base_z: 0.0, top_z: 2.80 };
        let opts = BimOptions::default();

        let wall =
            Wall { from: [0.0, 0.0], to: [5.0, 0.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None };

        // Points inside the wall corridor at y = 0.05 (distance = 0.05 <= 0.20*0.5 + 0.10 = 0.20)
        let mut wall_pts = Vec::new();
        for k in 0..25 {
            let z = 0.15 + (k as f64) * 0.10;
            for i in 0..10 {
                let x = 1.0 + (i as f64) * 0.04;
                wall_pts.push([x, 0.05, z]);
            }
        }

        let detected = detect_columns(&wall_pts, &storey, std::slice::from_ref(&wall), &opts).expect("should succeed");
        assert_eq!(detected.len(), 0, "points inside wall corridor must be excluded from columns");

        // Column placed at y = 1.50 (well outside wall corridor)
        let mut col_pts = Vec::new();
        for k in 0..25 {
            let z = 0.15 + (k as f64) * 0.10;
            for i in 0..10 {
                let ang = (i as f64 * 36.0).to_radians();
                col_pts.push([2.0 + 0.15 * ang.cos(), 1.50 + 0.15 * ang.sin(), z]);
            }
        }
        let detected2 = detect_columns(&col_pts, &storey, &[wall], &opts).expect("should succeed");
        assert_eq!(detected2.len(), 1, "column outside wall corridor must be detected");
    }

    #[test]
    fn column_candidate_against_wall_is_rejected() {
        let storey = Storey { index: 0, base_z: 0.0, top_z: 2.80 };
        let opts = BimOptions::default();

        let wall =
            Wall { from: [0.0, 0.0], to: [5.0, 0.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None };

        // Column-like candidate positioned at y = 0.28 (outside 0.20 corridor, but within 0.35 clearance)
        let mut col_pts = Vec::new();
        for k in 0..25 {
            let z = 0.15 + (k as f64) * 0.10;
            for i in 0..10 {
                let ang = (i as f64 * 36.0).to_radians();
                // Radius 0.05 m, centered at x = 2.50, y = 0.28
                col_pts.push([2.50 + 0.05 * ang.cos(), 0.28 + 0.05 * ang.sin(), z]);
            }
        }

        let detected = detect_columns(&col_pts, &storey, &[wall], &opts).expect("should succeed");
        assert_eq!(detected.len(), 0, "column candidate against a wall must be rejected");
    }

    #[test]
    fn vertical_tier_min_points_threshold() {
        let storey = Storey { index: 0, base_z: 0.0, top_z: 2.80 };
        let opts = BimOptions::default();

        // Column with only 5 points in the lower tier (threshold is 6)
        // lower tier is [0.28, 0.98], mid is [1.12, 1.82], upper is [1.96, 2.66]
        let mut col_pts = Vec::new();
        // 5 points in lower tier
        for i in 0..5 {
            col_pts.push([2.0, 2.0, 0.35 + (i as f64) * 0.10]);
        }
        // 10 points in mid tier
        for i in 0..10 {
            col_pts.push([2.0, 2.0, 1.20 + (i as f64) * 0.05]);
        }
        // 10 points in upper tier
        for i in 0..10 {
            col_pts.push([2.0, 2.0, 2.00 + (i as f64) * 0.05]);
        }

        let detected = detect_columns(&col_pts, &storey, &[], &opts).expect("should succeed");
        assert_eq!(detected.len(), 0, "column with count_lower < 6 must be rejected");
    }
}
