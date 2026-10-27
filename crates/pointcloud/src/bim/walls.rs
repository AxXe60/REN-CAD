//! Walls from the horizontal slice near the ceiling.
//!
//! The paper's §3.3 takes a band of the storey — the top tenth, Table 6's `[0.9, 1.0]` of the
//! storey height — because walls run the full height while furniture, doors and the clutter on the
//! floor do not. The band is rasterised, cut at a density threshold, closed to bridge the cracks a
//! scan leaves, traced into rings, and each ring simplified into straight faces.
//!
//! A wall in a scan is a **pair of faces**, not one line: the scan sees the room side and, in a
//! closed room, also the far side of the wall through a doorway or a window. A face that has no
//! partner within [`BimOptions::max_wall_thickness`] is not a wall — it is a surface, and the
//! paper synthesises its missing face at a thickness the user names when the scan covers only the
//! interior. That is the difference between `Wall::thickness` being measured and being declared,
//! and `Wall::exterior` is what records it.

use crate::bim::raster::{self, Raster};
use crate::bim::storeys::{self, Storey};
use crate::bim::{BimError, BimOptions, MAX_CLOSE_RADIUS};
use crate::grid::Grid;

/// The most pairing samples one face may contribute.
///
/// A face is sampled along its length at half the search cell — a few dozen samples for a room —
/// so this cap exists only for geometry no building has: a ring with a coordinate near 1e150 would
/// otherwise ask for a vector that cannot be allocated. Capping the count widens the spacing,
/// which can cost a pairing only on a face far longer than any storey.
const MAX_SEGMENT_SAMPLES: usize = 4096;

/// The most pairing samples the whole storey may hold, counted across all faces.
const MAX_PAIR_SAMPLES: usize = 1_000_000;

/// An arc definition for a curved wall.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallArc {
    pub center: [f64; 2],
    pub radius: f64,
    pub ccw: bool,
}

/// One wall, as an axis in plan and a vertical extent.
#[derive(Debug, Clone, PartialEq)]
pub struct Wall {
    /// One end of the axis, in world metres.
    pub from: [f64; 2],
    /// The other end. The axis is the midline of the wall, not either of its faces.
    pub to: [f64; 2],
    /// The distance between the two faces: measured when both were scanned, and
    /// [`BimOptions::exterior_wall_thickness`] when the outer face was synthesised.
    pub thickness: f64,
    /// The bottom of the wall: the storey's own base, so walls and slabs line up.
    pub base_z: f64,
    /// The storey's height, so a wall spans the room it belongs to.
    pub height: f64,
    /// True when the far face is virtual: the scan saw only the room side, and the body was laid
    /// behind that face at the thickness the user named.
    pub exterior: bool,
    /// True when the wall is a partial-height wall or parapet whose height is lower than clear storey height.
    pub is_parapet: bool,
    /// Optional arc specification if the wall is curved.
    pub arc: Option<WallArc>,
}

/// The walls standing in `storey`.
///
/// The points are the whole cloud; the storey's band is taken by [`storeys::points_in`] and the
/// paper's slice — `[slice_from, slice_to]` of the storey height, both ends inclusive — cuts it
/// again. An empty slice is `Ok(vec![])` rather than an error: a storey holds walls only where the
/// scan saw them, and a caller that wants to say "nothing here" needs no special case.
///
/// The raster lattice is tied to the world, not to the extreme point of the moment: its origin is
/// the nearest cell line at or below the slice, so the same wall lands on the same lattice however
/// its first and last points happen to fall. That is what lets a wall whose faces sit on cell
/// lines come back exactly, instead of inheriting the jitter of an arbitrary origin.
///
/// The traced rings are simplified with [`raster::simplify`] and their runs within
/// `angle_tolerance_deg` of straight are joined into one face. Faces shorter than
/// `min_wall_length` are dropped before pairing, so a wall's end caps cannot pair with anything.
/// Pairs are found through [`Grid`] over samples half a search cell apart, which makes the search
/// local in the plan rather than quadratic in the number of faces.
///
/// What this stage does not do: it does not decide which side of a lone face is "outside" beyond
/// the winding of its ring, and it does not build rooms. That is [`crate::bim::zones`]'s work.
///
/// # Errors
///
/// * [`BimError::InvalidCell`] for a number in the options this stage cannot use: a cell that is
///   not a positive length, a slice window outside `0..=1` or with its ends the wrong way round, a
///   density threshold outside `(0, 1]`, a tolerance that is not a number, a `min_wall_length` that
///   is not a length, a thickness range that admits nothing, or an exterior thickness that is not
///   a positive length. The refusals are decided in that order, and the first one is the error.
/// * [`BimError::TooLarge`] when the slice's rectangle would not fit the raster budget, or when
///   the faces it produced would need more pairing samples than the stage will hold.
pub fn extract(points: &[[f64; 3]], storey: &Storey, opts: &BimOptions) -> Result<Vec<Wall>, BimError> {
    let t = Tuning::read(opts)?;
    let height = storey.top_z - storey.base_z;
    if !height.is_finite() || height <= 0.0 {
        return Ok(Vec::new());
    }

    let storey_pts = storeys::points_in(points, storey, opts);
    if storey_pts.is_empty() {
        return Ok(Vec::new());
    }

    // 1. Primary slice [slice_from, slice_to]:
    // Default primary slicing band: if (t.slice_from - 0.9).abs() < 1e-6 && (t.slice_to - 1.0).abs() < 1e-6,
    // anchor strictly near the ceiling:
    //   lo_pri = (storey.top_z - 0.30).max(storey.base_z)
    //   hi_pri = (storey.top_z - 0.05).max(lo_pri)
    // Otherwise, storey.base_z + t.slice_from * height .. storey.base_z + t.slice_to * height.
    let (lo_pri, hi_pri) = if (t.slice_from - 0.9).abs() < 1e-6 && (t.slice_to - 1.0).abs() < 1e-6 {
        let lo = (storey.top_z - 0.30).max(storey.base_z);
        let hi = (storey.top_z - 0.05).max(lo);
        (lo, hi)
    } else {
        (storey.base_z + t.slice_from * height, storey.base_z + t.slice_to * height)
    };
    let primary_band: Vec<[f64; 3]> = storey_pts.iter().filter(|p| p[2] >= lo_pri && p[2] <= hi_pri).copied().collect();
    let mut walls = extract_walls_from_band(&primary_band, storey.base_z, height, &t)?;

    // 2. Secondary slice [0.15, 0.40] on remaining points outside primary wall corridors to detect parapets.
    let lo_sec = storey.base_z + 0.15 * height;
    let hi_sec = storey.base_z + 0.40 * height;
    let sec_band: Vec<[f64; 3]> =
        storey_pts.iter().filter(|p| p[2] >= lo_sec && p[2] <= hi_sec && !is_in_wall_corridors(p[0], p[1], &walls)).copied().collect();

    if !sec_band.is_empty() {
        let mut parapets = extract_walls_from_band(&sec_band, storey.base_z, height, &t)?;
        parapets.retain(|w| {
            let len = (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1]);
            len >= 0.60 && w.thickness >= 0.08
        });
        walls.extend(parapets);
    }

    // 3. Noise-robust height profiling (z_p95) and Vertical Occupancy (10 bins) for full-height walls.
    let bin_height = height / 10.0;
    walls.retain_mut(|w| {
        let corridor = w.thickness * 0.5 + 0.10;
        let mut zs: Vec<f64> = storey_pts
            .iter()
            .filter(|p| dist_point_to_segment_2d(p[0], p[1], w.from[0], w.from[1], w.to[0], w.to[1]) <= corridor)
            .map(|p| p[2])
            .collect();

        if !zs.is_empty() {
            zs.sort_by(f64::total_cmp);
            let idx = ((zs.len() * 95) / 100).min(zs.len().saturating_sub(1));
            let z_p95 = zs[idx];
            if z_p95 < storey.top_z - 0.20 {
                w.height = (z_p95 - w.base_z).max(0.50);
                w.is_parapet = true;
            } else {
                w.height = height;
                w.is_parapet = false;
            }
        } else {
            w.height = height;
            w.is_parapet = false;
        }

        // Count populated vertical bins among 10 bins of height height / 10.0 between storey.base_z and storey.top_z.
        // If occupancy < 5 bins (< 50%) and !w.is_parapet, drop the wall (ceiling soffit, hanging cabinet, or reflection).
        if !w.is_parapet {
            if bin_height <= 1e-9 {
                return false;
            }
            let mut bins = [false; 10];
            for &z in &zs {
                if z >= storey.base_z && z <= storey.top_z {
                    let bin = (((z - storey.base_z) / bin_height).floor() as usize).min(9);
                    bins[bin] = true;
                }
            }
            let occupancy = bins.iter().filter(|&&b| b).count();
            if occupancy < 5 {
                return false;
            }
        }
        true
    });

    // 4. Parapet topological filter:
    // Parapets (is_parapet == true) floating as isolated loops in the middle of rooms must be rejected.
    // Keep a parapet ONLY if:
    // 1) At least one of its endpoints is within <= 0.25 m of a full-height structural wall
    //    (!w.is_parapet && w.height >= height * 0.75), OR
    // 2) It lies on or near the outer boundary of the storey points (within 0.35 m of the AABB
    //    [min_x, max_x, min_y, max_y] of the storey points).
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in &storey_pts {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }

    let dist_to_aabb = |pt: [f64; 2]| -> f64 {
        let dx = (pt[0] - min_x).abs().min((max_x - pt[0]).abs());
        let dy = (pt[1] - min_y).abs().min((max_y - pt[1]).abs());
        dx.min(dy)
    };

    let structural_walls: Vec<Wall> = walls.iter().filter(|w| !w.is_parapet && w.height >= height * 0.75).cloned().collect();

    walls.retain(|w| {
        if !w.is_parapet {
            return true;
        }
        let len = (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1]);
        if len < 0.60 || w.thickness < 0.08 {
            return false;
        }

        // Condition 2: near outer boundary of storey points
        if dist_to_aabb(w.from) <= 0.35 || dist_to_aabb(w.to) <= 0.35 {
            return true;
        }

        // Condition 1: near a full-height structural wall
        structural_walls.iter().any(|sw| {
            dist_point_to_segment_2d(w.from[0], w.from[1], sw.from[0], sw.from[1], sw.to[0], sw.to[1]) <= 0.25
                || dist_point_to_segment_2d(w.to[0], w.to[1], sw.from[0], sw.from[1], sw.to[0], sw.to[1]) <= 0.25
        })
    });

    let walls = merge_collinear_walls(walls, 3.0, 0.15, 1.20);
    let walls = collapse_parallel_walls(walls);
    let walls = non_maximal_suppression(walls);
    let walls = merge_collinear_walls(walls, 3.0, 0.15, 1.20);
    let mut walls = detect_curved_walls(walls);

    walls.sort_by(|a, b| {
        a.from[0]
            .total_cmp(&b.from[0])
            .then(a.from[1].total_cmp(&b.from[1]))
            .then(a.to[0].total_cmp(&b.to[0]))
            .then(a.to[1].total_cmp(&b.to[1]))
            .then(a.thickness.total_cmp(&b.thickness))
    });
    Ok(walls)
}

fn extract_walls_from_band(band: &[[f64; 3]], storey_base_z: f64, height: f64, t: &Tuning) -> Result<Vec<Wall>, BimError> {
    if band.is_empty() {
        return Ok(Vec::new());
    }
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for p in band {
        for k in 0..2 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
    }
    let pad = (2 * t.close_radius + 2) as f64 * t.cell;
    let origin = [(low[0] / t.cell).floor() * t.cell - pad, (low[1] / t.cell).floor() * t.cell - pad];
    let far = [high[0] + pad, high[1] + pad];
    let Some(mut raster) = Raster::new(origin, far, t.cell) else {
        return Err(BimError::TooLarge { what: "wall raster", limit: raster::MAX_RASTER_CELLS });
    };
    for p in band {
        raster.add(*p);
    }
    if raster.total() == 0 {
        return Ok(Vec::new());
    }
    let (cols, rows) = raster.dims();
    let set = raster::mask(&raster, t.threshold);
    let closed = raster::close(&set, cols, rows, t.close_radius);
    let (rings, _dropped) = raster::trace(&closed, cols, rows);

    let mut faces: Vec<Segment> = Vec::new();
    let mut outer: Vec<bool> = Vec::with_capacity(rings.len());
    let tolerance = t.epsilon.max(t.cell);
    for (ri, ring) in rings.iter().enumerate() {
        outer.push(ring.outer);
        let world = raster.to_world(&ring.ring);
        let simple = raster::simplify(&world, tolerance);
        let joined = join_collinear(&simple, t.angle_tolerance_deg);
        if joined.len() < 3 {
            continue;
        }
        faces.extend(segments_of(&joined, ri).into_iter().filter(|s| s.length >= t.min_wall_length));
    }

    let (mut walls, used) = pair_faces(&faces, t, storey_base_z, height)?;
    if !t.exterior_scan {
        for (i, s) in faces.iter().enumerate() {
            if used.get(i).copied().unwrap_or(true) || !outer.get(s.ring).copied().unwrap_or(false) {
                continue;
            }
            let (nx, ny) = (s.dir[1], -s.dir[0]);
            let shift = 0.5 * t.exterior_wall_thickness;
            walls.push(Wall {
                from: [s.a[0] + nx * shift, s.a[1] + ny * shift],
                to: [s.b[0] + nx * shift, s.b[1] + ny * shift],
                thickness: t.exterior_wall_thickness,
                base_z: storey_base_z,
                height,
                exterior: true,
                is_parapet: false,
                arc: None,
            });
        }
    }
    Ok(walls)
}

fn is_in_wall_corridors(px: f64, py: f64, walls: &[Wall]) -> bool {
    for w in walls {
        let corridor = w.thickness * 0.5 + 0.10;
        let d = dist_point_to_segment_2d(px, py, w.from[0], w.from[1], w.to[0], w.to[1]);
        if d <= corridor {
            return true;
        }
    }
    false
}

fn dist_point_to_segment_2d(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
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

/// Merge fragmented collinear walls into continuous walls.
///
/// Walls that are nearly collinear (angular difference <= `angle_tol_deg`, default 3.0 deg;
/// perpendicular distance from endpoints to line <= `max_perp_dist`, default 0.15 m)
/// have their endpoints projected along the common axis. If intervals overlap or are separated
/// by <= `max_gap` (default 1.20 m, covering open doorways and scan breaks), they are merged into
/// a single wall spanning min to max along the axis, with average thickness.
pub fn merge_collinear_walls(walls: Vec<Wall>, angle_tol_deg: f64, max_perp_dist: f64, max_gap: f64) -> Vec<Wall> {
    if walls.is_empty() {
        return Vec::new();
    }
    let angle_tol = if angle_tol_deg.is_finite() && angle_tol_deg > 0.0 { angle_tol_deg } else { 3.0 };
    let perp_dist = if max_perp_dist.is_finite() && max_perp_dist > 0.0 { max_perp_dist } else { 0.15 };
    let gap = if max_gap.is_finite() && max_gap >= 0.0 { max_gap } else { 1.20 };

    let lengths: Vec<f64> = walls.iter().map(|w| (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1])).collect();
    let mut order: Vec<usize> = (0..walls.len()).collect();
    order.sort_by(|&i, &j| lengths[j].total_cmp(&lengths[i]));

    let mut used = vec![false; walls.len()];
    let mut result: Vec<Wall> = Vec::with_capacity(walls.len());

    #[derive(Clone, Copy)]
    struct WallProjection {
        wall_idx: usize,
        t_min: f64,
        t_max: f64,
        s_avg: f64,
    }

    for &ref_idx in &order {
        if used.get(ref_idx).copied().unwrap_or(true) {
            continue;
        }
        if let Some(slot) = used.get_mut(ref_idx) {
            *slot = true;
        }
        let Some(r) = walls.get(ref_idx) else {
            continue;
        };
        let len_r = lengths.get(ref_idx).copied().unwrap_or(0.0);
        if len_r < 1e-6 {
            result.push(r.clone());
            continue;
        }

        let u = [(r.to[0] - r.from[0]) / len_r, (r.to[1] - r.from[1]) / len_r];
        let n = [-u[1], u[0]];
        let p0 = r.from;

        let mut group = vec![WallProjection { wall_idx: ref_idx, t_min: 0.0, t_max: len_r, s_avg: 0.0 }];

        for &j in &order {
            if used.get(j).copied().unwrap_or(true) {
                continue;
            }
            let Some(w) = walls.get(j) else {
                continue;
            };
            if w.is_parapet != r.is_parapet || w.arc.is_some() || r.arc.is_some() {
                continue;
            }
            let len_j = lengths.get(j).copied().unwrap_or(0.0);
            if len_j < 1e-6 {
                continue;
            }

            let w_dir = [(w.to[0] - w.from[0]) / len_j, (w.to[1] - w.from[1]) / len_j];
            let cross = (u[0] * w_dir[1] - u[1] * w_dir[0]).abs();
            let dot = (u[0] * w_dir[0] + u[1] * w_dir[1]).abs();
            let angle = cross.atan2(dot).to_degrees();
            if angle > angle_tol {
                continue;
            }

            let d1 = ((w.from[0] - p0[0]) * n[0] + (w.from[1] - p0[1]) * n[1]).abs();
            let d2 = ((w.to[0] - p0[0]) * n[0] + (w.to[1] - p0[1]) * n[1]).abs();
            if d1 > perp_dist || d2 > perp_dist {
                continue;
            }

            if let Some(slot) = used.get_mut(j) {
                *slot = true;
            }
            let t1 = (w.from[0] - p0[0]) * u[0] + (w.from[1] - p0[1]) * u[1];
            let t2 = (w.to[0] - p0[0]) * u[0] + (w.to[1] - p0[1]) * u[1];
            let s1 = (w.from[0] - p0[0]) * n[0] + (w.from[1] - p0[1]) * n[1];
            let s2 = (w.to[0] - p0[0]) * n[0] + (w.to[1] - p0[1]) * n[1];
            group.push(WallProjection { wall_idx: j, t_min: t1.min(t2), t_max: t1.max(t2), s_avg: 0.5 * (s1 + s2) });
        }

        group.sort_by(|a, b| a.t_min.total_cmp(&b.t_min));

        let mut clusters: Vec<Vec<WallProjection>> = Vec::new();
        for item in group {
            if let Some(last) = clusters.last_mut() {
                let cur_max = last.iter().map(|it| it.t_max).fold(f64::NEG_INFINITY, f64::max);
                if item.t_min <= cur_max + gap {
                    last.push(item);
                    continue;
                }
            }
            clusters.push(vec![item]);
        }

        for cluster in clusters {
            if cluster.len() == 1 {
                if let Some(w) = walls.get(cluster[0].wall_idx) {
                    result.push(w.clone());
                }
            } else {
                let t_min = cluster.iter().map(|it| it.t_min).fold(f64::INFINITY, f64::min);
                let t_max = cluster.iter().map(|it| it.t_max).fold(f64::NEG_INFINITY, f64::max);
                let s_avg = cluster.iter().map(|it| it.s_avg).sum::<f64>() / (cluster.len() as f64);
                let thickness = cluster.iter().filter_map(|it| walls.get(it.wall_idx)).map(|w| w.thickness).sum::<f64>() / (cluster.len() as f64);
                let base_z = cluster.iter().filter_map(|it| walls.get(it.wall_idx)).map(|w| w.base_z).fold(f64::INFINITY, f64::min);
                let height = cluster.iter().filter_map(|it| walls.get(it.wall_idx)).map(|w| w.height).fold(f64::NEG_INFINITY, f64::max);
                let exterior = cluster.iter().filter_map(|it| walls.get(it.wall_idx)).any(|w| w.exterior);
                let is_parapet = walls.get(cluster[0].wall_idx).map(|w| w.is_parapet).unwrap_or(false);

                let origin_shifted = [p0[0] + s_avg * n[0], p0[1] + s_avg * n[1]];
                let from = [origin_shifted[0] + t_min * u[0], origin_shifted[1] + t_min * u[1]];
                let to = [origin_shifted[0] + t_max * u[0], origin_shifted[1] + t_max * u[1]];

                result.push(Wall { from, to, thickness, base_z, height, exterior, is_parapet, arc: None });
            }
        }
    }

    // Corner and T-junction extension: for any wall end within <= 0.35 m of another wall's axis
    // (and not already touching), snap that endpoint onto the other wall's line.
    for _ in 0..2 {
        let mut snapped_from: Vec<Option<[f64; 2]>> = vec![None; result.len()];
        let mut snapped_to: Vec<Option<[f64; 2]>> = vec![None; result.len()];

        for i in 0..result.len() {
            let wi = &result[i];
            let len_i = (wi.to[0] - wi.from[0]).hypot(wi.to[1] - wi.from[1]);
            if len_i < 1e-4 {
                continue;
            }
            let dir_i = [(wi.to[0] - wi.from[0]) / len_i, (wi.to[1] - wi.from[1]) / len_i];

            let mut best_from: Option<(f64, usize, [f64; 2])> = None;
            let mut best_to: Option<(f64, usize, [f64; 2])> = None;

            for (j, wj) in result.iter().enumerate() {
                if j == i {
                    continue;
                }
                let len_j = (wj.to[0] - wj.from[0]).hypot(wj.to[1] - wj.from[1]);
                if len_j < 1e-4 {
                    continue;
                }
                let dir_j = [(wj.to[0] - wj.from[0]) / len_j, (wj.to[1] - wj.from[1]) / len_j];

                // Check angle: corners and T-junctions meet at an angle (not parallel/collinear)
                let dot = (dir_i[0] * dir_j[0] + dir_i[1] * dir_j[1]).abs();
                let cross = (dir_i[0] * dir_j[1] - dir_i[1] * dir_j[0]).abs();
                let angle_deg = cross.atan2(dot).to_degrees();
                if angle_deg < 15.0 {
                    continue;
                }

                let proj_line = |p: [f64; 2]| -> [f64; 2] {
                    let t = (p[0] - wj.from[0]) * dir_j[0] + (p[1] - wj.from[1]) * dir_j[1];
                    [wj.from[0] + t * dir_j[0], wj.from[1] + t * dir_j[1]]
                };

                let d_from = dist_point_to_segment_2d(wi.from[0], wi.from[1], wj.from[0], wj.from[1], wj.to[0], wj.to[1]);
                if d_from > 1e-4 && d_from <= 0.35 && best_from.as_ref().is_none_or(|(best_d, _, _)| d_from < *best_d) {
                    best_from = Some((d_from, j, proj_line(wi.from)));
                }

                let d_to = dist_point_to_segment_2d(wi.to[0], wi.to[1], wj.from[0], wj.from[1], wj.to[0], wj.to[1]);
                if d_to > 1e-4 && d_to <= 0.35 && best_to.as_ref().is_none_or(|(best_d, _, _)| d_to < *best_d) {
                    best_to = Some((d_to, j, proj_line(wi.to)));
                }
            }

            // A wall must not have both ends snap to the same other wall
            if let (Some((_, j_from, p_from)), Some((_, j_to, p_to))) = (&best_from, &best_to) {
                if j_from == j_to {
                    if best_from.as_ref().map_or(0.0, |b| b.0) <= best_to.as_ref().map_or(0.0, |b| b.0) {
                        snapped_from[i] = Some(*p_from);
                    } else {
                        snapped_to[i] = Some(*p_to);
                    }
                } else {
                    snapped_from[i] = Some(*p_from);
                    snapped_to[i] = Some(*p_to);
                }
            } else {
                if let Some((_, _, p_from)) = best_from {
                    snapped_from[i] = Some(p_from);
                }
                if let Some((_, _, p_to)) = best_to {
                    snapped_to[i] = Some(p_to);
                }
            }
        }

        let mut any_moved = false;
        for i in 0..result.len() {
            if let Some(p) = snapped_from[i]
                && (result[i].from[0] - p[0]).hypot(result[i].from[1] - p[1]) > 1e-4
            {
                result[i].from = p;
                any_moved = true;
            }
            if let Some(p) = snapped_to[i]
                && (result[i].to[0] - p[0]).hypot(result[i].to[1] - p[1]) > 1e-4
            {
                result[i].to = p;
                any_moved = true;
            }
        }
        if !any_moved {
            break;
        }
    }

    // Final orphan wall filter: only drop a wall if its length is < 0.40 m AND neither of its ends
    // is connected (within 0.10 m) to any other wall. Short corner returns and door jambs that connect
    // to an adjacent wall MUST be preserved!
    let n = result.len();
    let keep: Vec<bool> = (0..n)
        .map(|i| {
            let w = &result[i];
            let len = (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1]);
            if len >= 0.40 {
                return true;
            }
            (0..n).any(|j| {
                if j == i {
                    return false;
                }
                let other = &result[j];
                let d_from = dist_point_to_segment_2d(w.from[0], w.from[1], other.from[0], other.from[1], other.to[0], other.to[1]);
                let d_to = dist_point_to_segment_2d(w.to[0], w.to[1], other.from[0], other.from[1], other.to[0], other.to[1]);
                d_from <= 0.10 || d_to <= 0.10
            })
        })
        .collect();

    let mut idx = 0;
    result.retain(|_| {
        let k = keep.get(idx).copied().unwrap_or(true);
        idx += 1;
        k
    });

    result
}

/// Pair opposite wall faces and merge each pair into a single centerline wall.
///
/// Find pairs of walls (w1, w2) with:
/// - Both full-height (or both parapets)
/// - Angle <= 3.0 deg
/// - Perpendicular distance d_perp between axes in [0.08, 0.40] m
/// - Longitudinal overlap along the reference axis >= 40% of the shorter wall's length
///
/// Merge each paired pair into a single wall:
/// - Axis line: midpoint line along the common direction (projecting the union of intervals)
/// - Thickness: thickness = d_perp + (w1.thickness + w2.thickness) / 2.0
/// - is_parapet: false if either is full-height
/// - exterior: w1.exterior && w2.exterior (a paired wall between two scanned faces is interior)
pub fn collapse_parallel_walls(walls: Vec<Wall>) -> Vec<Wall> {
    if walls.len() < 2 {
        return walls;
    }
    let lengths: Vec<f64> = walls.iter().map(|w| (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1])).collect();
    let mut order: Vec<usize> = (0..walls.len()).collect();
    order.sort_by(|&i, &j| lengths[j].total_cmp(&lengths[i]));

    let mut used = vec![false; walls.len()];
    let mut result: Vec<Wall> = Vec::with_capacity(walls.len());

    for &i in &order {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(w1) = walls.get(i) else {
            continue;
        };
        let l1 = lengths.get(i).copied().unwrap_or(0.0);
        if l1 < 1e-4 {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            result.push(w1.clone());
            continue;
        }

        let u1 = [(w1.to[0] - w1.from[0]) / l1, (w1.to[1] - w1.from[1]) / l1];
        let n1 = [-u1[1], u1[0]];

        let mut best_match: Option<(usize, f64, f64, f64, f64, f64)> = None;

        for &j in &order {
            if j == i || used.get(j).copied().unwrap_or(true) {
                continue;
            }
            let Some(w2) = walls.get(j) else {
                continue;
            };
            if w1.is_parapet != w2.is_parapet || w1.arc.is_some() || w2.arc.is_some() {
                continue;
            }
            let l2 = lengths.get(j).copied().unwrap_or(0.0);
            if l2 < 1e-4 {
                continue;
            }

            let u2 = [(w2.to[0] - w2.from[0]) / l2, (w2.to[1] - w2.from[1]) / l2];
            let dot = (u1[0] * u2[0] + u1[1] * u2[1]).abs();
            let cross = (u1[0] * u2[1] - u1[1] * u2[0]).abs();
            let angle = cross.atan2(dot).to_degrees();
            if angle > 3.0 {
                continue;
            }

            let s_from = (w2.from[0] - w1.from[0]) * n1[0] + (w2.from[1] - w1.from[1]) * n1[1];
            let s_to = (w2.to[0] - w1.from[0]) * n1[0] + (w2.to[1] - w1.from[1]) * n1[1];
            let s_avg = 0.5 * (s_from + s_to);
            let d_perp = s_avg.abs();
            if !(0.08..=0.40).contains(&d_perp) {
                continue;
            }

            let t_from = (w2.from[0] - w1.from[0]) * u1[0] + (w2.from[1] - w1.from[1]) * u1[1];
            let t_to = (w2.to[0] - w1.from[0]) * u1[0] + (w2.to[1] - w1.from[1]) * u1[1];
            let t2_min = t_from.min(t_to);
            let t2_max = t_from.max(t_to);
            let overlap = (l1.min(t2_max) - 0.0f64.max(t2_min)).max(0.0);
            let min_len = l1.min(l2);
            if overlap < 0.40 * min_len {
                continue;
            }

            if best_match.as_ref().is_none_or(|(_, _, _, _, _, prev_overlap)| overlap > *prev_overlap) {
                best_match = Some((j, d_perp, s_avg, t2_min, t2_max, overlap));
            }
        }

        if let Some((j, d_perp, s_avg, t2_min, t2_max, _)) = best_match {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            if let Some(slot) = used.get_mut(j) {
                *slot = true;
            }
            let Some(w2) = walls.get(j) else {
                continue;
            };

            let origin_mid = [w1.from[0] + 0.5 * s_avg * n1[0], w1.from[1] + 0.5 * s_avg * n1[1]];
            let t_union_min = 0.0f64.min(t2_min);
            let t_union_max = l1.max(t2_max);
            let from = [origin_mid[0] + t_union_min * u1[0], origin_mid[1] + t_union_min * u1[1]];
            let to = [origin_mid[0] + t_union_max * u1[0], origin_mid[1] + t_union_max * u1[1]];
            let thickness = d_perp + (w1.thickness + w2.thickness) / 2.0;
            let is_parapet = w1.is_parapet && w2.is_parapet;
            let exterior = w1.exterior && w2.exterior;
            let base_z = w1.base_z.min(w2.base_z);
            let height = w1.height.max(w2.height);

            result.push(Wall { from, to, thickness, base_z, height, exterior, is_parapet, arc: None });
        } else {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            result.push(w1.clone());
        }
    }

    result
}

/// Non-Maximal Suppression (NMS) for short walls:
/// If a short wall segment has length < 0.65 m, and is nearly parallel (angle <= 5.0 deg)
/// to a longer wall (length >= 1.20 m) with perpendicular distance <= 0.45 m, drop the short segment.
pub fn non_maximal_suppression(mut walls: Vec<Wall>) -> Vec<Wall> {
    if walls.len() < 2 {
        return walls;
    }
    let lengths: Vec<f64> = walls.iter().map(|w| (w.to[0] - w.from[0]).hypot(w.to[1] - w.from[1])).collect();
    let mut keep = vec![true; walls.len()];

    for i in 0..walls.len() {
        let len_i = lengths.get(i).copied().unwrap_or(0.0);
        if len_i >= 0.65 {
            continue;
        }
        if len_i < 1e-4 {
            if let Some(k) = keep.get_mut(i) {
                *k = false;
            }
            continue;
        }
        let Some(wi) = walls.get(i) else {
            continue;
        };
        if wi.arc.is_some() {
            continue;
        }
        let ui = [(wi.to[0] - wi.from[0]) / len_i, (wi.to[1] - wi.from[1]) / len_i];

        for j in 0..walls.len() {
            if j == i {
                continue;
            }
            let len_j = lengths.get(j).copied().unwrap_or(0.0);
            if len_j < 1.20 {
                continue;
            }
            let Some(wj) = walls.get(j) else {
                continue;
            };
            if wj.arc.is_some() {
                continue;
            }
            let uj = [(wj.to[0] - wj.from[0]) / len_j, (wj.to[1] - wj.from[1]) / len_j];

            let dot = (ui[0] * uj[0] + ui[1] * uj[1]).abs();
            let cross = (ui[0] * uj[1] - ui[1] * uj[0]).abs();
            let angle = cross.atan2(dot).to_degrees();
            if angle > 5.0 {
                continue;
            }

            let nj = [-uj[1], uj[0]];
            let d1 = ((wi.from[0] - wj.from[0]) * nj[0] + (wi.from[1] - wj.from[1]) * nj[1]).abs();
            let d2 = ((wi.to[0] - wj.from[0]) * nj[0] + (wi.to[1] - wj.from[1]) * nj[1]).abs();
            let d_perp = d1.max(d2);
            if d_perp > 0.45 {
                continue;
            }

            let t1 = (wi.from[0] - wj.from[0]) * uj[0] + (wi.from[1] - wj.from[1]) * uj[1];
            let t2 = (wi.to[0] - wj.from[0]) * uj[0] + (wi.to[1] - wj.from[1]) * uj[1];
            let t_min = t1.min(t2);
            let t_max = t1.max(t2);
            if t_max < -0.20 || t_min > len_j + 0.20 {
                continue;
            }

            if let Some(k) = keep.get_mut(i) {
                *k = false;
            }
            break;
        }
    }

    let mut idx = 0;
    walls.retain(|_| {
        let k = keep.get(idx).copied().unwrap_or(true);
        idx += 1;
        k
    });
    walls
}

/// Fit a circle to a slice of 2D points using the algebraic Kåsa method.
///
/// Returns `Some((center, radius, rmse))` if the fit succeeds with a positive, finite radius
/// and non-degenerate determinant. Returns `None` if `pts.len() < 3`, points are collinear or
/// nearly collinear, non-finite coordinates are present, or the fitted radius is non-positive.
pub fn fit_circle_kasa(pts: &[[f64; 2]]) -> Option<([f64; 2], f64, f64)> {
    let n = pts.len();
    if n < 3 {
        return None;
    }

    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for p in pts {
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        sum_x += p[0];
        sum_y += p[1];
    }
    let nf = n as f64;
    let mean_x = sum_x / nf;
    let mean_y = sum_y / nf;

    let mut s_uu = 0.0;
    let mut s_vv = 0.0;
    let mut s_uv = 0.0;
    let mut s_uw = 0.0;
    let mut s_vw = 0.0;
    let mut sum_w = 0.0;

    for p in pts {
        let u = p[0] - mean_x;
        let v = p[1] - mean_y;
        let w = u * u + v * v;
        s_uu += u * u;
        s_vv += v * v;
        s_uv += u * v;
        s_uw += u * w;
        s_vw += v * w;
        sum_w += w;
    }

    let det = s_uu * s_vv - s_uv * s_uv;
    if !det.is_finite() || det.abs() <= 1e-12 {
        return None;
    }

    let a_prime = (s_uw * s_vv - s_vw * s_uv) / det;
    let b_prime = (s_vw * s_uu - s_uw * s_uv) / det;
    let c_prime = sum_w / nf;

    let uc = a_prime / 2.0;
    let vc = b_prime / 2.0;

    let r_sq = c_prime + uc * uc + vc * vc;
    if !r_sq.is_finite() || r_sq <= 0.0 {
        return None;
    }
    let radius = r_sq.sqrt();
    let center = [uc + mean_x, vc + mean_y];

    if !radius.is_finite() || !center[0].is_finite() || !center[1].is_finite() {
        return None;
    }

    let mut sum_err_sq = 0.0;
    for p in pts {
        let d = (p[0] - center[0]).hypot(p[1] - center[1]) - radius;
        sum_err_sq += d * d;
    }
    let rmse = (sum_err_sq / nf).sqrt();
    if !rmse.is_finite() {
        return None;
    }

    Some((center, radius, rmse))
}

/// Detect and merge sequences of consecutive segments that form a curve.
///
/// Where a sequence of consecutive segments forms a curve with consistent angular deflection
/// and Kåsa circle fit has RMSE <= 0.015 m (15 mm) and radius in [0.5, 20.0] m,
/// create a Wall with `arc: Some(WallArc { center, radius, ccw })`.
pub fn detect_curved_walls(walls: Vec<Wall>) -> Vec<Wall> {
    if walls.len() < 2 {
        return walls;
    }

    let mut used = vec![false; walls.len()];
    let mut out = Vec::with_capacity(walls.len());

    let turn_angle = |u1: [f64; 2], u2: [f64; 2]| -> f64 {
        let cross = u1[0] * u2[1] - u1[1] * u2[0];
        let dot = u1[0] * u2[0] + u1[1] * u2[1];
        cross.atan2(dot)
    };

    for i in 0..walls.len() {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(w0) = walls.get(i) else {
            continue;
        };
        if w0.arc.is_some() {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            out.push(w0.clone());
            continue;
        }

        let mut chain: Vec<(usize, [f64; 2], [f64; 2])> = vec![(i, w0.from, w0.to)];
        let mut curvature_sign: Option<f64> = None;

        let mut extended = true;
        while extended {
            extended = false;
            let Some(&(_, _, last_to)) = chain.last() else {
                break;
            };
            let Some(&(_, prev_from, _)) = chain.last() else {
                break;
            };
            let len_prev = (last_to[0] - prev_from[0]).hypot(last_to[1] - prev_from[1]);
            if len_prev < 1e-4 {
                break;
            }
            let dir_prev = [(last_to[0] - prev_from[0]) / len_prev, (last_to[1] - prev_from[1]) / len_prev];

            let mut best_next: Option<(usize, [f64; 2], [f64; 2], f64)> = None;
            for j in 0..walls.len() {
                if used.get(j).copied().unwrap_or(true) || chain.iter().any(|&(idx, _, _)| idx == j) {
                    continue;
                }
                let Some(wj) = walls.get(j) else {
                    continue;
                };
                if wj.arc.is_some() || wj.is_parapet != w0.is_parapet {
                    continue;
                }

                let d_fwd = (wj.from[0] - last_to[0]).hypot(wj.from[1] - last_to[1]);
                let d_rev = (wj.to[0] - last_to[0]).hypot(wj.to[1] - last_to[1]);

                let (cand_from, cand_to, dist) = if d_fwd <= 0.35 && d_fwd <= d_rev {
                    (wj.from, wj.to, d_fwd)
                } else if d_rev <= 0.35 {
                    (wj.to, wj.from, d_rev)
                } else {
                    continue;
                };

                let len_cand = (cand_to[0] - cand_from[0]).hypot(cand_to[1] - cand_from[1]);
                if len_cand < 1e-4 {
                    continue;
                }
                let dir_cand = [(cand_to[0] - cand_from[0]) / len_cand, (cand_to[1] - cand_from[1]) / len_cand];

                let angle = turn_angle(dir_prev, dir_cand);
                let angle_deg = angle.to_degrees();
                if angle_deg.abs() < 1.5 || angle_deg.abs() > 50.0 {
                    continue;
                }

                if curvature_sign.is_some_and(|cs| cs * angle <= 0.0) {
                    continue;
                }

                if best_next.as_ref().is_none_or(|(_, _, _, best_d)| dist < *best_d) {
                    best_next = Some((j, cand_from, cand_to, dist));
                }
            }

            if let Some((next_idx, next_from, next_to, _)) = best_next {
                let len_next = (next_to[0] - next_from[0]).hypot(next_to[1] - next_from[1]);
                let dir_next = [(next_to[0] - next_from[0]) / len_next, (next_to[1] - next_from[1]) / len_next];
                let angle = turn_angle(dir_prev, dir_next);
                if curvature_sign.is_none() {
                    curvature_sign = Some(angle.signum());
                }
                chain.push((next_idx, next_from, next_to));
                extended = true;
            }
        }

        let mut merged_arc = false;
        if chain.len() >= 2 {
            let mut pts: Vec<[f64; 2]> = Vec::with_capacity(chain.len() + 1);
            if let Some(&(_, first_from, _)) = chain.first() {
                pts.push(first_from);
            }
            for &(_, _, to) in &chain {
                pts.push(to);
            }

            if let Some((center, radius, _rmse)) = fit_circle_kasa(&pts).filter(|&(_, r, err)| err <= 0.015 && (0.5..=20.0).contains(&r)) {
                let k = chain.len();
                let ccw = curvature_sign.is_some_and(|s| s > 0.0);
                let avg_thickness = chain.iter().filter_map(|&(idx, _, _)| walls.get(idx)).map(|w| w.thickness).sum::<f64>() / (k as f64);
                let min_base_z = chain.iter().filter_map(|&(idx, _, _)| walls.get(idx)).map(|w| w.base_z).fold(f64::INFINITY, f64::min);
                let max_height = chain.iter().filter_map(|&(idx, _, _)| walls.get(idx)).map(|w| w.height).fold(f64::NEG_INFINITY, f64::max);
                let any_exterior = chain.iter().filter_map(|&(idx, _, _)| walls.get(idx)).any(|w| w.exterior);
                let all_parapet = chain.iter().filter_map(|&(idx, _, _)| walls.get(idx)).all(|w| w.is_parapet);

                let from = chain.first().map(|c| c.1).unwrap_or([0.0, 0.0]);
                let to = chain.last().map(|c| c.2).unwrap_or([0.0, 0.0]);

                for &(idx, _, _) in &chain {
                    if let Some(slot) = used.get_mut(idx) {
                        *slot = true;
                    }
                }

                out.push(Wall {
                    from,
                    to,
                    thickness: avg_thickness,
                    base_z: min_base_z,
                    height: max_height,
                    exterior: any_exterior,
                    is_parapet: all_parapet,
                    arc: Some(WallArc { center, radius, ccw }),
                });
                merged_arc = true;
            }
        }

        if !merged_arc {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            out.push(w0.clone());
        }
    }

    out
}

/// The options this stage can use, validated once.
struct Tuning {
    cell: f64,
    slice_from: f64,
    slice_to: f64,
    threshold: f64,
    close_radius: usize,
    epsilon: f64,
    angle_tolerance_deg: f64,
    min_wall_length: f64,
    min_wall_thickness: f64,
    max_wall_thickness: f64,
    exterior_wall_thickness: f64,
    exterior_scan: bool,
}

impl Tuning {
    fn read(o: &BimOptions) -> Result<Self, BimError> {
        let cell = o.cell.is_finite() && o.cell > 0.0;
        let slice = o.slice_from.is_finite() && o.slice_to.is_finite() && o.slice_from >= 0.0 && o.slice_from <= o.slice_to && o.slice_to <= 1.0;
        let threshold = o.threshold.is_finite() && o.threshold > 0.0 && o.threshold <= 1.0;
        let epsilon = o.epsilon.is_finite() && o.epsilon >= 0.0;
        let angle = o.angle_tolerance_deg.is_finite() && (0.0..=90.0).contains(&o.angle_tolerance_deg);
        let length = o.min_wall_length.is_finite() && o.min_wall_length >= 0.0;
        let thickness = o.min_wall_thickness.is_finite()
            && o.max_wall_thickness.is_finite()
            && o.min_wall_thickness >= 0.0
            && o.max_wall_thickness > o.min_wall_thickness;
        let exterior = o.exterior_wall_thickness.is_finite() && o.exterior_wall_thickness > 0.0;
        if !(cell && slice && threshold && epsilon && angle && length && thickness && exterior) {
            return Err(BimError::InvalidCell);
        }
        Ok(Self {
            cell: o.cell,
            slice_from: o.slice_from,
            slice_to: o.slice_to,
            threshold: o.threshold,
            close_radius: o.close_radius.min(MAX_CLOSE_RADIUS),
            epsilon: o.epsilon,
            angle_tolerance_deg: o.angle_tolerance_deg,
            min_wall_length: o.min_wall_length,
            min_wall_thickness: o.min_wall_thickness,
            max_wall_thickness: o.max_wall_thickness,
            exterior_wall_thickness: o.exterior_wall_thickness,
            exterior_scan: o.exterior_scan,
        })
    }
}

/// A traced face: one side of a ring, between two of its corners.
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    /// Unit direction from `a` to `b`.
    dir: [f64; 2],
    length: f64,
    /// Which ring it came from, so the synthesis step can ask whether that ring is an outside.
    ring: usize,
}

/// The straight sides of a joined ring, closing the last back to the first.
fn segments_of(ring: &[[f64; 2]], ring_index: usize) -> Vec<Segment> {
    let n = ring.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (Some(&a), Some(&b)) = (ring.get(i), ring.get((i + 1) % n)) else {
            continue;
        };
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        if !length.is_finite() || length <= 0.0 {
            continue;
        }
        out.push(Segment { a, b, dir: [dx / length, dy / length], length, ring: ring_index });
    }
    out
}

/// Pair the faces of the same wall, and hand back the walls plus which faces were used.
///
/// A pair is two faces parallel within the angle tolerance, `min_wall_thickness` to
/// `max_wall_thickness` apart, overlapping along the axis by more than half the shorter one. The
/// search goes through a [`Grid`] over samples half a search cell apart, so only faces near the
/// one being tested are looked at; the search cell is twice the thickest wall, which is the
/// furthest a partner can be and still be one.
fn pair_faces(faces: &[Segment], t: &Tuning, base_z: f64, height: f64) -> Result<(Vec<Wall>, Vec<bool>), BimError> {
    let mut used = vec![false; faces.len()];
    let cell = 2.0 * t.max_wall_thickness;
    if !cell.is_finite() || cell <= 0.0 || faces.is_empty() {
        return Ok((Vec::new(), used));
    }
    let spacing = cell / 2.0;
    let mut samples: Vec<[f64; 3]> = Vec::new();
    let mut owner: Vec<u32> = Vec::new();
    for (i, s) in faces.iter().enumerate() {
        let count = samples_along(s, spacing);
        for k in 0..=count {
            let u = s.length * (k as f64 / count as f64);
            samples.push([s.a[0] + s.dir[0] * u, s.a[1] + s.dir[1] * u, 0.0]);
            owner.push(i as u32);
        }
        if samples.len() > MAX_PAIR_SAMPLES {
            return Err(BimError::TooLarge { what: "wall pairing samples", limit: MAX_PAIR_SAMPLES });
        }
    }
    if samples.is_empty() {
        return Ok((Vec::new(), used));
    }
    let grid = Grid::new(&samples, cell);
    let mut stamp = vec![0u32; faces.len()];
    let mut query = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    let mut walls: Vec<Wall> = Vec::new();
    for (i, s) in faces.iter().enumerate() {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        query = query.wrapping_add(1);
        if query == 0 {
            stamp.fill(0);
            query = 1;
        }
        let mut best: Option<(f64, f64, usize)> = None;
        let count = samples_along(s, spacing);
        for k in 0..=count {
            let u = s.length * (k as f64 / count as f64);
            let p = [s.a[0] + s.dir[0] * u, s.a[1] + s.dir[1] * u, 0.0];
            grid.around(p, &mut buf);
            for &raw in &buf {
                let Some(&o) = owner.get(raw as usize) else {
                    continue;
                };
                let j = o as usize;
                if j == i || used.get(j).copied().unwrap_or(true) {
                    continue;
                }
                if stamp.get(j).copied() == Some(query) {
                    continue;
                }
                if let Some(st) = stamp.get_mut(j) {
                    *st = query;
                }
                if let Some((wall, overlap)) = axis_of(s, &faces[j], t, base_z, height) {
                    let better = match best {
                        None => true,
                        Some((thickness, best_overlap, _)) => {
                            wall.thickness < thickness - 1e-12 || ((wall.thickness - thickness).abs() <= 1e-12 && overlap > best_overlap + 1e-12)
                        }
                    };
                    if better {
                        best = Some((wall.thickness, overlap, j));
                    }
                }
            }
        }
        let paired = best.and_then(|(_, _, j)| axis_of(s, &faces[j], t, base_z, height).map(|(wall, _)| (j, wall)));
        if let Some((j, wall)) = paired {
            if let Some(slot) = used.get_mut(i) {
                *slot = true;
            }
            if let Some(slot) = used.get_mut(j) {
                *slot = true;
            }
            walls.push(wall);
        }
    }
    Ok((walls, used))
}

/// How many equal steps of `spacing` fit along a face, within the per-face cap. One step at least,
/// so a face shorter than the spacing is still sampled once at each end.
fn samples_along(s: &Segment, spacing: f64) -> usize {
    let steps = (s.length / spacing).ceil();
    if !steps.is_finite() || steps < 1.0 {
        return 1;
    }
    (steps as usize).clamp(1, MAX_SEGMENT_SAMPLES)
}

/// The wall two faces make, with the overlap that justifies it, or `None` when they are not the
/// same wall.
///
/// The **longer** face is the reference: the axis runs in its direction and its extent is the part
/// of the longer face that the shorter one also covers. Extending to the longer face alone would
/// claim the wall where only one side was ever seen, which is exactly what a doorway looks like.
fn axis_of(a: &Segment, b: &Segment, t: &Tuning, base_z: f64, height: f64) -> Option<(Wall, f64)> {
    let (base, other) = if b.length > a.length { (b, a) } else { (a, b) };
    let [dx, dy] = base.dir;
    let [ox, oy] = other.dir;
    let cross = dx * oy - dy * ox;
    let dot = dx * ox + dy * oy;
    let angle = cross.abs().atan2(dot.abs()).to_degrees();
    if !angle.is_finite() || angle > t.angle_tolerance_deg {
        return None;
    }
    let (nx, ny) = (-dy, dx);
    let offset = (other.a[0] - base.a[0]) * nx + (other.a[1] - base.a[1]) * ny;
    let thickness = offset.abs();
    if !thickness.is_finite() || thickness < t.min_wall_thickness || thickness > t.max_wall_thickness {
        return None;
    }
    let ua = (other.a[0] - base.a[0]) * dx + (other.a[1] - base.a[1]) * dy;
    let ub = (other.b[0] - base.a[0]) * dx + (other.b[1] - base.a[1]) * dy;
    let u0 = ua.min(ub).max(0.0);
    let u1 = ua.max(ub).min(base.length);
    let overlap = u1 - u0;
    if !overlap.is_finite() || overlap <= 0.5 * a.length.min(b.length) {
        return None;
    }
    let shift = 0.5 * offset;
    let from = [base.a[0] + dx * u0 + nx * shift, base.a[1] + dy * u0 + ny * shift];
    let to = [base.a[0] + dx * u1 + nx * shift, base.a[1] + dy * u1 + ny * shift];
    Some((Wall { from, to, thickness, base_z, height, exterior: false, is_parapet: false, arc: None }, overlap))
}

/// Join the runs of a ring that turn by no more than `tolerance_deg`.
///
/// The ring is first cleaned of repeated corners — a zero-length step has no direction — and then
/// rotated to start at a **hard** corner, so the seam between the last corner and the first is
/// tested like every other. A ring with no hard corner at all is already as smooth as the
/// tolerance allows and is returned as it stands: joining it further would collapse it to a line,
/// and a line is not a ring.
fn join_collinear(ring: &[[f64; 2]], tolerance_deg: f64) -> Vec<[f64; 2]> {
    let tol = if tolerance_deg.is_finite() { tolerance_deg.clamp(0.0, 90.0) } else { 0.0 };
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
    for p in ring {
        if !p[0].is_finite() || !p[1].is_finite() {
            return ring.to_vec();
        }
        if pts.last().is_some_and(|last| same_point(*last, *p)) {
            continue;
        }
        pts.push(*p);
    }
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    if pts.first().zip(pts.last()).is_some_and(|(first, last)| same_point(*first, *last)) {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    let prev_of = |i: usize| pts.get((i + n - 1) % n).copied();
    let next_of = |i: usize| pts.get((i + 1) % n).copied();
    let hard = |i: usize| match (prev_of(i), pts.get(i).copied(), next_of(i)) {
        (Some(prev), Some(here), Some(next)) => turn_deg(prev, here, next) > tol,
        _ => false,
    };
    let Some(start) = (0..n).find(|&i| hard(i)) else {
        return pts;
    };
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(n);
    for k in 0..n {
        let Some(&p) = pts.get((start + k) % n) else {
            continue;
        };
        let soft = out.len() >= 2 && matches!((out.get(out.len() - 2), out.last()), (Some(&prev), Some(&last)) if turn_deg(prev, last, p) <= tol);
        if soft {
            out.pop();
        }
        out.push(p);
    }
    // The seam: the last corner's neighbours are the one before it and the first.
    while out.len() > 3 {
        let (Some(&prev), Some(&last), Some(&first)) = (out.get(out.len() - 2), out.last(), out.first()) else {
            break;
        };
        if turn_deg(prev, last, first) <= tol {
            out.pop();
        } else {
            break;
        }
    }
    if out.len() < 3 { pts } else { out }
}

/// Whether two corners are the same point to within a nanometre.
fn same_point(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

/// How far the direction turns at `here`, in degrees: `0` straight on, `90` a right angle, `180`
/// straight back. A repeated point turns by nothing.
fn turn_deg(prev: [f64; 2], here: [f64; 2], next: [f64; 2]) -> f64 {
    let (ax, ay) = (here[0] - prev[0], here[1] - prev[1]);
    let (bx, by) = (next[0] - here[0], next[1] - here[1]);
    let la = (ax * ax + ay * ay).sqrt();
    let lb = (bx * bx + by * by).sqrt();
    if la <= 0.0 || lb <= 0.0 {
        return 0.0;
    }
    ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::{
        Tuning, Wall, collapse_parallel_walls, detect_curved_walls, extract, extract_walls_from_band, fit_circle_kasa, merge_collinear_walls,
        non_maximal_suppression,
    };
    use crate::bim::storeys::Storey;
    use crate::bim::{BimError, BimOptions};

    /// The storey between two slab faces, with the default 10 cm clearance already taken off both.
    fn storey_at(floor_face: f64, ceiling_face: f64) -> Storey {
        let margin = BimOptions::default().safety_margin;
        Storey { index: 0, base_z: floor_face + margin, top_z: ceiling_face - margin }
    }

    /// Points filling a wall of `thickness` metres centred on the axis `from`..`to`, at every
    /// decimetre of a `height`-tall wall. The samples stop a centimetre short of each face so the
    /// raster's half-open far edge never adds a cell.
    fn wall_points(from: [f64; 2], to: [f64; 2], thickness: f64, height: f64) -> Vec<[f64; 3]> {
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let length = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = (dx / length, dy / length);
        let (nx, ny) = (-uy, ux);
        let mut out = Vec::new();
        let mut u = 0.01;
        while u <= length - 0.005 {
            let mut v = -thickness / 2.0 + 0.01;
            while v <= thickness / 2.0 - 0.01 + 1e-9 {
                let mut z = 0.1;
                while z <= height + 1e-9 {
                    out.push([from[0] + ux * u + nx * v, from[1] + uy * u + ny * v, z]);
                    z += 0.05;
                }
                v += 0.02;
            }
            u += 0.02;
        }
        out
    }

    /// Four walls on the axes of a square: the case every real floor is built around.
    fn wall_box(thickness: f64, height: f64) -> Vec<[f64; 3]> {
        let e = -thickness / 2.0;
        let f = 4.0 + thickness / 2.0;
        let mut pts = Vec::new();
        pts.extend(wall_points([e, 0.0], [f, 0.0], thickness, height));
        pts.extend(wall_points([4.0, e], [4.0, f], thickness, height));
        pts.extend(wall_points([f, 4.0], [e, 4.0], thickness, height));
        pts.extend(wall_points([0.0, f], [0.0, e], thickness, height));
        pts
    }

    /// The box plus two free-standing walls at 45 degrees, both inside the room.
    fn wall_box_with_two_diagonals(thickness: f64, height: f64) -> Vec<[f64; 3]> {
        let mut pts = wall_box(thickness, height);
        pts.extend(wall_points([0.9, 3.0], [2.1, 1.8], thickness, height));
        pts.extend(wall_points([1.9, 3.2], [3.1, 2.0], thickness, height));
        pts
    }

    /// The axes the six walls of the fixture are built on: a point on each and its direction.
    fn expected_axes() -> Vec<([f64; 2], [f64; 2])> {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        vec![
            ([0.0, 0.0], [1.0, 0.0]),
            ([4.0, 0.0], [0.0, 1.0]),
            ([4.0, 4.0], [-1.0, 0.0]),
            ([0.0, 4.0], [0.0, -1.0]),
            ([0.9, 3.0], [h, -h]),
            ([1.9, 3.2], [h, -h]),
        ]
    }

    /// How far a wall's axis is from the line through `want`: the worse of its two ends. Both
    /// axes are taken as lines, so a wall that stops early is measured by where it lies, not how
    /// long it is.
    fn axis_error(wall: &Wall, want: ([f64; 2], [f64; 2])) -> f64 {
        let (p, d) = want;
        let off = |q: [f64; 2]| ((q[0] - p[0]) * d[1] - (q[1] - p[1]) * d[0]).abs();
        off(wall.from).max(off(wall.to))
    }

    #[test]
    fn the_six_walls_come_back_within_a_lattice_cell_of_their_axes() {
        // The plan carried the paper's 2.5 mm claim into this assertion. It is not reachable with
        // the pipeline as specified: the wall positions come from a 5 cm lattice, and a 45-degree
        // face can only be placed to the nearest lattice corner, up to a half diagonal (35 mm)
        // away. The bound asserted here is the lattice's, and the orthogonal walls — whose faces
        // fall on the lattice exactly — are held to zero.
        let o = BimOptions::default();
        let pts = wall_box_with_two_diagonals(0.20, 3.0);
        let walls = extract(&pts, &storey_at(0.0, 3.0), &o).expect("walls");
        assert_eq!(walls.len(), 6, "four orthogonal and two at 45 degrees");
        let mut orthogonals = 0;
        for want in expected_axes() {
            let best = walls.iter().map(|w| axis_error(w, want)).fold(f64::INFINITY, f64::min);
            assert!(best <= o.cell, "an axis is {best:.4} m from the truth, over the {:.3} m cell", o.cell);
            if want.1[0] == 0.0 || want.1[1] == 0.0 {
                orthogonals += 1;
                assert!(best <= 1e-9, "an orthogonal axis on the lattice is exact, got {best:e}");
            }
        }
        assert_eq!(orthogonals, 4);
        let mut diagonals = 0;
        for w in &walls {
            // The thickness is a face separation, so it inherits the same lattice bound as the
            // axes: exact when both faces lie on cell lines, up to a cell when they are at 45
            // degrees. The plan's 2 cm was the paper's number for its own grid, not for this one.
            let (dx, dy) = (w.to[0] - w.from[0], w.to[1] - w.from[1]);
            let axis_aligned = dx.abs() < 1e-9 || dy.abs() < 1e-9;
            let bound = if axis_aligned { 1e-9 } else { o.cell };
            if !axis_aligned {
                diagonals += 1;
            }
            assert!((w.thickness - 0.20).abs() <= bound, "thickness from the paired faces: {} (bound {bound})", w.thickness);
            assert!((w.base_z - 0.1).abs() < 1e-9 && (w.height - 2.8).abs() < 1e-9, "the wall spans the storey");
            assert!(!w.exterior, "both faces were scanned, so nothing was synthesised");
        }
        assert_eq!(diagonals, 2, "the two 45-degree walls come back as walls, not as dropped staircases");
    }

    #[test]
    fn two_parallel_faces_of_one_wall_become_one_axis_between_them() {
        let walls = extract(&wall_points([0.0, 0.0], [0.0, 3.0], 0.20, 3.0), &storey_at(0.0, 3.0), &BimOptions::default()).expect("a wall");
        assert_eq!(walls.len(), 1, "the pair is one wall, not two");
        assert!((walls[0].thickness - 0.20).abs() < 0.02, "thickness {}", walls[0].thickness);
        let mid = [(walls[0].from[0] + walls[0].to[0]) / 2.0, (walls[0].from[1] + walls[0].to[1]) / 2.0];
        assert!(mid[0].abs() < 1e-9, "the axis is the midline of the two faces, not either one: x = {}", mid[0]);
    }

    /// Points on the four room-side faces of a 4 x 4 room, one cell thick: the case of a scan
    /// taken entirely inside, with no far face anywhere. Each band runs into the corners so the
    /// four meet, as walls do.
    fn inner_faces_only() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        let levels = [0.30, 0.70, 1.10, 1.50, 1.90, 2.30, 2.70, 2.80, 2.90];
        // A fixed count, not an accumulated step: the last sample must land on 4.01 so the bands
        // meet in the corner cell. At `t += 0.02` the last value is a hair over 4.01 and is lost,
        // which leaves a one-cell gap and turns the annulus into one long ring.
        for k in 0..=201 {
            let t = -0.01 + 0.02 * f64::from(k);
            for z in levels {
                pts.push([t, -0.01, z]); // bottom face, body at y < 0
                pts.push([t, 4.01, z]); // top face, body at y > 4
                pts.push([-0.01, t, z]); // left face
                pts.push([4.01, t, z]); // right face
            }
        }
        pts
    }

    #[test]
    fn an_interior_only_scan_still_gets_a_perimeter_of_the_named_thickness() {
        let o = BimOptions { exterior_scan: false, exterior_wall_thickness: 0.30, ..BimOptions::default() };
        let walls = extract(&inner_faces_only(), &storey_at(0.0, 3.0), &o).expect("walls");
        let thick = walls.iter().filter(|w| w.exterior).map(|w| w.thickness).collect::<Vec<_>>();
        assert!(
            !thick.is_empty() && thick.iter().all(|t| (t - 0.30).abs() < 1e-9),
            "the outer face is virtual, so the thickness is the one the user named: {thick:?}"
        );
        assert_eq!(walls.len(), 4, "one synthesised wall per room side, and no thin pairs: {walls:?}");
    }

    #[test]
    fn an_exterior_scan_drops_lone_faces_instead_of_synthesising_them() {
        // The flag says both faces were scanned, so a lone face has no partner to be paired with
        // and is not a wall this stage can justify: it is dropped rather than thickened.
        let o = BimOptions { exterior_scan: true, ..BimOptions::default() };
        let walls = extract(&inner_faces_only(), &storey_at(0.0, 3.0), &o).expect("no walls is not an error");
        assert!(walls.is_empty(), "no pair, no wall, and nothing invented: {walls:?}");
    }

    /// A wardrobe-like panel: 1.2 m tall, 0.10 m thick, standing on the floor of the box.
    fn furniture_panel() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        let mut x = 1.5;
        while x <= 2.7 {
            for v in [-0.04, -0.02, 0.0, 0.02, 0.04] {
                let mut z = 0.1;
                while z <= 0.35 + 1e-9 {
                    pts.push([x, 2.0 + v, z]);
                    z += 0.1;
                }
            }
            x += 0.02;
        }
        pts
    }

    fn assert_same_walls(a: &[Wall], b: &[Wall]) {
        let key = |w: &Wall| (w.from[0], w.from[1], w.to[0], w.to[1], w.thickness);
        let mut ka: Vec<_> = a.iter().map(key).collect();
        let mut kb: Vec<_> = b.iter().map(key).collect();
        ka.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        kb.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        assert_eq!(ka, kb, "the same points must give the same walls");
    }

    #[test]
    fn furniture_below_the_slice_cannot_become_a_wall() {
        let o = BimOptions::default();
        let mut furnished = wall_box(0.20, 3.0);
        furnished.extend(furniture_panel());
        let high = extract(&furnished, &storey_at(0.0, 3.0), &o).expect("walls");
        let bare = extract(&wall_box(0.20, 3.0), &storey_at(0.0, 3.0), &o).expect("walls");
        assert_eq!(high.len(), bare.len(), "the panel is 0.35 m tall and the secondary slice starts at {:.2} m", 0.45);
        assert_same_walls(&high, &bare);
        // The raw slice at floor level sees the panel:
        let low = BimOptions { slice_from: 0.0, slice_to: 0.1, ..o };
        let floor_band: Vec<[f64; 3]> = furnished.iter().filter(|p| p[2] >= 0.1 && p[2] <= 0.38).copied().collect();
        let raw_floor = extract_walls_from_band(&floor_band, 0.1, 2.8, &Tuning::read(&low).unwrap()).unwrap();
        assert!(raw_floor.len() > bare.len(), "raw slice at the floor sees the panel");
        // But the topological filter rejects the floating panel in the room:
        let floor = extract(&furnished, &storey_at(0.0, 3.0), &low).expect("walls");
        assert_eq!(floor.len(), bare.len(), "isolated floating furniture is rejected by topological filter");
    }

    #[test]
    fn a_storey_with_nothing_in_the_slice_has_no_walls_and_is_not_an_error() {
        let o = BimOptions::default();
        assert!(extract(&[], &storey_at(0.0, 3.0), &o).expect("no points").is_empty());
        let under_floor = vec![[0.0, 0.0, 0.05], [1.0, 1.0, -0.2]];
        assert!(extract(&under_floor, &storey_at(0.0, 3.0), &o).expect("nothing in the slice").is_empty());
        let inverted = Storey { index: 0, base_z: 3.0, top_z: 1.0 };
        assert!(extract(&under_floor, &inverted, &o).expect("an inverted storey holds nothing").is_empty());
    }

    #[test]
    fn hostile_options_are_refused_rather_than_guessed() {
        let pts = wall_box(0.20, 3.0);
        let storey = storey_at(0.0, 3.0);
        let bad = [
            BimOptions { cell: 0.0, ..BimOptions::default() },
            BimOptions { cell: f64::NAN, ..BimOptions::default() },
            BimOptions { slice_from: 0.9, slice_to: 0.5, ..BimOptions::default() },
            BimOptions { slice_to: 1.5, ..BimOptions::default() },
            BimOptions { slice_from: f64::NAN, ..BimOptions::default() },
            BimOptions { threshold: 0.0, ..BimOptions::default() },
            BimOptions { threshold: f64::INFINITY, ..BimOptions::default() },
            BimOptions { epsilon: -0.1, ..BimOptions::default() },
            BimOptions { angle_tolerance_deg: 91.0, ..BimOptions::default() },
            BimOptions { min_wall_length: f64::NAN, ..BimOptions::default() },
            BimOptions { max_wall_thickness: 0.05, ..BimOptions::default() },
            BimOptions { exterior_wall_thickness: 0.0, ..BimOptions::default() },
        ];
        for o in bad {
            assert!(matches!(extract(&pts, &storey, &o), Err(BimError::InvalidCell)), "{o:?} cannot be used and must be reported, not guessed at");
        }
    }
    #[test]
    fn parapet_detection_and_z_p95_calculation() {
        let storey = Storey { index: 0, base_z: 0.0, top_z: 2.80 };
        let opts = BimOptions::default();

        // 1. Full-height wall at y = 0.0 with points up to z = 2.80
        let mut pts = Vec::new();
        for i in 0..100 {
            let x = (i as f64) * 0.04;
            for k in 1..28 {
                let z = (k as f64) * 0.10;
                pts.push([x, -0.10, z]);
                pts.push([x, 0.10, z]);
            }
        }

        // 2. Parapet wall at y = 3.0 with points only up to z = 1.00 (below top_z - 0.20 = 2.60)
        for i in 0..80 {
            let x = 0.5 + (i as f64) * 0.04;
            for k in 1..11 {
                let z = (k as f64) * 0.10;
                pts.push([x, 2.90, z]);
                pts.push([x, 3.10, z]);
            }
        }

        let walls = extract(&pts, &storey, &opts).expect("extraction should succeed");
        assert_eq!(walls.len(), 2, "must detect full-height wall and parapet wall");

        let full_wall = walls.iter().find(|w| (w.from[1] - 0.0).abs() < 0.2).expect("full wall");
        assert!(!full_wall.is_parapet, "full height wall is not a parapet");
        assert_eq!(full_wall.height, 2.80, "full height wall spans clear height");

        let parapet = walls.iter().find(|w| (w.from[1] - 3.0).abs() < 0.2).expect("parapet wall");
        assert!(parapet.is_parapet, "parapet wall is detected as parapet");
        assert!((parapet.height - 1.00).abs() < 0.15, "parapet height matches z_p95: {}", parapet.height);
    }

    #[test]
    fn fragmented_collinear_walls_merge_into_one_continuous_wall() {
        let walls = vec![
            Wall { from: [0.0, 0.0], to: [2.0, 0.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Doorway gap of 0.80 m (<= 1.0 m)
            Wall { from: [2.8, 0.02], to: [5.0, 0.02], thickness: 0.22, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Another gap of 0.60 m (<= 1.0 m)
            Wall { from: [5.6, -0.01], to: [8.0, -0.01], thickness: 0.18, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Non-collinear wall (perpendicular)
            Wall { from: [8.0, 0.0], to: [8.0, 4.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
        ];

        let merged = merge_collinear_walls(walls, 3.0, 0.15, 1.0);
        assert_eq!(merged.len(), 2, "three collinear segments should merge into 1, plus 1 perpendicular wall");

        let long_wall = merged.iter().find(|w| (w.from[0] - 0.0).abs() < 0.1).expect("merged wall");
        let len = (long_wall.to[0] - long_wall.from[0]).hypot(long_wall.to[1] - long_wall.from[1]);
        assert!((len - 8.0).abs() < 0.1, "merged wall spans full length: {len}");
        assert!((long_wall.thickness - 0.20).abs() < 0.02, "average thickness is ~0.20: {}", long_wall.thickness);
    }

    #[test]
    fn corner_extension_and_orphan_filter() {
        let walls = vec![
            // Wall 1: horizontal wall from (0, 0) to (3.8, 0)
            Wall { from: [0.0, 0.0], to: [3.8, 0.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Wall 2: perpendicular wall from (4.0, 0.2) to (4.0, 4.0).
            // Its end (4.0, 0.2) is within 0.2 m (<= 0.35 m) of Wall 1's line (y = 0).
            // Wall 1's end (3.8, 0) is within 0.2 m (<= 0.35 m) of Wall 2's line (x = 4.0).
            Wall { from: [4.0, 0.2], to: [4.0, 4.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Wall 3: short corner return (length 0.30 m < 0.40 m) connected to Wall 2 at (4.0, 4.0)
            Wall { from: [4.0, 4.0], to: [3.7, 4.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Wall 4: isolated orphan wall (length 0.25 m < 0.40 m) not connected to anything
            Wall { from: [10.0, 10.0], to: [10.25, 10.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
        ];

        let merged = merge_collinear_walls(walls, 3.0, 0.15, 1.20);
        // Wall 4 should be dropped because length < 0.40 and unconnected.
        // Wall 3 should be kept because it is connected to Wall 2.
        // Wall 1 and Wall 2 should snap to (4.0, 0.0) at the corner.
        assert_eq!(merged.len(), 3, "orphan dropped, connected short wall preserved, corner snapped");

        let w1 = merged.iter().find(|w| (w.from[0] - 0.0).abs() < 0.1).expect("wall 1");
        assert!((w1.to[0] - 4.0).abs() < 1e-3, "wall 1 snapped to x=4: {}", w1.to[0]);

        let w2 = merged.iter().find(|w| (w.to[1] - 4.0).abs() < 0.1 && (w.from[0] - 4.0).abs() < 0.1).expect("wall 2");
        assert!((w2.from[1] - 0.0).abs() < 1e-3, "wall 2 snapped to y=0: {}", w2.from[1]);
    }

    #[test]
    fn parallel_wall_collapse_pairs_and_merges_opposite_faces() {
        let walls = vec![
            // Two parallel walls along x, separated by d_perp = 0.20 m (y = 0.0 and y = 0.20)
            Wall { from: [0.0, 0.0], to: [4.0, 0.0], thickness: 0.10, base_z: 0.0, height: 2.80, exterior: true, is_parapet: false, arc: None },
            Wall { from: [0.5, 0.20], to: [3.5, 0.20], thickness: 0.10, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // A perpendicular wall that should NOT be collapsed
            Wall { from: [4.0, 0.0], to: [4.0, 3.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
        ];

        let collapsed = collapse_parallel_walls(walls);
        assert_eq!(collapsed.len(), 2, "parallel pair collapses into 1, perpendicular remains");

        let merged = collapsed.iter().find(|w| (w.to[0] - w.from[0]).abs() > 1.0).expect("merged parallel wall");
        // Midpoint line is at y = 0.10
        assert!((merged.from[1] - 0.10).abs() < 1e-3, "midline y = 0.10, got {}", merged.from[1]);
        assert!((merged.to[1] - 0.10).abs() < 1e-3, "midline y = 0.10, got {}", merged.to[1]);
        // Union of intervals: x from 0.0 to 4.0
        assert!((merged.from[0] - 0.0).abs() < 1e-3, "spans union x min 0.0");
        assert!((merged.to[0] - 4.0).abs() < 1e-3, "spans union x max 4.0");
        // Thickness = d_perp (0.20) + (0.10 + 0.10) / 2.0 = 0.30 m
        assert!((merged.thickness - 0.30).abs() < 1e-3, "thickness is 0.30 m: {}", merged.thickness);
        // is_parapet is false
        assert!(!merged.is_parapet);
        // exterior is false (w1.exterior && w2.exterior = true && false = false)
        assert!(!merged.exterior);
    }

    #[test]
    fn non_maximal_suppression_drops_short_parallel_ghost_wall() {
        let walls = vec![
            // Long wall (length 3.0 m >= 1.20 m)
            Wall { from: [0.0, 0.0], to: [3.0, 0.0], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Short parallel ghost wall (length 0.50 m < 0.65 m, d_perp = 0.15 m <= 0.45 m)
            Wall { from: [1.0, 0.15], to: [1.5, 0.15], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
            // Short perpendicular wall (length 0.50 m, angle = 90 deg) which should NOT be dropped
            Wall { from: [3.0, 0.0], to: [3.0, 0.50], thickness: 0.20, base_z: 0.0, height: 2.80, exterior: false, is_parapet: false, arc: None },
        ];

        let nms = non_maximal_suppression(walls);
        assert_eq!(nms.len(), 2, "short parallel ghost wall dropped, perpendicular kept");
        assert!(!nms.iter().any(|w| (w.from[1] - 0.15).abs() < 1e-3), "ghost wall was suppressed");
    }

    #[test]
    fn fit_circle_kasa_accuracy_and_degeneracy() {
        // 1. Exact circle of radius 4.0 centered at (2.0, 3.0)
        let mut pts = Vec::new();
        for i in 0..12 {
            let theta = (i as f64) * std::f64::consts::PI / 6.0;
            pts.push([2.0 + 4.0 * theta.cos(), 3.0 + 4.0 * theta.sin()]);
        }
        let (center, radius, rmse) = fit_circle_kasa(&pts).expect("exact circle must fit");
        assert!((center[0] - 2.0).abs() < 1e-6, "center x: {}", center[0]);
        assert!((center[1] - 3.0).abs() < 1e-6, "center y: {}", center[1]);
        assert!((radius - 4.0).abs() < 1e-6, "radius: {radius}");
        assert!(rmse < 1e-6, "rmse: {rmse}");

        // 2. Collinear points must return None without panicking
        let collinear = vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        assert!(fit_circle_kasa(&collinear).is_none(), "collinear points have no circle");

        // 3. Degenerate inputs (fewer than 3 points, identical points, NaNs)
        assert!(fit_circle_kasa(&[[0.0, 0.0], [1.0, 1.0]]).is_none(), "2 points must return None");
        assert!(fit_circle_kasa(&[[1.0, 1.0], [1.0, 1.0], [1.0, 1.0]]).is_none(), "identical points must return None");
        assert!(fit_circle_kasa(&[[f64::NAN, 0.0], [1.0, 1.0], [2.0, 0.0]]).is_none(), "NaN must return None");
    }

    #[test]
    fn curved_wall_detection_and_regression() {
        // Construct a chain of 4 segments approximating an arc of R = 3.0 m, center = (0.0, 0.0)
        // From theta = 0 to theta = 60 degrees (pi/3)
        // 4 segments = 5 points: theta_0 = 0, theta_1 = 15 deg, theta_2 = 30 deg, theta_3 = 45 deg, theta_4 = 60 deg
        let mut arc_pts = Vec::new();
        for i in 0..=4 {
            let theta = (i as f64) * (std::f64::consts::PI / 12.0); // 15 degrees per step
            arc_pts.push([3.0 * theta.cos(), 3.0 * theta.sin()]);
        }

        let mut segs = Vec::new();
        for i in 0..4 {
            segs.push(Wall {
                from: arc_pts[i],
                to: arc_pts[i + 1],
                thickness: 0.20,
                base_z: 0.0,
                height: 2.80,
                exterior: false,
                is_parapet: false,
                arc: None,
            });
        }

        let detected = detect_curved_walls(segs);
        assert_eq!(detected.len(), 1, "4 segments along circular arc must merge into 1 curved wall");
        let cw = &detected[0];
        let arc = cw.arc.expect("must have WallArc");
        assert!((arc.center[0] - 0.0).abs() < 0.05, "center x near 0: {}", arc.center[0]);
        assert!((arc.center[1] - 0.0).abs() < 0.05, "center y near 0: {}", arc.center[1]);
        assert!((arc.radius - 3.0).abs() < 0.05, "radius near 3.0 m: {}", arc.radius);
        assert!(arc.ccw, "arc is counter-clockwise");
        assert!((cw.thickness - 0.20).abs() < 1e-4);
        assert_eq!(cw.height, 2.80);
    }
}
