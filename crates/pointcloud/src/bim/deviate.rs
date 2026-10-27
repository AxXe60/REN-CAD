//! Cloud-to-model deviation and the furniture it isolates.
//!
//! Measures distance from cloud points to modelled walls and slabs, computes RMSE and mean
//! deviations in millimetres, and clusters off-model points (deviation > `deviation_threshold`)
//! into furniture components using 26-neighbour connectivity *(ours)*.

use std::collections::HashMap;

use cadcraft_geom::{Line, Vec2, point_in_polygon};

use crate::bim::slabs::Slab;
use crate::bim::walls::Wall;
use crate::bim::{BimError, BimOptions};

/// A cluster of off-model points representing an isolated piece of furniture.
#[derive(Debug, Clone, PartialEq)]
pub struct FurnitureCluster {
    /// World-space minimum AABB corner `[min_x, min_y, min_z]` in metres.
    pub min: [f64; 3],
    /// World-space maximum AABB corner `[max_x, max_y, max_z]` in metres.
    pub max: [f64; 3],
    /// Number of points contained in this cluster.
    pub points: usize,
    /// Mean distance from the points in this cluster to the nearest model surface, in millimetres.
    pub mean_mm: f64,
}

/// Cloud-to-model deviation statistics and isolated furniture clusters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviationReport {
    /// Count of valid, finite points evaluated against the model.
    pub points: usize,
    /// Root-mean-square error in millimetres: `1000 * sqrt(Σ d² / n)`.
    pub rmse_mm: f64,
    /// Arithmetic mean deviation in millimetres: `1000 * (Σ d / n)`.
    pub mean_mm: f64,
    /// Maximum deviation from any valid point to the model, in millimetres.
    pub max_mm: f64,
    /// Count of points whose distance exceeds `deviation_threshold`.
    pub above_threshold: usize,
    /// Connected furniture components sorted deterministically by `(min.x, min.y, min.z)`.
    pub furniture: Vec<FurnitureCluster>,
}

/// Pre-calculated 3D bounding box representation of a wall body for fast point queries.
struct PreparedWall {
    from: [f64; 2],
    d: [f64; 2],
    n: [f64; 2],
    len: f64,
    half_thickness: f64,
    base_z: f64,
    height: f64,
    aabb_min: [f64; 3],
    aabb_max: [f64; 3],
}

/// Pre-calculated prism representation of a slab body for fast point queries.
struct PreparedSlab {
    polygon_vec2: Vec<Vec2>,
    holes_vec2: Vec<Vec<Vec2>>,
    edges: Vec<Line>,
    base_z: f64,
    top_z: f64,
}

/// Per-cell point accumulator in the 3D voxel grid.
struct CellAcc {
    key: (i64, i64, i64),
    min: [f64; 3],
    max: [f64; 3],
    points: usize,
    sum_d_mm: f64,
}

/// Computes the shortest distance from point `p` to the wall box body.
///
/// Inside the body evaluates to zero distance.
fn wall_dist(w: &PreparedWall, p: [f64; 3]) -> f64 {
    let [px, py, pz] = p;
    let [fx, fy] = w.from;
    let [dx, dy] = w.d;
    let [nx, ny] = w.n;
    let rx = px - fx;
    let ry = py - fy;
    let u = rx * dx + ry * dy;
    let v = rx * nx + ry * ny;
    let wz = pz - w.base_z;

    let du = 0.0_f64.max(-u).max(u - w.len);
    let dv = 0.0_f64.max(v.abs() - w.half_thickness);
    let dw = 0.0_f64.max(-wz).max(wz - w.height);

    (du * du + dv * dv + dw * dw).sqrt()
}

/// Computes the shortest distance from point `p` to the slab prism body.
///
/// Inside the body evaluates to zero distance.
fn slab_dist(s: &PreparedSlab, p: [f64; 3]) -> f64 {
    let [px, py, pz] = p;
    let p2 = Vec2::new(px, py);
    let d2 = if point_in_polygon(&s.polygon_vec2, p2) && !s.holes_vec2.iter().any(|h| point_in_polygon(h, p2)) {
        0.0
    } else {
        let mut min_edge_d = f64::INFINITY;
        for line in &s.edges {
            let ed = line.dist(p2);
            if ed < min_edge_d {
                min_edge_d = ed;
            }
        }
        min_edge_d
    };
    let dz = 0.0_f64.max(s.base_z - pz).max(pz - s.top_z);
    (d2 * d2 + dz * dz).sqrt()
}

/// Measures deviation between a point cloud and the reconstructed BIM model.
///
/// # Algorithm & Decisions
/// - Validates options: `deviation_threshold >= 0` *(ours: default 0.05 m)*, `deviation_cell > 0`
///   *(ours: default 0.10 m)*, `min_furniture_points >= 1` *(ours: default 50)*, and
///   `max_furniture_clusters >= 1` *(ours: default 4096)*; otherwise returns [`BimError::InvalidCell`].
/// - An empty point cloud returns an empty [`DeviationReport`].
/// - If there are no valid walls and no valid slabs, returns [`BimError::Empty`] because there is no
///   model to evaluate distances against.
/// - Non-finite points and hostile or degenerate walls/slabs are safely skipped without panic.
/// - Distance is the minimum distance across all wall box bodies and slab prism bodies.
/// - Points with deviation strictly greater than `deviation_threshold` are bucketed into
///   voxel cubes of width `deviation_cell`.
/// - 26-neighbour connectivity *(ours)* is used to join occupied voxel cubes: diagonal connectivity
///   ensures parts of a single furniture item (e.g., thin chair legs meeting a seat) remain in one
///   connected component rather than fragmenting into multiple clusters across voxel edges.
/// - Components with at least `min_furniture_points` become [`FurnitureCluster`]s. If the number of
///   qualifying clusters exceeds `max_furniture_clusters`, [`BimError::TooLarge`] is returned.
/// - Clusters are sorted deterministically by `(min.x, min.y, min.z)`.
pub fn measure(points: &[[f64; 3]], walls: &[Wall], slabs: &[Slab], opts: &BimOptions) -> Result<DeviationReport, BimError> {
    if !opts.deviation_threshold.is_finite()
        || opts.deviation_threshold < 0.0
        || !opts.deviation_cell.is_finite()
        || opts.deviation_cell <= 0.0
        || opts.min_furniture_points < 1
        || opts.max_furniture_clusters < 1
    {
        return Err(BimError::InvalidCell);
    }

    if points.is_empty() {
        return Ok(DeviationReport::default());
    }

    // Filter and prepare valid walls.
    let mut prepared_walls = Vec::with_capacity(walls.len());
    for w in walls {
        let [fx, fy] = w.from;
        let [tx, ty] = w.to;
        if !fx.is_finite()
            || !fy.is_finite()
            || !tx.is_finite()
            || !ty.is_finite()
            || !w.base_z.is_finite()
            || !w.height.is_finite()
            || !w.thickness.is_finite()
            || w.thickness < 0.0
            || w.height <= 0.0
        {
            continue;
        }
        let dx = tx - fx;
        let dy = ty - fy;
        let len = dx.hypot(dy);
        if !len.is_finite() || len <= 0.0 {
            continue;
        }
        let d = [dx / len, dy / len];
        let n = [-d[1], d[0]];
        let ht = w.thickness / 2.0;
        let corners =
            [[fx + n[0] * ht, fy + n[1] * ht], [fx - n[0] * ht, fy - n[1] * ht], [tx + n[0] * ht, ty + n[1] * ht], [tx - n[0] * ht, ty - n[1] * ht]];
        let aabb_min =
            [corners.iter().map(|c| c[0]).fold(f64::INFINITY, f64::min), corners.iter().map(|c| c[1]).fold(f64::INFINITY, f64::min), w.base_z];
        let aabb_max = [
            corners.iter().map(|c| c[0]).fold(f64::NEG_INFINITY, f64::max),
            corners.iter().map(|c| c[1]).fold(f64::NEG_INFINITY, f64::max),
            w.base_z + w.height,
        ];
        prepared_walls.push(PreparedWall { from: [fx, fy], d, n, len, half_thickness: ht, base_z: w.base_z, height: w.height, aabb_min, aabb_max });
    }

    // Filter and prepare valid slabs.
    let mut prepared_slabs = Vec::with_capacity(slabs.len());
    for s in slabs {
        if !s.base_z.is_finite() || !s.top_z.is_finite() || s.polygon.len() < 3 {
            continue;
        }
        let mut valid_poly = true;
        let mut poly_vec2 = Vec::with_capacity(s.polygon.len());
        for v in &s.polygon {
            let [vx, vy] = *v;
            if !vx.is_finite() || !vy.is_finite() {
                valid_poly = false;
                break;
            }
            poly_vec2.push(Vec2::new(vx, vy));
        }
        if !valid_poly || poly_vec2.len() < 3 {
            continue;
        }
        let n = poly_vec2.len();
        let mut edges = Vec::with_capacity(n);
        for i in 0..n {
            let Some(&a) = poly_vec2.get(i) else { continue };
            let Some(&b) = poly_vec2.get((i + 1) % n) else { continue };
            edges.push(Line::new(a, b));
        }
        let mut holes_vec2 = Vec::new();
        for hole in &s.holes {
            let mut hole_v2 = Vec::with_capacity(hole.len());
            let mut valid_hole = true;
            for v in hole {
                let [vx, vy] = *v;
                if !vx.is_finite() || !vy.is_finite() {
                    valid_hole = false;
                    break;
                }
                hole_v2.push(Vec2::new(vx, vy));
            }
            if valid_hole && hole_v2.len() >= 3 {
                let m = hole_v2.len();
                for i in 0..m {
                    let Some(&a) = hole_v2.get(i) else { continue };
                    let Some(&b) = hole_v2.get((i + 1) % m) else { continue };
                    edges.push(Line::new(a, b));
                }
                holes_vec2.push(hole_v2);
            }
        }
        prepared_slabs.push(PreparedSlab { polygon_vec2: poly_vec2, holes_vec2, edges, base_z: s.base_z.min(s.top_z), top_z: s.base_z.max(s.top_z) });
    }

    if prepared_walls.is_empty() && prepared_slabs.is_empty() {
        return Err(BimError::Empty);
    }

    let mut valid_points_count = 0usize;
    let mut sum_d = 0.0f64;
    let mut sum_d2 = 0.0f64;
    let mut max_d = 0.0f64;
    let mut above_threshold_count = 0usize;

    let inv_cell = 1.0 / opts.deviation_cell;
    let mut grid: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut cells: Vec<CellAcc> = Vec::new();

    for pt in points {
        let [px, py, pz] = *pt;
        if !px.is_finite() || !py.is_finite() || !pz.is_finite() {
            continue;
        }

        let mut min_d = f64::INFINITY;
        for w in &prepared_walls {
            let dx = 0.0_f64.max(w.aabb_min[0] - px).max(px - w.aabb_max[0]);
            let dy = 0.0_f64.max(w.aabb_min[1] - py).max(py - w.aabb_max[1]);
            let dz = 0.0_f64.max(w.aabb_min[2] - pz).max(pz - w.aabb_max[2]);
            if dx >= min_d || dy >= min_d || dz >= min_d {
                continue;
            }
            let d = wall_dist(w, [px, py, pz]);
            if d < min_d {
                min_d = d;
                if min_d == 0.0 {
                    break;
                }
            }
        }
        if min_d > 0.0 {
            for s in &prepared_slabs {
                let dz = 0.0_f64.max(s.base_z - pz).max(pz - s.top_z);
                if dz >= min_d {
                    continue;
                }
                let d = slab_dist(s, [px, py, pz]);
                if d < min_d {
                    min_d = d;
                    if min_d == 0.0 {
                        break;
                    }
                }
            }
        }

        valid_points_count = valid_points_count.saturating_add(1);
        sum_d += min_d;
        sum_d2 += min_d * min_d;
        if min_d > max_d {
            max_d = min_d;
        }

        if min_d > opts.deviation_threshold {
            above_threshold_count = above_threshold_count.saturating_add(1);

            let kx_f = (px * inv_cell).floor();
            let ky_f = (py * inv_cell).floor();
            let kz_f = (pz * inv_cell).floor();

            // Safeguard against extreme hostile coordinates overflowing i64.
            if kx_f < i64::MIN as f64
                || kx_f > i64::MAX as f64
                || ky_f < i64::MIN as f64
                || ky_f > i64::MAX as f64
                || kz_f < i64::MIN as f64
                || kz_f > i64::MAX as f64
            {
                continue;
            }

            let key = (kx_f as i64, ky_f as i64, kz_f as i64);
            let d_mm = min_d * 1000.0;

            if let Some(&idx) = grid.get(&key) {
                if let Some(cell) = cells.get_mut(idx) {
                    cell.min[0] = cell.min[0].min(px);
                    cell.min[1] = cell.min[1].min(py);
                    cell.min[2] = cell.min[2].min(pz);
                    cell.max[0] = cell.max[0].max(px);
                    cell.max[1] = cell.max[1].max(py);
                    cell.max[2] = cell.max[2].max(pz);
                    cell.points = cell.points.saturating_add(1);
                    cell.sum_d_mm += d_mm;
                }
            } else {
                let idx = cells.len();
                grid.insert(key, idx);
                cells.push(CellAcc { key, min: [px, py, pz], max: [px, py, pz], points: 1, sum_d_mm: d_mm });
            }
        }
    }

    if valid_points_count == 0 {
        return Ok(DeviationReport::default());
    }

    let n = valid_points_count as f64;
    let rmse_mm = 1000.0 * (sum_d2 / n).max(0.0).sqrt();
    let mean_mm = 1000.0 * (sum_d / n);
    let max_mm = 1000.0 * max_d;

    // Join occupied voxel cells using 26-neighbour connectivity.
    let mut visited = vec![false; cells.len()];
    let mut stack = Vec::with_capacity(cells.len());
    let mut furniture = Vec::new();

    for start_idx in 0..cells.len() {
        match visited.get(start_idx) {
            Some(&true) | None => continue,
            Some(&false) => (),
        }
        if let Some(v) = visited.get_mut(start_idx) {
            *v = true;
        }
        stack.push(start_idx);

        let mut comp_points = 0usize;
        let mut comp_sum_d_mm = 0.0f64;
        let mut comp_min = [f64::INFINITY; 3];
        let mut comp_max = [f64::NEG_INFINITY; 3];

        while let Some(curr_idx) = stack.pop() {
            let Some(cell) = cells.get(curr_idx) else {
                continue;
            };
            comp_points = comp_points.saturating_add(cell.points);
            comp_sum_d_mm += cell.sum_d_mm;
            for k in 0..3 {
                if let (Some(cmin), Some(pmin)) = (comp_min.get_mut(k), cell.min.get(k)) {
                    *cmin = (*cmin).min(*pmin);
                }
                if let (Some(cmax), Some(pmax)) = (comp_max.get_mut(k), cell.max.get(k)) {
                    *cmax = (*cmax).max(*pmax);
                }
            }

            let (cx, cy, cz) = cell.key;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        let nbr_key = (cx.saturating_add(dx), cy.saturating_add(dy), cz.saturating_add(dz));
                        if let Some(&nbr_idx) = grid.get(&nbr_key)
                            && let Some(v) = visited.get_mut(nbr_idx)
                            && !*v
                        {
                            *v = true;
                            stack.push(nbr_idx);
                        }
                    }
                }
            }
        }

        if comp_points >= opts.min_furniture_points {
            if furniture.len() >= opts.max_furniture_clusters {
                return Err(BimError::TooLarge { what: "furniture clusters", limit: opts.max_furniture_clusters });
            }
            let mean_mm = comp_sum_d_mm / (comp_points as f64);
            furniture.push(FurnitureCluster { min: comp_min, max: comp_max, points: comp_points, mean_mm });
        }
    }

    // Deterministic sort by (min.x, min.y, min.z).
    furniture.sort_by(|a, b| {
        a.min[0]
            .total_cmp(&b.min[0])
            .then_with(|| a.min[1].total_cmp(&b.min[1]))
            .then_with(|| a.min[2].total_cmp(&b.min[2]))
            .then_with(|| a.max[0].total_cmp(&b.max[0]))
            .then_with(|| a.max[1].total_cmp(&b.max[1]))
            .then_with(|| a.max[2].total_cmp(&b.max[2]))
    });

    Ok(DeviationReport { points: valid_points_count, rmse_mm, mean_mm, max_mm, above_threshold: above_threshold_count, furniture })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_model() -> (Vec<Wall>, Vec<Slab>) {
        let w = |from, to, exterior| Wall { from, to, thickness: 0.20, base_z: 0.0, height: 2.8, exterior, is_parapet: false, arc: None };
        let walls =
            vec![w([0.0, 0.0], [6.0, 0.0], true), w([6.0, 0.0], [6.0, 4.0], true), w([6.0, 4.0], [0.0, 4.0], true), w([0.0, 4.0], [0.0, 0.0], true)];
        let slabs = vec![
            Slab {
                base_z: -0.2,
                top_z: 0.0,
                thickness: 0.2,
                polygon: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 4.0], [0.0, 4.0]],
                holes: Vec::new(),
                single_sided: true,
            },
            Slab {
                base_z: 2.8,
                top_z: 3.0,
                thickness: 0.2,
                polygon: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 4.0], [0.0, 4.0]],
                holes: Vec::new(),
                single_sided: true,
            },
        ];
        (walls, slabs)
    }

    #[test]
    fn points_on_the_model_surfaces_report_an_rmse_below_a_millimetre() {
        let (walls, slabs) = box_model();
        let mut pts = Vec::new();
        // Floor and ceiling on the visible faces.
        for i in 0..61 {
            for j in 0..41 {
                pts.push([f64::from(i) * 0.1, f64::from(j) * 0.1, 0.0]);
                pts.push([f64::from(i) * 0.1, f64::from(j) * 0.1, 2.8]);
            }
        }
        // The four inner faces at ±0.1 from the axes.
        for i in 0..61 {
            let x = f64::from(i) * 0.1;
            for k in 0..29 {
                pts.push([x, 0.1, f64::from(k) * 0.1]);
                pts.push([x, 3.9, f64::from(k) * 0.1]);
            }
        }
        for j in 0..41 {
            let y = f64::from(j) * 0.1;
            for k in 0..29 {
                pts.push([0.1, y, f64::from(k) * 0.1]);
                pts.push([5.9, y, f64::from(k) * 0.1]);
            }
        }
        let r = measure(&pts, &walls, &slabs, &BimOptions::default()).expect("a model");
        assert!(r.rmse_mm < 1.0, "the model is where the points are: RMSE {} mm", r.rmse_mm);
        assert_eq!(r.above_threshold, 0);
        assert!(r.furniture.is_empty());
    }

    #[test]
    fn furniture_becomes_one_cluster_with_the_right_box_and_mean() {
        let (walls, slabs) = box_model();
        let mut pts = Vec::new();
        // Room points on model surfaces.
        for i in 0..61 {
            for j in 0..41 {
                pts.push([f64::from(i) * 0.1, f64::from(j) * 0.1, 0.0]);
            }
        }
        // 1.2 x 0.6 x 0.45 m sofa in middle of the room: x in [2.4, 3.6], y in [1.7, 2.3], z in [0.0, 0.45].
        let mut sofa_pts = Vec::new();
        // Top surface of sofa at z = 0.45 (distance = 450 mm above floor).
        for ix in 0..16 {
            let x = 2.4 + f64::from(ix) * 0.08;
            for iy in 0..8 {
                let y = 1.7 + f64::from(iy) * 0.08;
                sofa_pts.push([x, y, 0.45]);
            }
        }
        // Four legs down to z = 0.08 (within 0.1 m of sofa base at z = 0).
        for &(lx, ly) in &[(2.4, 1.7), (3.6, 1.7), (2.4, 2.26), (3.6, 2.26)] {
            for iz in 0..5 {
                let z = 0.08 + f64::from(iz) * 0.08;
                sofa_pts.push([lx, ly, z]);
            }
        }
        let sofa_count = sofa_pts.len();
        pts.extend(sofa_pts);

        let r = measure(&pts, &walls, &slabs, &BimOptions::default()).expect("measure succeeds");
        assert_eq!(r.above_threshold, sofa_count, "above_threshold equals sofa point count");
        assert_eq!(r.furniture.len(), 1, "exactly one furniture cluster");
        let c = &r.furniture[0];
        assert_eq!(c.points, sofa_count);
        assert!((c.min[0] - 2.4).abs() <= 0.1, "min.x within 0.1 m of sofa");
        assert!((c.max[0] - 3.6).abs() <= 0.1, "max.x within 0.1 m of sofa");
        assert!((c.min[1] - 1.7).abs() <= 0.1, "min.y within 0.1 m of sofa");
        assert!((c.max[1] - 2.3).abs() <= 0.1, "max.y within 0.1 m of sofa");
        assert!((c.min[2] - 0.0).abs() <= 0.1, "min.z within 0.1 m of sofa");
        assert!((c.max[2] - 0.45).abs() <= 0.1, "max.z within 0.1 m of sofa");
        assert!(c.mean_mm >= 300.0 && c.mean_mm <= 600.0, "mean_mm between 300 and 600, got {}", c.mean_mm);
    }

    #[test]
    fn empty_cloud_gives_empty_report_and_unmodelled_cloud_gives_empty_error() {
        let (walls, slabs) = box_model();
        let o = BimOptions::default();
        let r = measure(&[], &walls, &slabs, &o).expect("empty cloud is an empty report");
        assert_eq!(r, DeviationReport::default());

        let pts = vec![[3.0, 2.0, 1.0]];
        let err = measure(&pts, &[], &[], &o).unwrap_err();
        assert!(matches!(err, BimError::Empty));
    }

    #[test]
    fn hostile_options_and_geometry_never_panic() {
        let (mut walls, mut slabs) = box_model();
        let o = BimOptions::default();

        // Negative threshold -> InvalidCell
        let mut bad_opts = o;
        bad_opts.deviation_threshold = -0.05;
        assert!(matches!(measure(&[[3.0, 2.0, 0.0]], &walls, &slabs, &bad_opts), Err(BimError::InvalidCell)));

        // Zero cell -> InvalidCell
        let mut bad_opts2 = o;
        bad_opts2.deviation_cell = 0.0;
        assert!(matches!(measure(&[[3.0, 2.0, 0.0]], &walls, &slabs, &bad_opts2), Err(BimError::InvalidCell)));

        // Zero min_furniture_points -> InvalidCell
        let mut bad_opts3 = o;
        bad_opts3.min_furniture_points = 0;
        assert!(matches!(measure(&[[3.0, 2.0, 0.0]], &walls, &slabs, &bad_opts3), Err(BimError::InvalidCell)));

        // Zero max_furniture_clusters -> InvalidCell
        let mut bad_opts4 = o;
        bad_opts4.max_furniture_clusters = 0;
        assert!(matches!(measure(&[[3.0, 2.0, 0.0]], &walls, &slabs, &bad_opts4), Err(BimError::InvalidCell)));

        // NaN point, NaN wall, slab with two vertices are skipped and do not panic
        let pts = vec![
            [f64::NAN, 2.0, 0.0],
            [3.0, 2.0, 0.0], // on floor slab, d = 0
        ];
        walls.push(Wall {
            from: [f64::NAN, 0.0],
            to: [1.0, 0.0],
            thickness: 0.2,
            base_z: 0.0,
            height: 2.8,
            exterior: false,
            is_parapet: false,
            arc: None,
        });
        slabs.push(Slab {
            base_z: 0.0,
            top_z: 0.2,
            thickness: 0.2,
            polygon: vec![[0.0, 0.0], [1.0, 1.0]], // only 2 vertices
            holes: Vec::new(),
            single_sided: false,
        });
        let r = measure(&pts, &walls, &slabs, &o).expect("bad geometry and points skipped");
        assert_eq!(r.points, 1, "NaN point is skipped");
        assert!(r.rmse_mm < 1.0);

        // More clusters than the cap returns TooLarge
        let mut cap_opts = o;
        cap_opts.max_furniture_clusters = 1;
        cap_opts.min_furniture_points = 5;
        // Two distinct clusters far apart in space
        let mut two_clusters = Vec::new();
        for i in 0..5 {
            two_clusters.push([1.0, 1.0, 1.0 + f64::from(i) * 0.01]);
            two_clusters.push([4.0, 3.0, 1.0 + f64::from(i) * 0.01]);
        }
        let (clean_walls, clean_slabs) = box_model();
        let err = measure(&two_clusters, &clean_walls, &clean_slabs, &cap_opts).unwrap_err();
        assert!(matches!(err, BimError::TooLarge { what: "furniture clusters", limit: 1 }));
    }

    #[test]
    fn points_inside_slab_void_have_distance_greater_than_zero() {
        let o = BimOptions::default();
        // Slab spanning [0.0, 0.0] to [10.0, 10.0], z in [0.0, 0.20]
        // with a hole spanning [4.0, 4.0] to [6.0, 6.0].
        let slab = Slab {
            base_z: 0.0,
            top_z: 0.20,
            thickness: 0.20,
            polygon: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            holes: vec![vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]]],
            single_sided: false,
        };
        // Point in solid part of slab (e.g. [2.0, 2.0, 0.10]) -> d == 0
        // Point in void hole (e.g. [5.0, 5.0, 0.10]) -> d == 1.0 m (dist to hole edge at x=4 or x=6, y=4 or y=6)
        let pts = vec![
            [2.0, 2.0, 0.10], // solid
            [5.0, 5.0, 0.10], // void center
        ];
        let rep = measure(&pts, &[], &[slab], &o).expect("measure succeeds");
        assert_eq!(rep.points, 2);
        // The point at [5.0, 5.0, 0.10] has distance ~1.0 m = 1000 mm to the nearest hole boundary edge
        assert!(rep.max_mm >= 950.0 && rep.max_mm <= 1050.0, "point inside void has distance ~1.0 m to boundary, got {}", rep.max_mm);
        assert_eq!(rep.above_threshold, 1, "void point is above the 50 mm threshold");
    }
}
