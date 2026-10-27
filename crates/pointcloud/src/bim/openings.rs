//! Openings in the walls: the elevation raster that finds them, and the door/window classifier.
//!
//! The paper (arXiv:2503.11498, §3.4) projects wall points onto a `u × z` elevation raster and
//! looks for regions of empty cells that satisfy size and classification rules. Every number that
//! the paper does not publish is labelled *(ours)* in the doc comment of the option that carries
//! it; every number the paper does publish is labelled *(paper)*.
//!
//! The module has three public entry points:
//! - [`band`] — the world box to stream fine points for one wall.
//! - [`detect`] — the per-wall opening detector.
//! - [`detect_all`] — loop all walls, concatenate and sort.

use crate::Band;
use crate::bim::raster::{self, MAX_RASTER_CELLS, Raster};
use crate::bim::walls::Wall;
use crate::bim::{BimError, BimOptions};

// ---------------------------------------------------------------------------
// Public types — the contract the engine draws from; kept exactly as the stub.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpeningShape {
    Rectangular,
    Arched { arch_radius: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpeningKind {
    Door,
    Window,
    ClosedDoor { recess_depth: f64 },
}

impl OpeningKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpeningKind::Door => "door",
            OpeningKind::Window => "window",
            OpeningKind::ClosedDoor { .. } => "closed_door",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opening {
    pub wall: usize,
    pub kind: OpeningKind,
    pub shape: OpeningShape,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub thickness: f64,
    pub sill_z: f64,
    pub head_z: f64,
}

impl Opening {
    pub fn width(&self) -> f64 {
        (self.to[0] - self.from[0]).hypot(self.to[1] - self.from[1])
    }

    pub fn height(&self) -> f64 {
        self.head_z - self.sill_z
    }

    pub fn center(&self) -> [f64; 2] {
        [(self.from[0] + self.to[0]) / 2.0, (self.from[1] + self.to[1]) / 2.0]
    }
}

// ---------------------------------------------------------------------------
// band
// ---------------------------------------------------------------------------

/// The world box to stream fine points for this wall: the axis plus `opening_band` on every side,
/// and the full height from `base_z` to `base_z + height`. A zero-length wall or unusable options
/// return `None` — a caller that calls with a good wall gets a band; an unusable one gets silence.
///
/// *(ours)* The XY box is the wall's own body (±thickness/2 in the normal direction, 0..len in
/// the along-wall direction), expanded by `opening_band` on all four sides.
pub fn band(wall: &Wall, opts: &BimOptions) -> Option<Band> {
    // Validate the options this function uses; any unusable one → None (not an error, never panics).
    let ob = opts.opening_band;
    if !ob.is_finite() || ob < 0.0 {
        return None;
    }
    let dx = wall.to[0] - wall.from[0];
    let dy = wall.to[1] - wall.from[1];
    let len = dx.hypot(dy);
    if !len.is_finite() || len <= 0.0 {
        return None;
    }
    // Unit direction and normal.
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let half_thick = wall.thickness / 2.0;
    if !half_thick.is_finite() {
        return None;
    }
    // The four corners of the body in world XY. The band margin `ob` expands them outward.
    // `ux * ob` extends past each wall end along the axis; `(half_thick + ob)` expands off each face.
    let corners = [
        (wall.from[0] - ux * ob - nx * (half_thick + ob), wall.from[1] - uy * ob - ny * (half_thick + ob)),
        (wall.from[0] - ux * ob + nx * (half_thick + ob), wall.from[1] - uy * ob + ny * (half_thick + ob)),
        (wall.to[0] + ux * ob - nx * (half_thick + ob), wall.to[1] + uy * ob - ny * (half_thick + ob)),
        (wall.to[0] + ux * ob + nx * (half_thick + ob), wall.to[1] + uy * ob + ny * (half_thick + ob)),
    ];
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for (x, y) in corners {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        min[0] = min[0].min(x);
        min[1] = min[1].min(y);
        max[0] = max[0].max(x);
        max[1] = max[1].max(y);
    }
    let z0 = wall.base_z;
    let z1 = wall.base_z + wall.height;
    if !z0.is_finite() || !z1.is_finite() || z1 <= z0 {
        return None;
    }
    Some(Band { min, max, z0, z1 })
}

// ---------------------------------------------------------------------------
// detect
// ---------------------------------------------------------------------------

/// The validated options for this stage, read once rather than re-checked per point.
struct Tuning {
    cell: f64,
    threshold: f64,
    rank: usize,
    min_width: f64,
    max_width: f64,
    door_min_height: f64,
    door_max_sill: f64,
    window_min_sill: f64,
    band: f64,
}

impl Tuning {
    fn read(o: &BimOptions) -> Result<Self, BimError> {
        // Every validation failure is InvalidCell — the variant the crate uses for "a parameter
        // that cannot be used", not only for a literal cell size.
        let ok = o.opening_cell.is_finite()
            && o.opening_cell > 0.0
            && o.opening_threshold.is_finite()
            && o.opening_threshold > 0.0
            && o.opening_threshold <= 1.0
            && o.opening_rank >= 1
            && o.min_opening_width.is_finite()
            && o.min_opening_width > 0.0
            && o.max_opening_width.is_finite()
            && o.max_opening_width >= o.min_opening_width
            && o.door_min_height.is_finite()
            && o.door_min_height >= 0.0
            && o.door_max_sill.is_finite()
            && o.door_max_sill >= 0.0
            && o.window_min_sill.is_finite()
            && o.window_min_sill >= 0.0
            && o.opening_band.is_finite()
            && o.opening_band >= 0.0;
        if !ok {
            return Err(BimError::InvalidCell);
        }
        Ok(Tuning {
            cell: o.opening_cell,
            threshold: o.opening_threshold,
            rank: o.opening_rank,
            min_width: o.min_opening_width,
            max_width: o.max_opening_width,
            door_min_height: o.door_min_height,
            door_max_sill: o.door_max_sill,
            window_min_sill: o.window_min_sill,
            band: o.opening_band,
        })
    }
}

/// Detect the openings in one wall.
///
/// `points` are fine-resolution points already streamed for this wall's band. The `wall_index` is
/// carried into every returned `Opening::wall` so the IFC writer and drawing code agree on which
/// wall the hole is in.
///
/// # Errors
/// - [`BimError::InvalidCell`] when any option this stage reads is unusable.
/// - [`BimError::TooLarge`] when the elevation raster would exceed [`MAX_RASTER_CELLS`].
pub fn detect(points: &[[f64; 3]], wall: &Wall, wall_index: usize, opts: &BimOptions) -> Result<Vec<Opening>, BimError> {
    // Step 1: validate options once before any work.
    let t = Tuning::read(opts)?;

    // Step 2: wall axis — unit direction and left-hand normal, length.
    let dx = wall.to[0] - wall.from[0];
    let dy = wall.to[1] - wall.from[1];
    let len = dx.hypot(dy);
    if !len.is_finite() || len <= 0.0 {
        // A wall with no line has no elevation to read.
        return Ok(vec![]);
    }
    let (dux, duy) = (dx / len, dy / len); // unit d (along wall)
    let (nx, ny) = (-duy, dux); // unit normal (left-hand)

    // Step 3: keep the points on this wall's body. Non-finite points are skipped, never indexed.
    // u = (p - from)·d ∈ [-band, len+band]
    // |v| = |(p - from)·n| ≤ thickness/2 + band
    // z ∈ [base_z, base_z + height]
    let half_thick = wall.thickness / 2.0;
    let u_lo = -t.band;
    let u_hi = len + t.band;
    let v_limit = half_thick + t.band;
    let z_lo = wall.base_z;
    let z_hi = wall.base_z + wall.height;

    let mut proj: Vec<(f64, f64)> = Vec::new(); // (u, z − base_z)
    for &[px, py, pz] in points {
        if !px.is_finite() || !py.is_finite() || !pz.is_finite() {
            continue; // non-finite points skipped at the door
        }
        if pz < z_lo || pz > z_hi {
            continue;
        }
        let rx = px - wall.from[0];
        let ry = py - wall.from[1];
        let u = rx * dux + ry * duy;
        let v = rx * nx + ry * ny;
        if u < u_lo || u > u_hi || v.abs() > v_limit {
            continue;
        }
        proj.push((u, pz - wall.base_z));
    }

    // Step 4: rasterise (u, z−base_z) at opening_cell.
    // The raster spans [0, len+cell] × [0, height+cell] regardless of which points arrived; a
    // wall whose full face is present except for a hole will fill all non-hole cells.
    if !wall.height.is_finite() || wall.height <= 0.0 {
        return Ok(vec![]);
    }
    let r_u_max = len + t.cell; // one extra cell so the last legal u lands inside
    let r_z_max = wall.height + t.cell;

    let Some(mut raster) = Raster::new([0.0, 0.0], [r_u_max, r_z_max], t.cell) else {
        return Err(BimError::TooLarge { what: "opening raster", limit: MAX_RASTER_CELLS });
    };
    for &(u, dz) in &proj {
        raster.add([u, dz, 0.0]);
    }

    // Step 5: rank-10 threshold (the paper's `max10`). *(paper)*
    // Sort cell densities descending; `reference = densities[min(rank, n) - 1]` (1-based rank).
    // A cell is occupied when density > 0 AND density ≥ opening_threshold × reference.
    // `opening_threshold` is ours; the rank is the paper's.
    let (w, h) = raster.dims();
    let density = raster.density();
    let mut sorted: Vec<u32> = density.iter().copied().filter(|&d| d > 0).collect();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let n = sorted.len();
    if n == 0 {
        return Ok(vec![]);
    }
    // Clamp rank to the density count — a hostile `usize::MAX` must not allocate or index
    // past the sorted vec. *(paper: the rank selects the reference cell)*
    let rank_idx = t.rank.min(n) - 1; // 0-based index into the sorted descending list
    let reference = match sorted.get(rank_idx) {
        Some(&v) => v,
        None => return Ok(vec![]),
    };
    if reference == 0 {
        // The rank-th densest cell has nothing → nothing to read.
        return Ok(vec![]);
    }
    let cut = t.threshold * f64::from(reference);
    let occupied: Vec<bool> = density.iter().map(|&d| d > 0 && f64::from(d) >= cut).collect();

    // Step 6: close the occupied mask with radius 1.
    // A 2 cm close bridges scan cracks; a 0.6 m door is thirty cells wide and cannot close.
    // *(ours)*
    let closed = raster::close(&occupied, w, h, 1);

    // Step 7: flood-fill the **empty** cells with 4-neighbour connectivity. Each filled region is
    // a candidate hole. The stack is a `Vec<u32>` — no recursion — bounded by `w·h`.
    let cells = w * h;
    let mut visited: Vec<bool> = vec![false; cells];
    let mut openings_out: Vec<Opening> = Vec::new();

    // Pre-mark every occupied (closed) cell as visited so flood-fill never enters it.
    for (i, seen) in visited.iter_mut().enumerate() {
        if closed.get(i).copied().unwrap_or(true) {
            *seen = true;
        }
    }

    // Each unvisited seed starts a new connected empty region.
    for seed in 0..cells {
        if visited.get(seed).copied().unwrap_or(true) {
            continue;
        }
        let mut stack: Vec<u32> = Vec::new();
        let mut region_min_u = usize::MAX;
        let mut region_max_u = 0usize;
        let mut region_min_z = usize::MAX;
        let mut region_max_z = 0usize;
        let mut touches_left = false;
        let mut touches_right = false;
        let mut col_max_cy: Vec<Option<usize>> = vec![None; w];

        stack.push(seed as u32);
        if let Some(v) = visited.get_mut(seed) {
            *v = true;
        }
        // Budget: at most w·h iterations — the entire grid — so the loop always terminates.
        let budget = cells + 1;
        let mut iters = 0usize;

        while let Some(cur) = stack.pop() {
            iters += 1;
            if iters > budget {
                break;
            }
            let ci = cur as usize;
            let cx = ci % w; // column = u cell index
            let cy = ci / w; // row = z cell index

            // Track the bounding box of this region.
            region_min_u = region_min_u.min(cx);
            region_max_u = region_max_u.max(cx);
            region_min_z = region_min_z.min(cy);
            region_max_z = region_max_z.max(cy);

            if let Some(entry) = col_max_cy.get_mut(cx) {
                *entry = Some(entry.map_or(cy, |m| m.max(cy)));
            }

            // Check edge touches.
            if cx == 0 {
                touches_left = true;
            }
            if cx + 1 >= w {
                touches_right = true;
            }

            // 4-neighbour expansion.
            for (nx2, ny2) in [(cx.wrapping_sub(1), cy), (cx + 1, cy), (cx, cy.wrapping_sub(1)), (cx, cy + 1)] {
                if nx2 >= w || ny2 >= h {
                    continue;
                }
                let ni = ny2 * w + nx2;
                if visited.get(ni).copied().unwrap_or(true) {
                    continue;
                }
                if let Some(v) = visited.get_mut(ni) {
                    *v = true;
                }
                stack.push(ni as u32);
            }
        }

        // Abort regions that never accumulated any cell.
        if region_min_u > region_max_u || region_min_z > region_max_z {
            continue;
        }

        // Step 8: reject candidates that span the entire wall length (unpopulated wall)
        // or whose width is outside the accepted range.
        if touches_left && touches_right {
            continue;
        }

        // Width in world metres: (max_u_cell − min_u_cell + 1) × cell. *(ours)*
        let width_m = (region_max_u - region_min_u + 1) as f64 * t.cell;
        if width_m < t.min_width || width_m > t.max_width {
            continue;
        }

        // Step 9: sill and head in world metres.
        // sill = base_z + min_z_cell × cell; head = base_z + (max_z_cell + 1) × cell.
        let sill_above_base = region_min_z as f64 * t.cell;
        let head_above_base = (region_max_z + 1) as f64 * t.cell;
        let height_m = head_above_base - sill_above_base;

        // Classify: door when sill ≤ door_max_sill and height ≥ door_min_height; window when
        // sill ≥ window_min_sill; anything else is dropped.
        let kind = if sill_above_base <= t.door_max_sill && height_m >= t.door_min_height {
            OpeningKind::Door
        } else if sill_above_base >= t.window_min_sill {
            OpeningKind::Window
        } else {
            continue;
        };

        let sill_z = wall.base_z + sill_above_base;
        let head_z = wall.base_z + head_above_base;

        // Arched top fitting on candidate openings: check upper region [0.75, 1.0] of height.
        // If the profile fits a semicircle with RMSE <= 0.03 m, set shape: Arched { arch_radius: width / 2.0 }.
        let r_arch = width_m / 2.0;
        let is_arched = if r_arch > 0.0 && r_arch <= 0.60 * height_m {
            let mut sum_sq = 0.0;
            let mut count = 0usize;
            let mut has_corner_drop = false;
            for cx in region_min_u..=region_max_u {
                if let Some(Some(cy)) = col_max_cy.get(cx) {
                    let u_rel = (cx - region_min_u) as f64 * t.cell + 0.5 * t.cell;
                    let dx = (u_rel - r_arch).clamp(-r_arch, r_arch);
                    let z_expected = (head_above_base - r_arch) + (r_arch * r_arch - dx * dx).max(0.0).sqrt();
                    let z_actual = (cy + 1) as f64 * t.cell;
                    let diff = z_actual - z_expected;
                    sum_sq += diff * diff;
                    count += 1;
                    if (head_above_base - z_actual) >= 0.25 * r_arch {
                        has_corner_drop = true;
                    }
                }
            }
            if count >= 4 && has_corner_drop {
                let rmse = (sum_sq / (count as f64)).sqrt();
                rmse <= 0.03
            } else {
                false
            }
        } else {
            false
        };

        let shape = if is_arched { OpeningShape::Arched { arch_radius: r_arch } } else { OpeningShape::Rectangular };

        // Step 10: jambs in world metres, clamped to the wall ends.
        // from = wall.from + d·(min_u_cell·cell), to = wall.from + d·((max_u_cell+1)·cell).
        let u0 = (region_min_u as f64 * t.cell).clamp(0.0, len);
        let u1 = ((region_max_u + 1) as f64 * t.cell).clamp(0.0, len);
        let from = [wall.from[0] + dux * u0, wall.from[1] + duy * u0];
        let to = [wall.from[0] + dux * u1, wall.from[1] + duy * u1];

        openings_out.push(Opening { wall: wall_index, kind, shape, from, to, thickness: wall.thickness, sill_z, head_z });
    }

    // Step 10b: detect closed doors in candidate wall regions without density drops
    let closed_doors = detect_closed_doors(points, wall, wall_index, &openings_out);
    openings_out.extend(closed_doors);

    // Step 11: sort by (from.x, from.y, kind) for determinism across repeated runs. *(ours)*
    openings_out.sort_by(|a, b| a.from[0].total_cmp(&b.from[0]).then(a.from[1].total_cmp(&b.from[1])).then(a.kind.as_str().cmp(b.kind.as_str())));

    Ok(openings_out)
}

// ---------------------------------------------------------------------------
// detect_all
// ---------------------------------------------------------------------------

/// Detect openings in every wall, using pre-streamed fine points in `fine[i]`.
///
/// A missing band at index `i` (`fine.get(i)` returns `None`) means no fine points were streamed
/// for that wall — not an error. The result is sorted by `(wall, from.x, from.y)` for
/// determinism across repeated runs.
pub fn detect_all(walls: &[Wall], fine: &[Vec<[f64; 3]>], opts: &BimOptions) -> Result<Vec<Opening>, BimError> {
    let mut out: Vec<Opening> = Vec::new();
    for (i, wall) in walls.iter().enumerate() {
        let pts: &[[f64; 3]] = match fine.get(i) {
            Some(v) => v.as_slice(),
            None => &[], // missing band = no fine points, not an error
        };
        let mut found = detect(pts, wall, i, opts)?;
        out.append(&mut found);
    }
    out.sort_by(|a, b| a.wall.cmp(&b.wall).then(a.from[0].total_cmp(&b.from[0])).then(a.from[1].total_cmp(&b.from[1])));
    Ok(out)
}

fn median_f64(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// Inspect candidate wall regions without density drops for micro-relief (recess) characteristic of closed doors.
///
/// A closed door region:
/// - Width in [0.65, 1.20] m
/// - Height in [1.90, 2.25] m
/// - Starts near the floor (sill <= 0.10 m)
/// - Average recess in [0.01, 0.08] m (1 to 8 cm) relative to the surrounding wall face
/// - Does not overlap with existing open openings
pub fn detect_closed_doors(points: &[[f64; 3]], wall: &Wall, wall_index: usize, existing: &[Opening]) -> Vec<Opening> {
    let dx = wall.to[0] - wall.from[0];
    let dy = wall.to[1] - wall.from[1];
    let len = dx.hypot(dy);
    if !len.is_finite() || len < 0.65 || !wall.height.is_finite() || wall.height < 1.90 {
        return Vec::new();
    }
    let (dux, duy) = (dx / len, dy / len);
    let (nx, ny) = (-duy, dux);

    let half_thick = wall.thickness / 2.0;
    let band = 0.10;
    let v_limit = half_thick + band;

    #[derive(Clone, Copy)]
    struct Pt {
        u: f64,
        v: f64,
        z: f64,
    }
    let mut wall_pts: Vec<Pt> = Vec::new();
    for &[px, py, pz] in points {
        if !px.is_finite() || !py.is_finite() || !pz.is_finite() {
            continue;
        }
        let z_rel = pz - wall.base_z;
        if z_rel < 0.0 || z_rel > wall.height {
            continue;
        }
        let rx = px - wall.from[0];
        let ry = py - wall.from[1];
        let u = rx * dux + ry * duy;
        let v = rx * nx + ry * ny;
        if u >= 0.0 && u <= len && v.abs() <= v_limit {
            wall_pts.push(Pt { u, v, z: z_rel });
        }
    }
    if wall_pts.is_empty() {
        return Vec::new();
    }

    let mut excluded_u: Vec<(f64, f64)> = Vec::new();
    for op in existing {
        let u_a = (op.from[0] - wall.from[0]) * dux + (op.from[1] - wall.from[1]) * duy;
        let u_b = (op.to[0] - wall.from[0]) * dux + (op.to[1] - wall.from[1]) * duy;
        let u_min = (u_a.min(u_b) - 0.05).max(0.0);
        let u_max = (u_a.max(u_b) + 0.05).min(len);
        excluded_u.push((u_min, u_max));
    }

    let is_excluded = |u: f64| -> bool { excluded_u.iter().any(|&(lo, hi)| u >= lo && u <= hi) };

    let mut detected: Vec<Opening> = Vec::new();

    for side in [1.0_f64, -1.0_f64] {
        let face_pts: Vec<Pt> =
            wall_pts.iter().copied().filter(|p| side * p.v > 0.0 && (side * p.v) >= half_thick - 0.12 && !is_excluded(p.u)).collect();
        if face_pts.len() < 30 {
            continue;
        }

        let bin_w = 0.02;
        let num_bins = (len / bin_w).ceil() as usize;
        if num_bins == 0 {
            continue;
        }

        let mut bin_mid: Vec<Vec<f64>> = vec![Vec::new(); num_bins];
        let mut bin_top: Vec<Vec<f64>> = vec![Vec::new(); num_bins];
        let mut all_mid_v: Vec<f64> = Vec::new();
        let mut all_top_v: Vec<f64> = Vec::new();

        for p in &face_pts {
            let bi = ((p.u / bin_w).floor() as usize).min(num_bins - 1);
            let val = side * p.v;
            if p.z >= 0.50 && p.z <= 1.80 {
                bin_mid[bi].push(val);
                all_mid_v.push(val);
            } else if p.z >= 2.15 && p.z <= 2.70_f64.min(wall.height) {
                bin_top[bi].push(val);
                all_top_v.push(val);
            }
        }

        if all_mid_v.len() < 20 {
            continue;
        }

        let v_wall_mid_median = match median_f64(all_mid_v) {
            Some(m) => m,
            None => continue,
        };
        let v_wall_top_median = median_f64(all_top_v);

        let mut med_mid: Vec<Option<f64>> = Vec::with_capacity(num_bins);
        let mut med_top: Vec<Option<f64>> = Vec::with_capacity(num_bins);
        for bi in 0..num_bins {
            med_mid.push(median_f64(bin_mid[bi].clone()));
            med_top.push(median_f64(bin_top[bi].clone()));
        }

        let mut bin_is_recessed: Vec<bool> = vec![false; num_bins];

        for bi in 0..num_bins {
            if let Some(m_mid) = med_mid[bi] {
                let v_ref = if let Some(m_top) = med_top[bi] {
                    m_top
                } else if let Some(top_med) = v_wall_top_median {
                    top_med
                } else {
                    v_wall_mid_median
                };
                let diff = v_ref - m_mid;
                if (0.008..=0.085).contains(&diff) {
                    bin_is_recessed[bi] = true;
                }
            }
        }

        let mut start_bin: Option<usize> = None;
        #[allow(clippy::needless_range_loop)]
        for bi in 0..=num_bins {
            let is_rec = if bi < num_bins { bin_is_recessed[bi] } else { false };
            if is_rec {
                if start_bin.is_none() {
                    start_bin = Some(bi);
                }
            } else if let Some(s) = start_bin {
                let e = bi;
                start_bin = None;
                let u_start = s as f64 * bin_w;
                let u_end = e as f64 * bin_w;
                let width = u_end - u_start;

                if !(0.65..=1.20).contains(&width) {
                    continue;
                }

                let door_pts: Vec<Pt> = face_pts.iter().copied().filter(|p| p.u >= u_start && p.u <= u_end).collect();
                if door_pts.is_empty() {
                    continue;
                }

                let min_z = door_pts.iter().map(|p| p.z).fold(f64::INFINITY, f64::min);
                if min_z > 0.20 {
                    continue;
                }

                let body_pts: Vec<f64> = door_pts.iter().filter(|p| p.z >= 0.30 && p.z <= 1.80).map(|p| side * p.v).collect();
                let Some(v_door) = median_f64(body_pts) else {
                    continue;
                };

                let mut best_h: Option<f64> = None;
                let mut best_step = 0.0f64;
                for z_step in (190..=225).step_by(2) {
                    let z_cand = z_step as f64 / 100.0;
                    let below_pts: Vec<f64> = door_pts.iter().filter(|p| p.z >= z_cand - 0.10 && p.z < z_cand).map(|p| side * p.v).collect();
                    let above_pts: Vec<f64> =
                        door_pts.iter().filter(|p| p.z >= z_cand && p.z <= (z_cand + 0.10).min(wall.height)).map(|p| side * p.v).collect();
                    if !below_pts.is_empty() && !above_pts.is_empty() {
                        let mean_b = below_pts.iter().sum::<f64>() / (below_pts.len() as f64);
                        let mean_a = above_pts.iter().sum::<f64>() / (above_pts.len() as f64);
                        let step = mean_a - mean_b;
                        if step > best_step {
                            best_step = step;
                            best_h = Some(z_cand);
                        }
                    }
                }
                let found_h = if best_step >= 0.008 { best_h } else { None };

                let left_pts: Vec<f64> = face_pts
                    .iter()
                    .filter(|p| p.u >= (u_start - 0.25).max(0.0) && p.u < u_start && p.z >= 0.50 && p.z <= 1.80)
                    .map(|p| side * p.v)
                    .collect();
                let right_pts: Vec<f64> = face_pts
                    .iter()
                    .filter(|p| p.u > u_end && p.u <= (u_end + 0.25).min(len) && p.z >= 0.50 && p.z <= 1.80)
                    .map(|p| side * p.v)
                    .collect();

                let step_left = median_f64(left_pts).map(|vl| vl - v_door);
                let step_right = median_f64(right_pts).map(|vr| vr - v_door);

                let has_jamb_step = step_left.is_some_and(|s| s >= 0.008) || step_right.is_some_and(|s| s >= 0.008);
                let has_lintel_step = found_h.is_some();

                if !has_lintel_step && !has_jamb_step {
                    continue;
                }
                if wall.height >= 2.25 && !has_lintel_step {
                    continue;
                }

                let detected_h = found_h.unwrap_or(2.10_f64.min(wall.height));
                if !(1.90..=2.25).contains(&detected_h) {
                    continue;
                }

                let mut surrounding_face: Vec<f64> = Vec::new();
                for p in &door_pts {
                    if p.z >= detected_h && p.z <= (detected_h + 0.35).min(wall.height) {
                        surrounding_face.push(side * p.v);
                    }
                }
                for p in &face_pts {
                    if p.z >= 0.50
                        && p.z <= 1.80
                        && ((p.u >= (u_start - 0.25).max(0.0) && p.u < u_start) || (p.u > u_end && p.u <= (u_end + 0.25).min(len)))
                    {
                        surrounding_face.push(side * p.v);
                    }
                }

                let v_surround = median_f64(surrounding_face).unwrap_or(v_wall_mid_median);
                let avg_recess = v_surround - v_door;

                if (0.01..=0.08).contains(&avg_recess) {
                    let from = [wall.from[0] + dux * u_start, wall.from[1] + duy * u_start];
                    let to = [wall.from[0] + dux * u_end, wall.from[1] + duy * u_end];
                    detected.push(Opening {
                        wall: wall_index,
                        kind: OpeningKind::ClosedDoor { recess_depth: avg_recess },
                        shape: OpeningShape::Rectangular,
                        from,
                        to,
                        thickness: wall.thickness,
                        sill_z: wall.base_z,
                        head_z: wall.base_z + detected_h,
                    });
                }
            }
        }
    }

    let get_u_mid = |op: &Opening| -> f64 {
        let u_from = (op.from[0] - wall.from[0]) * dux + (op.from[1] - wall.from[1]) * duy;
        let u_to = (op.to[0] - wall.from[0]) * dux + (op.to[1] - wall.from[1]) * duy;
        (u_from + u_to) * 0.5
    };

    let get_recess = |op: &Opening| -> f64 {
        match op.kind {
            OpeningKind::ClosedDoor { recess_depth } => recess_depth,
            _ => 0.0,
        }
    };

    let mut deduped: Vec<Opening> = Vec::new();
    for cand in detected {
        let u_cand = get_u_mid(&cand);
        let rec_cand = get_recess(&cand);
        if let Some(existing_pos) = deduped.iter().position(|d| (get_u_mid(d) - u_cand).abs() <= 0.30) {
            if rec_cand > get_recess(&deduped[existing_pos]) {
                deduped[existing_pos] = cand;
            }
        } else {
            deduped.push(cand);
        }
    }
    deduped.sort_by(|a, b| get_u_mid(a).total_cmp(&get_u_mid(b)));
    deduped
}

// ---------------------------------------------------------------------------
// Tests (TDD: written before implementation, run RED, then GREEN)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bim::BimOptions;

    fn wall() -> Wall {
        Wall { from: [0.0, 0.0], to: [4.0, 0.0], thickness: 0.20, base_z: 0.0, height: 3.0, exterior: false, is_parapet: false, arc: None }
    }

    /// Points on both faces of the wall, on a 2 cm grid, skipping the two holes.
    fn wall_with_a_door_and_a_window() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        for face in [-0.1_f64, 0.1] {
            let mut x = 0.01_f64;
            while x < 4.0 {
                let mut z = 0.01_f64;
                while z < 3.0 {
                    let in_door = (1.5..=2.4).contains(&x) && z <= 2.1;
                    let in_window = (2.8..=3.6).contains(&x) && (1.0..=2.0).contains(&z);
                    if !in_door && !in_window {
                        pts.push([x, face, z]);
                    }
                    z += 0.02;
                }
                x += 0.02;
            }
        }
        pts
    }

    // ----- Step 1: RED → GREEN -----
    #[test]
    fn a_door_and_a_window_are_found_and_classified_within_the_acceptance_size() {
        let o = BimOptions::default();
        let got = detect(&wall_with_a_door_and_a_window(), &wall(), 0, &o).expect("the wall has openings");
        assert_eq!(got.len(), 2, "one door and one window, not the scan cracks: {got:?}");
        let door = got.iter().find(|o| o.kind == OpeningKind::Door).expect("a door");
        let window = got.iter().find(|o| o.kind == OpeningKind::Window).expect("a window");
        assert_eq!(door.shape, OpeningShape::Rectangular);
        assert_eq!(window.shape, OpeningShape::Rectangular);
        assert!((door.width() - 0.9).abs() <= 0.025, "door width {}, acceptance 25 mm", door.width());
        assert!((door.height() - 2.1).abs() <= 0.025, "door height {}", door.height());
        assert!(door.sill_z.abs() <= 0.025, "a door starts at the floor, sill {}", door.sill_z);
        assert!((window.width() - 0.8).abs() <= 0.025, "window width {}", window.width());
        assert!((window.sill_z - 1.0).abs() <= 0.025, "window sill {}", window.sill_z);
        assert!((window.head_z - 2.0).abs() <= 0.025, "window head {}", window.head_z);
        assert_eq!(door.wall, 0);
        assert!((door.thickness - 0.20).abs() < 1e-9);
    }

    // ----- Step 2: density spikes cannot defeat the rank-10 threshold *(paper)* -----
    #[test]
    fn density_spikes_do_not_defeat_the_rank_10_threshold() {
        let mut pts = wall_with_a_door_and_a_window();
        // Nine spike cells on the solid wall, each ten times denser than the faces: the paper's
        // max10 reference is the tenth densest cell, so up to nine outliers can outrank the wall
        // and the reference still describes the wall. A stage that used the tallest bin would
        // take 22 as the reference and call the whole wall empty. The positions avoid both holes.
        for &(ux, uz) in
            &[(0.10_f64, 1.5_f64), (0.30, 1.5), (0.50, 1.5), (0.70, 1.5), (0.90, 1.5), (1.10, 1.5), (1.30, 1.5), (2.70, 1.5), (3.90, 1.5)]
        {
            for face in [-0.1_f64, 0.1] {
                for _ in 0..10 {
                    pts.push([ux, face, uz]);
                }
            }
        }
        let o = BimOptions::default();
        let got = detect(&pts, &wall(), 0, &o).expect("spikes must not crash the detector");
        assert_eq!(got.len(), 2, "density spikes must not hide the openings: {got:?}");
        let door = got.iter().find(|o| o.kind == OpeningKind::Door).expect("a door");
        let window = got.iter().find(|o| o.kind == OpeningKind::Window).expect("a window");
        assert_eq!(door.shape, OpeningShape::Rectangular);
        assert_eq!(window.shape, OpeningShape::Rectangular);
        assert!((door.width() - 0.9).abs() <= 0.025, "door width still correct under spikes: {}", door.width());
        assert!((window.width() - 0.8).abs() <= 0.025, "window width still correct: {}", window.width());
    }

    // ----- Step 3: solid wall has no openings, 4 cm crack is not one -----
    #[test]
    fn a_solid_wall_has_no_openings_and_a_two_cell_crack_closes() {
        let o = BimOptions::default();
        let mut pts = Vec::new();
        for face in [-0.1_f64, 0.1] {
            for i in 0..200 {
                for k in 0..150 {
                    pts.push([0.01 + 0.02 * f64::from(i), face, 0.01 + 0.02 * f64::from(k)]);
                }
            }
        }
        assert!(detect(&pts, &wall(), 0, &o).expect("a solid wall").is_empty());
        // Remove two columns (4 cm) around u = 2.0: the radius-1 close bridges them.
        let cut: Vec<[f64; 3]> = pts.into_iter().filter(|p| !(1.98..=2.02).contains(&p[0])).collect();
        assert!(detect(&cut, &wall(), 0, &o).expect("a crack is not a door").is_empty(), "a 4 cm crack is scan noise");
    }

    // ----- Step 4: band covers the wall body plus the reading margin -----
    #[test]
    fn the_band_is_the_wall_body_with_the_reading_margin() {
        let b = band(&wall(), &BimOptions::default()).expect("a wall has a band");
        assert!((b.min[0] + 0.05).abs() < 1e-9 && (b.max[0] - 4.05).abs() < 1e-9, "u runs past both ends");
        assert!((b.min[1] + 0.15).abs() < 1e-9 && (b.max[1] - 0.15).abs() < 1e-9, "the body is 0.20 thick plus 0.05 a side");
        assert!(b.z0.abs() < 1e-9 && (b.z1 - 3.0).abs() < 1e-9);
        assert!(band(&Wall { from: [1.0, 1.0], to: [1.0, 1.0], ..wall() }, &BimOptions::default()).is_none(), "a wall with no length has no band");
    }

    // ----- Step 5: hostile input never panics -----
    #[test]
    fn hostile_input_never_panics_and_an_extreme_rank_is_clamped() {
        let o = BimOptions::default();
        let wall = wall();

        // NaN coordinates in the point list: the point is skipped, no panic.
        let nan_pts = vec![[f64::NAN, 0.0, 1.0], [0.5, 0.0, 1.0]];
        assert!(detect(&nan_pts, &wall, 0, &o).is_ok(), "NaN points are skipped, not fatal");

        // A NaN option → InvalidCell.
        let bad_cell = BimOptions { opening_cell: f64::NAN, ..o };
        assert!(matches!(detect(&nan_pts, &wall, 0, &bad_cell), Err(BimError::InvalidCell)), "a NaN cell is InvalidCell");

        // Zero option → InvalidCell.
        let zero_cell = BimOptions { opening_cell: 0.0, ..o };
        assert!(matches!(detect(&nan_pts, &wall, 0, &zero_cell), Err(BimError::InvalidCell)), "a zero cell is InvalidCell");

        // Empty point list → Ok with no openings.
        assert!(detect(&[], &wall, 0, &o).expect("empty is fine").is_empty());

        // opening_rank = usize::MAX through the struct: the rank is clamped to the density count,
        // so no allocation grows proportional to MAX, and no panic occurs. *(paper)*
        let huge_rank = BimOptions { opening_rank: usize::MAX, ..o };
        let pts = wall_with_a_door_and_a_window();
        // No panic is the contract; the result may have openings or be empty.
        let _res = detect(&pts, &wall, 0, &huge_rank);
    }

    // ----- detect_all: a missing band is treated as no points, not an error -----
    #[test]
    fn detect_all_treats_a_missing_band_as_no_points() {
        let walls = vec![wall()];
        let fine: Vec<Vec<[f64; 3]>> = vec![];
        let o = BimOptions::default();
        let result = detect_all(&walls, &fine, &o).expect("missing band is not an error");
        assert!(result.is_empty());
    }

    #[test]
    fn arched_opening_fitting_detects_semicircle_profile() {
        let o = BimOptions::default();
        let mut pts = Vec::new();
        // Wall 4.0m long, 3.0m high, 0.20m thick (faces at y = -0.10, +0.10)
        // Arched doorway: x in [1.50, 2.50] (width = 1.0 m, radius = 0.50 m)
        // Rectangular part: z in [0.0, 1.70]
        // Semicircle arch top: center at (2.0, 1.70), radius 0.50 m, peak at z = 2.20
        for face in [-0.10_f64, 0.10] {
            let mut x = 0.01_f64;
            while x < 4.0 {
                let mut z = 0.01_f64;
                while z < 3.0 {
                    let in_rect = (1.50..=2.50).contains(&x) && z <= 1.70;
                    let in_arch = (1.50..=2.50).contains(&x) && z > 1.70 && {
                        let dx = x - 2.0;
                        let dz = z - 1.70;
                        dx * dx + dz * dz <= 0.50 * 0.50
                    };
                    if !in_rect && !in_arch {
                        pts.push([x, face, z]);
                    }
                    z += 0.02;
                }
                x += 0.02;
            }
        }

        let got = detect(&pts, &wall(), 0, &o).expect("arched door detected");
        assert_eq!(got.len(), 1, "exactly one arched doorway: {got:?}");
        let door = &got[0];
        assert_eq!(door.kind, OpeningKind::Door);
        match door.shape {
            OpeningShape::Arched { arch_radius } => {
                assert!((arch_radius - 0.50).abs() <= 0.03, "arch radius ~0.50 m, got {arch_radius}");
            }
            OpeningShape::Rectangular => panic!("expected Arched shape, got Rectangular"),
        }
        assert!((door.width() - 1.0).abs() <= 0.03, "width ~1.0 m: {}", door.width());
        assert!((door.head_z - 2.20).abs() <= 0.04, "head ~2.20 m: {}", door.head_z);
    }

    #[test]
    fn closed_door_detection_from_micro_relief() {
        let o = BimOptions::default();
        let mut pts = Vec::new();
        // Solid wall with NO empty hole (no density drop).
        // Wall faces normally at y = -0.10 and y = +0.10.
        // Closed door on face y = +0.10:
        // x in [1.50, 2.40] (width 0.90 m), z in [0.0, 2.10] (height 2.10 m)
        // Door leaf is recessed by 0.03 m (3 cm) -> y = 0.07 m!
        // Above z = 2.10 m in [1.50, 2.40], y = 0.10 m (solid wall face).
        // Face y = -0.10 is completely flat at y = -0.10.
        for i in 0..200 {
            let x = 0.01 + (i as f64) * 0.02;
            for k in 0..150 {
                let z = 0.01 + (k as f64) * 0.02;
                // Back face
                pts.push([x, -0.10, z]);

                // Front face
                let is_door = (1.50..=2.40).contains(&x) && z <= 2.10;
                let y = if is_door { 0.07 } else { 0.10 };
                pts.push([x, y, z]);
            }
        }

        let got = detect(&pts, &wall(), 0, &o).expect("closed door detection");
        assert_eq!(got.len(), 1, "exactly one closed door detected: {got:?}");
        let door = &got[0];
        match door.kind {
            OpeningKind::ClosedDoor { recess_depth } => {
                assert!((recess_depth - 0.03).abs() <= 0.01, "recess ~0.03 m: {recess_depth}");
            }
            _ => panic!("expected ClosedDoor, got {:?}", door.kind),
        }
        assert_eq!(door.shape, OpeningShape::Rectangular);
        assert!((door.width() - 0.90).abs() <= 0.03, "width ~0.90 m: {}", door.width());
        assert!((door.height() - 2.10).abs() <= 0.03, "height ~2.10 m: {}", door.height());
    }

    #[test]
    fn dual_face_scanned_closed_door_does_not_duplicate() {
        let o = BimOptions::default();
        let mut pts = Vec::new();
        // Wall faces at y = -0.10 and y = +0.10.
        // Recessed door at x in [1.50, 2.40], z in [0.0, 2.10]:
        // Front face: recess 0.03 m (y = 0.07 m)
        // Back face: recess 0.05 m (y = -0.05 m)
        for i in 0..200 {
            let x = 0.01 + (i as f64) * 0.02;
            for k in 0..150 {
                let z = 0.01 + (k as f64) * 0.02;
                let is_door = (1.50..=2.40).contains(&x) && z <= 2.10;

                // Back face (y = -0.10 normal, -0.05 recessed -> 0.05 m recess)
                let y_back = if is_door { -0.05 } else { -0.10 };
                pts.push([x, y_back, z]);

                // Front face (y = +0.10 normal, 0.07 recessed -> 0.03 m recess)
                let y_front = if is_door { 0.07 } else { 0.10 };
                pts.push([x, y_front, z]);
            }
        }

        let got = detect(&pts, &wall(), 0, &o).expect("closed door detection");
        assert_eq!(got.len(), 1, "exactly one closed door detected despite dual-face scan: {got:?}");
        let door = &got[0];
        match door.kind {
            OpeningKind::ClosedDoor { recess_depth } => {
                assert!((recess_depth - 0.05).abs() <= 0.01, "keeps larger recess ~0.05 m: {recess_depth}");
            }
            _ => panic!("expected ClosedDoor, got {:?}", door.kind),
        }
    }
}
