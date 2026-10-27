//! The horizontal surfaces: the vertical Z histogram, its faces, and the slabs they pair into.
//!
//! A scan of a building spends most of its points on walls and on whatever stands on the floor,
//! and a small, huge fraction of them on the few horizontal planes: the floor, the underside and
//! top of each slab, the ceiling. Binning every point's Z at `z_step` and keeping the bins that
//! hold at least `slab_ratio` of the tallest bin is the paper's slab detector (arXiv:2503.11498
//! §3.1, Figure 3): the peaks are the surfaces, the rest is furniture.
//!
//! Nothing here is measured from a file: the stage takes the plain points its caller already holds
//! and hands back owned geometry, so the crate keeps compiling for wasm.

use std::collections::{HashMap, VecDeque};

use crate::bim::columns::convex_hull_2d;
use crate::bim::raster::{Raster, close, contours, mask};
use crate::bim::{BimError, BimOptions, MAX_CLOSE_RADIUS, MAX_RASTER_CELLS};
use cadcraft_geom::{Vec2, point_in_polygon, shoelace};

#[cfg(test)]
pub(crate) mod fixture;

/// The largest vertical histogram this stage will build.
///
/// Bin count is `(max_z - min_z) / z_step`, so a cloud whose Z spans a kilometre at the paper's
/// `z_step` asks for twenty thousand counters and a cloud that spans a hundred kilometres asks
/// for two million. `MAX_Z_BINS` is the point where the stage stops guessing: 20 000 bins is
/// 999.95 m of building at `z_step = 0.05`, because the count includes both end bins — a 1 000 m
/// band needs 20 001 and is refused. The quotient is compared in `f64` *before* it is cast, so a
/// hostile range is refused rather than allocated.
pub const MAX_Z_BINS: usize = 20_000;

/// A sub-region of a slab at a distinct elevation or inclination (e.g. split-levels or ramps).
#[derive(Debug, Clone, PartialEq)]
pub struct SlabRegion {
    pub polygon: Vec<[f64; 2]>,
    pub base_z: f64,
    pub top_z: f64,
    pub is_sloped: bool,
    pub slope_deg: f64,
}

/// A flight of stairs detected between storeys.
#[derive(Debug, Clone, PartialEq)]
pub struct Stair {
    pub storey_index: usize,
    pub base_z: f64,
    pub top_z: f64,
    pub num_risers: usize,
    pub riser_height: f64,
    pub tread_depth: f64,
    pub flight_axis: [[f64; 2]; 2],
    pub width: f64,
}

/// One horizontal surface found in the cloud, and the plan shape that goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Slab {
    /// The underside. For a single-sided slab it is synthesised (see [`find`]), because only one
    /// face of the body was scanned.
    pub base_z: f64,
    /// The top surface. For a single-sided slab it is synthesised the same way.
    pub top_z: f64,
    /// `top_z - base_z` for a paired slab — the measured gap — and `single_sided_slab_thickness`
    /// for the others.
    pub thickness: f64,
    /// The slab's plan ring in world metres: the cell-corner ring of [`contours`] turned over by
    /// [`Raster::to_world`], with the first corner not repeated. Empty when the band that would
    /// draw it holds nothing.
    pub polygon: Vec<[f64; 2]>,
    /// Any inner voids / openings (e.g. stairwells, vertical shafts) contained within `polygon`.
    pub holes: Vec<Vec<[f64; 2]>>,
    /// True when only one face of this body was scanned, so the other face is the option's
    /// thickness rather than a measurement.
    pub single_sided: bool,
    /// Any sub-regions at distinct elevations or inclinations (split-levels / ramps).
    pub regions: Vec<SlabRegion>,
}

/// The scan's horizontal surfaces, ordered by [`Slab::base_z`].
///
/// The paper's rules (§3.1), in the order this function applies them:
///
/// 1. a bin holding at least `slab_ratio` × the tallest bin is a candidate, and a run of adjacent
///    candidate bins is one face, standing at the mean Z of the points that made it. A lone bin
///    below the cut that still holds points, with a candidate on each side, joins the run: a rough
///    plane is a hump in the histogram, not a plateau, and the cut can fall through one trough in
///    it (see [`faces`]);
/// 2. *(paper, rule 1)* the lowest face is the ground or a partially scanned slab, seen only from
///    above, so it is `single_sided` and its body hangs `single_sided_slab_thickness` **below** it;
/// 3. *(paper, rule 2)* the faces above it pair bottom-up, two and two, into slabs whose thickness
///    is the measured gap and whose plan is the raster of **both** faces merged — the paper's
///    reason for merging is that the slab mirrors the ceiling shape of the floor below, so
///    whichever face the furniture erased, the other still describes;
/// 4. *(paper, rule 3)* a last face with no partner is an unscanned roof or a ceiling: it is
///    `single_sided` too, and its body stands `single_sided_slab_thickness` **above** it.
///
/// A leftover face whose partner fell under the `slab_ratio` cut — furniture can occlude a slab's
/// lower surface — is read by rule 4 as a ceiling with its body above it. The room underneath then
/// extends into the real slab body: [`crate::bim::storeys::split`]'s ceiling sits inside it, and
/// the wall stage's top slice meets slab points there. That is the pairing's doing, not the
/// direction rule's, and it is pinned by
/// `a_slab_face_erased_past_the_ratio_cut_is_read_as_a_ceiling_not_a_slab`.
///
/// Which side a single-sided body lies on is **ours**, not the paper's: it names the case (the
/// `bfs`/`tfs` thickness) and never says which way the slab goes, and [`BimOptions`] carries the
/// thickness without a direction. The rule is that a surface is scanned from the void side: the
/// lowest surface in a band is the one you stand on, and anything above you without a partner is a
/// ceiling or a roof. The storey split depends on it, because a storey is the void between
/// consecutive slabs — put the ground slab's body above its face and the first room starts in the
/// air two decimetres over the floor.
///
/// A point with a coordinate that is not a number is skipped and never reaches a bin; the count of
/// the points that did is what decides [`BimError::Empty`], and the plan stage reports a grid that
/// will not fit as its own refusal rather than as an empty polygon.
///
/// # Errors
///
/// * [`BimError::InvalidCell`] when a number this stage cannot work with is in the options.
/// * [`BimError::TooLarge`] when an allocation or a loop bound would be sized from a hostile
///   coordinate range or option: the z histogram, the slab raster, the close radius.
/// * [`BimError::Empty`] when no face is found: no points, no point with three finite coordinates,
///   or nothing tall enough to be a surface.
///
/// The refusals are decided in a fixed order — the close radius, then the other option numbers,
/// then the z histogram — and the first one that fails is the error, so a cloud that is hostile in
/// two ways reports the earlier one.
pub fn find(points: &[[f64; 3]], opts: &BimOptions) -> Result<Vec<Slab>, BimError> {
    let tuning = Tuning::read(opts)?;
    let faces = faces(points, &tuning)?;
    let mut slabs: Vec<Slab> = Vec::new();
    for body in bodies(&faces) {
        let Some(mut slab) = body.slab(&tuning) else {
            continue;
        };
        let (poly, holes) = plan(points, &body.faces, &tuning)?;
        slab.polygon = poly;
        slab.holes = holes;
        slabs.push(slab);
    }
    Ok(slabs)
}

/// The six numbers this stage reads out of [`BimOptions`], read once and refused once.
///
/// [`find`] checks them before it looks at a single point, so nothing below re-checks them and no
/// hostile option ever reaches an allocation or a loop bound. An unusable number is
/// [`BimError::InvalidCell`] — the crate's word for a parameter that cannot be used, not only for
/// a cell size — never the default and never a panic (amendment B5). `close_radius` answers
/// [`BimError::TooLarge`] instead, because a radius is the width of the morphological pass's
/// window: [`BimOptions::from_params`](crate::bim::BimOptions::from_params) already refuses the
/// same value on the JSON path, and a stage handed a directly constructed `BimOptions` has to
/// refuse it too rather than walk a grid for a century.
struct Tuning {
    /// The histogram bin size and the plan's band half-width, in metres (`z_step`).
    z_step: f64,
    /// The plan grid's cell, in metres (`cell`).
    cell: f64,
    /// The thickness given to a slab whose second face was never scanned.
    thickness: f64,
    /// A face bin must hold at least this times the tallest bin (`slab_ratio`).
    slab_ratio: f64,
    /// A grid cell must hold at least this times the tallest cell to be in the plan (`threshold`).
    threshold: f64,
    /// The morphological close radius, in cells, already inside its budget.
    close_radius: usize,
}

impl Tuning {
    fn read(opts: &BimOptions) -> Result<Self, BimError> {
        if opts.close_radius > MAX_CLOSE_RADIUS {
            return Err(BimError::TooLarge { what: "close radius", limit: MAX_CLOSE_RADIUS });
        }
        Ok(Tuning {
            z_step: usable(opts.z_step)?,
            cell: usable(opts.cell)?,
            thickness: usable(opts.single_sided_slab_thickness)?,
            // `threshold` is the one number allowed to be zero, for the reason [`ratio`] gives; a
            // `slab_ratio` of zero would read the wall levels instead of the surfaces, so it is
            // refused like a length.
            slab_ratio: usable(opts.slab_ratio)?,
            threshold: ratio(opts.threshold)?,
            close_radius: opts.close_radius,
        })
    }
}

/// A length that can be worked with: finite, and above zero.
fn usable(v: f64) -> Result<f64, BimError> {
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(BimError::InvalidCell) }
}

/// A cut expressed as a fraction: finite and not negative. Zero means "no density filter", which
/// [`mask`] offers deliberately, so only the numbers that are not numbers are refused.
fn ratio(v: f64) -> Result<f64, BimError> {
    if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(BimError::InvalidCell) }
}

/// A slab body as the pairing left it: the faces that bound it — one for a single-sided slab, two
/// for a measured one — and which way its synthesised side goes.
struct Body {
    /// The face heights, ascending. Never empty, because [`bodies`] only makes one from a face.
    faces: Vec<f64>,
    single_sided: bool,
    /// For a single-sided body: true when the face is the *top* of the slab (a floor you stand
    /// on), false when it is the *bottom* of it (a ceiling above you). Ignored once paired.
    floor_side: bool,
}

/// The paper's pairing, bottom-up, over faces that [`faces`] already put in ascending order.
fn bodies(faces: &[f64]) -> Vec<Body> {
    let Some((first, rest)) = faces.split_first() else {
        return Vec::new();
    };
    // Paper rule 1: the lowest surface is the ground slab, and a scanner standing on it never
    // sees its underside.
    let mut out: Vec<Body> = Vec::new();
    out.push(Body { faces: vec![*first], single_sided: true, floor_side: true });
    let mut i = 0;
    while i < rest.len() {
        match (rest.get(i), rest.get(i + 1)) {
            // Paper rule 2: two faces with nothing between them are one slab, measured.
            (Some(a), Some(b)) => {
                out.push(Body { faces: vec![*a, *b], single_sided: false, floor_side: false });
                i += 2;
            }
            // Paper rule 3: a last face with no partner needs the manual thickness. B3: it is a
            // ceiling or a roof, because a surface is scanned from the void side and the scanner
            // stands under it — so its body stands above it, unlike the lowest face's.
            (Some(z), None) => {
                out.push(Body { faces: vec![*z], single_sided: true, floor_side: false });
                i += 1;
            }
            (None, _) => break,
        }
    }
    out
}

impl Body {
    /// The body with its bounds and thickness, before the plan is rasterised. `None` when the
    /// pairing went wrong and the face is missing, which [`bodies`] does not produce: an invented
    /// slab is worse than a dropped one.
    fn slab(&self, tuning: &Tuning) -> Option<Slab> {
        let t = tuning.thickness;
        let (base_z, top_z, thickness) = match (self.faces.first(), self.faces.get(1)) {
            (Some(a), Some(b)) => (*a, *b, *b - *a),
            (Some(a), None) => {
                if self.floor_side {
                    (*a - t, *a, t)
                } else {
                    (*a, *a + t, t)
                }
            }
            (None, _) => return None,
        };
        Some(Slab { base_z, top_z, thickness, polygon: Vec::new(), holes: Vec::new(), single_sided: self.single_sided, regions: Vec::new() })
    }
}

/// The face heights, ascending: the Z histogram, the candidate cut, the runs merged, the means.
fn faces(points: &[[f64; 3]], tuning: &Tuning) -> Result<Vec<f64>, BimError> {
    let step = tuning.z_step;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut counted = 0usize;
    for p in points.iter().filter(|p| finite(p)) {
        counted += 1;
        if p[2] < lo {
            lo = p[2];
        }
        if p[2] > hi {
            hi = p[2];
        }
    }
    if counted == 0 {
        return Err(BimError::Empty);
    }
    // Amendment B5, and the reason the count is decided here rather than at the cast: the bin
    // count comes straight out of the coordinate range, so a band spanning kilometres asks for
    // millions of counters. The quotient is compared in `f64` *before* it becomes a `usize`, the
    // way raster's `cells_across` does, so a hostile range is refused and never allocated. Both
    // end bins count, so the budget is the 999.95 m band at the paper's `z_step` and a 1 000 m
    // band is refused; `hi - lo` of two finite numbers that overflows to infinity is refused by
    // the same test.
    let bins = ((hi - lo) / step).max(0.0) + 1.0;
    if !bins.is_finite() || bins > MAX_Z_BINS as f64 {
        return Err(BimError::TooLarge { what: "z histogram", limit: MAX_Z_BINS });
    }
    // Safe now: finite, at least 1.0, at most MAX_Z_BINS.
    let bins = bins as usize;
    let mut hist = vec![0u32; bins];
    let mut zsum = vec![0f64; bins];
    for p in points.iter().filter(|p| finite(p)) {
        let Some(i) = bin(p[2], lo, step, bins) else {
            continue;
        };
        if let Some(c) = hist.get_mut(i) {
            *c = c.saturating_add(1);
        }
        if let Some(s) = zsum.get_mut(i) {
            *s += p[2];
        }
    }
    // The cut is a multiply, so no division by zero is possible; a bin that holds nothing is never
    // a candidate even at a `slab_ratio` of zero.
    let tallest = hist.iter().copied().max().unwrap_or(0);
    if tallest == 0 {
        return Err(BimError::Empty);
    }
    let cut = f64::from(tallest) * tuning.slab_ratio;
    // A real plane is a hump, not a plateau: scan noise spreads the floor of a 41.7-million-point
    // building over a decimetre, and the tallest bin belongs to another surface (the ceiling), so
    // the 50 % cut can fall through one trough inside the hump. Applied bin by bin it splits that
    // floor into two faces, the upper half pairs with the ceiling and the stage reports a slab
    // metres thick instead of the room it stands in. So a lone bin below the cut that still holds
    // points, with a candidate on each side, is the flank of the same surface and joins the run.
    // Both neighbours are what keeps two real faces apart: a gap of one *empty* bin, or of two
    // bins at all, still ends the run, so a 15 cm slab is still two faces.
    let over_cut = |i: usize| hist.get(i).copied().is_some_and(|c| c > 0 && f64::from(c) >= cut);
    let mut in_run = vec![false; bins];
    for i in 0..bins {
        let bridge = i > 0 && i + 1 < bins && over_cut(i - 1) && over_cut(i + 1) && hist.get(i).copied().is_some_and(|c| c > 0);
        if let Some(slot) = in_run.get_mut(i) {
            *slot = over_cut(i) || bridge;
        }
    }
    let mut out: Vec<f64> = Vec::new();
    let mut run = None;
    let mut last = 0usize;
    for i in 0..bins {
        if in_run.get(i).copied().unwrap_or(false) {
            if run.is_none() {
                run = Some(i);
            }
            last = i;
        } else if let Some(from) = run.take() {
            push_face(&mut out, &hist, &zsum, from, last, lo, step);
        }
    }
    if let Some(from) = run.take() {
        push_face(&mut out, &hist, &zsum, from, last, lo, step);
    }
    if out.is_empty() {
        return Err(BimError::Empty);
    }
    Ok(out)
}

/// One face: the mean Z of every point in the run of bins `from ..= last`, which for a plane
/// scanned at any real thickness is the plane. Falls back to the middle of the run's bin range
/// when the running sum was poisoned by coordinates too large to add — a finite answer either
/// way, and never a panic.
fn push_face(out: &mut Vec<f64>, hist: &[u32], zsum: &[f64], from: usize, last: usize, lo: f64, step: f64) {
    let mut count = 0u64;
    let mut sum = 0f64;
    for i in from..=last {
        count += u64::from(hist.get(i).copied().unwrap_or(0));
        sum += zsum.get(i).copied().unwrap_or(0.0);
    }
    let centre = lo + (from as f64 + last as f64 + 1.0) * 0.5 * step;
    let z = if count > 0 && sum.is_finite() { sum / count as f64 } else { centre };
    if z.is_finite() {
        out.push(z);
    }
}

/// The bin a height lands in, clamped to the histogram it is being put into.
fn bin(z: f64, lo: f64, step: f64, bins: usize) -> Option<usize> {
    let d = (z - lo) / step;
    if !d.is_finite() || d < 0.0 {
        return if d.is_finite() { Some(0) } else { None };
    }
    bins.checked_sub(1).map(|last| (d.floor() as usize).min(last))
}

/// The plan ring of a slab: the points lying within one `z_step` of any of its faces, rasterised
/// together, closed and traced, with the most prominent outer ring kept (§3.1: "the most prominent
/// contour is extracted, scaled, and converted into a Polygon object that delineates the floor
/// plan"). Holes are not carried: a slab is one ring here.
/// The plan ring of a slab and its interior holes: the points lying within one `z_step` of any of its
/// faces, rasterised together, closed and traced. The most prominent outer ring delineates the floor
/// plan, and inner contours contained within it delineate voids / slab openings.
fn plan(points: &[[f64; 3]], faces: &[f64], tuning: &Tuning) -> Result<(Vec<[f64; 2]>, Vec<Vec<[f64; 2]>>), BimError> {
    let step = tuning.z_step;
    // Two passes rather than a band vector: the working level of a real scan is millions of
    // points, and this runs once per slab.
    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut in_band = 0usize;
    for p in points.iter().filter(|p| finite(p)) {
        if faces.iter().any(|f| (p[2] - *f).abs() <= step) {
            in_band += 1;
            if p[0] < min[0] {
                min[0] = p[0];
            }
            if p[1] < min[1] {
                min[1] = p[1];
            }
            if p[0] > max[0] {
                max[0] = p[0];
            }
            if p[1] > max[1] {
                max[1] = p[1];
            }
        }
    }
    if in_band == 0 {
        // No point sits within a step of the measured face, so there is no plan to draw. Nothing
        // is invented and the caller sees an empty ring.
        return Ok((Vec::new(), Vec::new()));
    }
    // A band one point deep, or one cell narrower than a cell, still needs a box to grid onto.
    let lo = [min[0], min[1]];
    let hi = [min[0] + (max[0] - min[0]).max(tuning.cell), min[1] + (max[1] - min[1]).max(tuning.cell)];
    let Some(mut grid) = Raster::new(lo, hi, tuning.cell) else {
        // Amendment B6: a grid that will not fit is an error, not an empty polygon. An empty
        // polygon silently draws nothing, and the caller could then not tell a scan a hundred
        // kilometres across from an empty one.
        return Err(BimError::TooLarge { what: "slab raster", limit: MAX_RASTER_CELLS });
    };
    for p in points.iter().filter(|p| finite(p)) {
        if faces.iter().any(|f| (p[2] - *f).abs() <= step) {
            grid.add(*p);
        }
    }
    let m = mask(&grid, tuning.threshold);
    let (w, h) = grid.dims();
    let closed = close(&m, w, h, tuning.close_radius);
    let rings = contours(&closed, w, h);
    let mut best_outer: Option<&[_]> = None;
    let mut best_area = 0f64;
    for ring in rings.iter().filter(|c| c.outer) {
        let area = ring_area(&ring.ring).abs();
        if area > best_area {
            best_area = area;
            best_outer = Some(&ring.ring);
        }
    }
    let Some(outer_ring) = best_outer else {
        return Ok((Vec::new(), Vec::new()));
    };
    let outer_poly = grid.to_world(outer_ring);
    let outer_pts: Vec<Vec2> = outer_ring.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let mut holes = Vec::new();
    for c in &rings {
        if !c.outer
            && c.ring.len() >= 3
            && let Some(first_v) = c.ring.first()
        {
            let p = Vec2::new(first_v[0], first_v[1]);
            if point_in_polygon(&outer_pts, p) {
                holes.push(grid.to_world(&c.ring));
            }
        }
    }
    Ok((outer_poly, holes))
}

/// The signed area of a ring, in whatever units it is given: cell steps inside this module, world
/// metres for a caller that measures a [`Slab::polygon`]. The one place this module speaks `geom`
/// types; the API stays plain arrays (ruling R2).
fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let pts: Vec<Vec2> = ring.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    shoelace(&pts)
}

/// Every coordinate a number. A point that fails this is skipped by every pass here, and never
/// becomes an index.
fn finite(p: &[f64; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
}

/// Fit a plane z = A*x + B*y + C to 3D points. Returns (A, B, inclination_deg) if successful.
fn fit_plane_slope(points: &[[f64; 3]]) -> Option<(f64, f64, f64)> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_z = 0.0;
    for p in points {
        sum_x += p[0];
        sum_y += p[1];
        sum_z += p[2];
    }
    let mean_x = sum_x / nf;
    let mean_y = sum_y / nf;
    let mean_z = sum_z / nf;

    let mut s_uu = 0.0;
    let mut s_uv = 0.0;
    let mut s_vv = 0.0;
    let mut s_uw = 0.0;
    let mut s_vw = 0.0;
    for p in points {
        let u = p[0] - mean_x;
        let v = p[1] - mean_y;
        let w = p[2] - mean_z;
        s_uu += u * u;
        s_uv += u * v;
        s_vv += v * v;
        s_uw += u * w;
        s_vw += v * w;
    }

    let det = s_uu * s_vv - s_uv * s_uv;
    if !det.is_finite() || det.abs() < 1e-9 {
        return None;
    }

    let a = (s_uw * s_vv - s_vw * s_uv) / det;
    let b = (s_vw * s_uu - s_uw * s_uv) / det;
    if !a.is_finite() || !b.is_finite() {
        return None;
    }

    let tan_alpha = (a * a + b * b).sqrt();
    let alpha_rad = tan_alpha.atan();
    let alpha_deg = alpha_rad.to_degrees();
    Some((a, b, alpha_deg))
}

/// Detect split-levels, floor steps, and sloped slabs across the given slab's horizontal footprint.
///
/// Bins the XY plane into regular cells (1.0 m grid). Computes the median Z of floor points in each cell.
/// When adjacent clusters show an elevation step Delta Z in [0.15, 1.50] m, emits distinct [`SlabRegion`]
/// boundaries with local `base_z` / `top_z`.
/// Checks normal vector inclination: if alpha in [3.0 deg, 45.0 deg], sets `is_sloped: true`, `slope_deg: alpha`.
pub fn detect_split_levels(points: &[[f64; 3]], slab: &Slab, opts: &BimOptions) -> Vec<SlabRegion> {
    let ref_z = slab.top_z;
    let z_min_search = ref_z - 1.60;
    let z_max_search = ref_z + 1.60;

    let mut candidate_points: Vec<[f64; 3]> = Vec::new();
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for p in points {
        if !finite(p) {
            continue;
        }
        if p[2] >= z_min_search && p[2] <= z_max_search {
            candidate_points.push(*p);
            if p[0] < min_x {
                min_x = p[0];
            }
            if p[0] > max_x {
                max_x = p[0];
            }
            if p[1] < min_y {
                min_y = p[1];
            }
            if p[1] > max_y {
                max_y = p[1];
            }
        }
    }

    if candidate_points.len() < 8 || !min_x.is_finite() || !min_y.is_finite() || max_x <= min_x || max_y <= min_y {
        return Vec::new();
    }

    let cell_size = 1.0;
    let mut grid: HashMap<(i32, i32), Vec<[f64; 3]>> = HashMap::new();
    for p in &candidate_points {
        let ix = ((p[0] - min_x) / cell_size).floor() as i32;
        let iy = ((p[1] - min_y) / cell_size).floor() as i32;
        grid.entry((ix, iy)).or_default().push(*p);
    }

    struct CellData {
        coord: (i32, i32),
        median_z: f64,
        pts: Vec<[f64; 3]>,
    }

    let mut valid_cells: Vec<CellData> = Vec::new();
    let mut cell_map: HashMap<(i32, i32), usize> = HashMap::new();

    let mut sorted_keys: Vec<(i32, i32)> = grid.keys().copied().collect();
    sorted_keys.sort();

    for coord in sorted_keys {
        let pts = match grid.remove(&coord) {
            Some(p) => p,
            None => continue,
        };
        if pts.len() < 3 {
            continue;
        }
        let mut z_vals: Vec<f64> = pts.iter().map(|p| p[2]).collect();
        z_vals.sort_by(|a, b| a.total_cmp(b));
        let median_z = z_vals.get(z_vals.len() / 2).copied().unwrap_or(0.0);
        let idx = valid_cells.len();
        valid_cells.push(CellData { coord, median_z, pts });
        cell_map.insert(coord, idx);
    }

    if valid_cells.is_empty() {
        return Vec::new();
    }

    // Connected components: two 4-adjacent cells belong to same cluster if |median_z_1 - median_z_2| < 0.12
    let mut visited = vec![false; valid_cells.len()];
    let mut clusters: Vec<Vec<usize>> = Vec::new();

    for i in 0..valid_cells.len() {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut cluster = vec![i];
        let mut queue = VecDeque::new();
        queue.push_back(i);

        while let Some(curr_idx) = queue.pop_front() {
            let (cx, cy) = valid_cells[curr_idx].coord;
            let cz = valid_cells[curr_idx].median_z;
            let neighbors = [(cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)];
            for n_coord in neighbors {
                if let Some(&n_idx) = cell_map.get(&n_coord)
                    && !visited[n_idx]
                {
                    let nz = valid_cells[n_idx].median_z;
                    if (cz - nz).abs() < 0.12 {
                        visited[n_idx] = true;
                        cluster.push(n_idx);
                        queue.push_back(n_idx);
                    }
                }
            }
        }
        clusters.push(cluster);
    }

    // Check if any adjacent cells across clusters exhibit an elevation step Delta Z in [0.15, 1.50]
    let mut cluster_of_cell = vec![0usize; valid_cells.len()];
    for (ci, cl) in clusters.iter().enumerate() {
        for &c_idx in cl {
            if let Some(slot) = cluster_of_cell.get_mut(c_idx) {
                *slot = ci;
            }
        }
    }

    let mut has_step = false;
    for (i, cell) in valid_cells.iter().enumerate() {
        let (cx, cy) = cell.coord;
        let ci = cluster_of_cell.get(i).copied().unwrap_or(0);
        let neighbors = [(cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)];
        for n_coord in neighbors {
            if let Some(&n_idx) = cell_map.get(&n_coord) {
                let n_ci = cluster_of_cell.get(n_idx).copied().unwrap_or(0);
                if ci != n_ci {
                    let dz = (cell.median_z - valid_cells[n_idx].median_z).abs();
                    if (0.15..=1.50).contains(&dz) {
                        has_step = true;
                        break;
                    }
                }
            }
        }
        if has_step {
            break;
        }
    }

    let thickness = if slab.thickness.is_finite() && slab.thickness > 0.0 { slab.thickness } else { opts.single_sided_slab_thickness };

    if has_step {
        let mut regions = Vec::new();
        for cl in &clusters {
            let mut cl_pts_2d = Vec::new();
            let mut cl_pts_3d = Vec::new();
            let mut z_sum = 0.0;
            let mut count = 0usize;
            for &c_idx in cl {
                let cell = &valid_cells[c_idx];
                for p in &cell.pts {
                    cl_pts_2d.push([p[0], p[1]]);
                    cl_pts_3d.push(*p);
                    z_sum += p[2];
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let mean_z = z_sum / count as f64;
            let hull = convex_hull_2d(&cl_pts_2d);
            let poly = if hull.len() >= 3 {
                hull
            } else if let Some(first_c) = cl.first().and_then(|&idx| valid_cells.get(idx)) {
                let (ix, iy) = first_c.coord;
                let x0 = min_x + ix as f64 * cell_size;
                let y0 = min_y + iy as f64 * cell_size;
                vec![[x0, y0], [x0 + cell_size, y0], [x0 + cell_size, y0 + cell_size], [x0, y0 + cell_size]]
            } else {
                continue;
            };

            let slope_info = fit_plane_slope(&cl_pts_3d);
            let (is_sloped, slope_deg) = match slope_info {
                Some((_, _, deg)) if (3.0..=45.0).contains(&deg) => (true, (deg * 10.0).round() / 10.0),
                _ => (false, 0.0),
            };

            regions.push(SlabRegion { polygon: poly, base_z: mean_z - thickness, top_z: mean_z, is_sloped, slope_deg });
        }
        regions.sort_by(|a, b| {
            a.base_z.total_cmp(&b.base_z).then_with(|| {
                let acx = a.polygon.iter().map(|p| p[0]).sum::<f64>() / a.polygon.len().max(1) as f64;
                let acy = a.polygon.iter().map(|p| p[1]).sum::<f64>() / a.polygon.len().max(1) as f64;
                let bcx = b.polygon.iter().map(|p| p[0]).sum::<f64>() / b.polygon.len().max(1) as f64;
                let bcy = b.polygon.iter().map(|p| p[1]).sum::<f64>() / b.polygon.len().max(1) as f64;
                acx.total_cmp(&bcx).then_with(|| acy.total_cmp(&bcy))
            })
        });
        return regions;
    }

    // No split step detected. Check if the entire slab is sloped (alpha in [3.0 deg, 45.0 deg])
    if let Some((_, _, deg)) = fit_plane_slope(&candidate_points)
        && (3.0..=45.0).contains(&deg)
    {
        let poly = if slab.polygon.len() >= 3 {
            slab.polygon.clone()
        } else {
            let pts_2d: Vec<[f64; 2]> = candidate_points.iter().map(|p| [p[0], p[1]]).collect();
            convex_hull_2d(&pts_2d)
        };
        return vec![SlabRegion { polygon: poly, base_z: slab.base_z, top_z: slab.top_z, is_sloped: true, slope_deg: (deg * 10.0).round() / 10.0 }];
    }

    Vec::new()
}

/// Detect stairs inside slab voids / openings or within storey volume.
///
/// For any hole in `slabs[i].holes`, analyzes point cloud within the hole cylinder.
/// Detects periodic riser steps Delta z in [0.14, 0.22] m and horizontal run Delta x in [0.22, 0.35] m.
/// When at least 3 consecutive steps are detected, emits a [`Stair`] record.
pub fn detect_stairs(points: &[[f64; 3]], slabs: &[Slab], storey_idx: usize, base_z: f64, top_z: f64) -> Vec<Stair> {
    let mut candidates: Vec<Vec<[f64; 3]>> = Vec::new();

    // 1. Check all holes across slabs
    for slab in slabs {
        for hole in &slab.holes {
            if hole.len() < 3 {
                continue;
            }
            let hole_vec2: Vec<Vec2> = hole.iter().map(|p| Vec2::new(p[0], p[1])).collect();
            let mut in_cyl = Vec::new();
            for p in points {
                if !finite(p) {
                    continue;
                }
                if p[2] < base_z - 0.20 || p[2] > top_z + 0.20 {
                    continue;
                }
                if point_in_polygon(&hole_vec2, Vec2::new(p[0], p[1])) {
                    in_cyl.push(*p);
                }
            }
            if in_cyl.len() >= 10 {
                candidates.push(in_cyl);
            }
        }
    }

    // 2. If no hole candidates were found (or slabs has no holes), fallback to storey points
    if candidates.is_empty() {
        let in_vol: Vec<[f64; 3]> = points.iter().filter(|p| finite(p) && p[2] >= base_z - 0.10 && p[2] <= top_z + 0.10).copied().collect();
        if in_vol.len() >= 10 {
            candidates.push(in_vol);
        }
    }

    let mut stairs = Vec::new();

    for pts in candidates {
        let z_step = 0.02;
        let mut lo_z = f64::INFINITY;
        let mut hi_z = f64::NEG_INFINITY;
        for p in &pts {
            if p[2] < lo_z {
                lo_z = p[2];
            }
            if p[2] > hi_z {
                hi_z = p[2];
            }
        }
        if !lo_z.is_finite() || !hi_z.is_finite() || hi_z <= lo_z {
            continue;
        }

        let num_bins = ((hi_z - lo_z) / z_step).ceil() as usize + 1;
        if num_bins > 2000 {
            continue;
        }
        let mut hist = vec![0u32; num_bins];
        for p in &pts {
            let bi = ((p[2] - lo_z) / z_step).floor() as usize;
            if let Some(c) = hist.get_mut(bi) {
                *c += 1;
            }
        }

        // Find local peaks in the histogram (horizontal treads form density peaks)
        let mut peak_zs = Vec::new();
        for i in 0..num_bins {
            let count = hist.get(i).copied().unwrap_or(0);
            if count < 3 {
                continue;
            }
            let prev = if i > 0 { hist.get(i - 1).copied().unwrap_or(0) } else { 0 };
            let next = hist.get(i + 1).copied().unwrap_or(0);
            if count >= prev && count >= next {
                let pz = lo_z + (i as f64 + 0.5) * z_step;
                peak_zs.push(pz);
            }
        }

        if peak_zs.len() < 3 {
            continue;
        }

        // Merge peaks that are closer than 0.08 m (min riser is 0.14 m)
        let mut merged_peaks = Vec::new();
        let mut curr_group = vec![peak_zs[0]];
        for &pz in &peak_zs[1..] {
            if let Some(&last_z) = curr_group.last() {
                if (pz - last_z).abs() < 0.08 {
                    curr_group.push(pz);
                } else {
                    let avg = curr_group.iter().sum::<f64>() / curr_group.len() as f64;
                    merged_peaks.push(avg);
                    curr_group.clear();
                    curr_group.push(pz);
                }
            }
        }
        if !curr_group.is_empty() {
            let avg = curr_group.iter().sum::<f64>() / curr_group.len() as f64;
            merged_peaks.push(avg);
        }

        if merged_peaks.len() < 3 {
            continue;
        }

        struct StepLevel {
            z: f64,
            centroid: [f64; 2],
            points_2d: Vec<[f64; 2]>,
        }

        let mut step_levels: Vec<StepLevel> = Vec::new();
        for &mz in &merged_peaks {
            let mut sx = 0.0;
            let mut sy = 0.0;
            let mut p2d = Vec::new();
            for p in &pts {
                if (p[2] - mz).abs() <= 0.03 {
                    sx += p[0];
                    sy += p[1];
                    p2d.push([p[0], p[1]]);
                }
            }
            if p2d.len() >= 3 {
                let n = p2d.len() as f64;
                step_levels.push(StepLevel { z: mz, centroid: [sx / n, sy / n], points_2d: p2d });
            }
        }

        if step_levels.len() < 3 {
            continue;
        }

        // Search for a run of consecutive steps satisfying Delta z in [0.14, 0.22] and Delta run in [0.20, 0.38]
        let mut best_run: Vec<usize> = Vec::new();
        for start in 0..step_levels.len() {
            let mut current_run = vec![start];
            let mut prev = start;
            for next in (start + 1)..step_levels.len() {
                let dz = step_levels[next].z - step_levels[prev].z;
                let dx = step_levels[next].centroid[0] - step_levels[prev].centroid[0];
                let dy = step_levels[next].centroid[1] - step_levels[prev].centroid[1];
                let dl = (dx * dx + dy * dy).sqrt();

                if (0.14..=0.22).contains(&dz) && (0.20..=0.38).contains(&dl) {
                    current_run.push(next);
                    prev = next;
                } else if dz > 0.22 {
                    break;
                }
            }
            if current_run.len() > best_run.len() {
                best_run = current_run;
            }
        }

        if best_run.len() >= 3 {
            let Some(&first_idx) = best_run.first() else {
                continue;
            };
            let Some(&last_idx) = best_run.last() else {
                continue;
            };
            let s_first = &step_levels[first_idx];
            let s_last = &step_levels[last_idx];

            let n_transitions = (best_run.len() - 1) as f64;
            if n_transitions <= 0.0 {
                continue;
            }
            let total_dz = s_last.z - s_first.z;
            let avg_riser = total_dz / n_transitions;

            let total_dx = s_last.centroid[0] - s_first.centroid[0];
            let total_dy = s_last.centroid[1] - s_first.centroid[1];
            let total_run = (total_dx * total_dx + total_dy * total_dy).sqrt();
            let avg_tread = total_run / n_transitions;

            let flight_axis = [s_first.centroid, s_last.centroid];

            let (ux, uy) = if total_run > 1e-6 { (total_dx / total_run, total_dy / total_run) } else { (1.0, 0.0) };
            let (nx, ny) = (-uy, ux);
            let mut v_min = f64::INFINITY;
            let mut v_max = f64::NEG_INFINITY;
            for &r_idx in &best_run {
                if let Some(lvl) = step_levels.get(r_idx) {
                    for p in &lvl.points_2d {
                        let proj = (p[0] - s_first.centroid[0]) * nx + (p[1] - s_first.centroid[1]) * ny;
                        if proj < v_min {
                            v_min = proj;
                        }
                        if proj > v_max {
                            v_max = proj;
                        }
                    }
                }
            }
            let measured_w = if v_max > v_min { v_max - v_min } else { 1.0 };
            let width = measured_w.clamp(0.60, 3.0);

            stairs.push(Stair {
                storey_index: storey_idx,
                base_z: s_first.z - avg_riser,
                top_z: s_last.z,
                num_risers: best_run.len(),
                riser_height: (avg_riser * 1000.0).round() / 1000.0,
                tread_depth: (avg_tread * 1000.0).round() / 1000.0,
                flight_axis,
                width: (width * 100.0).round() / 100.0,
            });
        }
    }

    stairs
}

#[cfg(test)]
mod tests {
    use super::fixture::{
        FACE_POINTS, Noise, PLAN_X, PLAN_Y, WALL_POINTS_PER_LEVEL, four_face_box, half_lower_face_box, plane_between, three_face_box,
    };
    use super::{BimError, MAX_Z_BINS, Slab, Tuning, find, plan, ring_area};
    use crate::bim::{BimOptions, MAX_CLOSE_RADIUS, MAX_RASTER_CELLS};

    #[test]
    fn three_faces_pair_into_two_slabs_and_the_middle_one_is_its_measured_thickness() {
        // B2: Step 1's fixture comment describes four faces while its assertion says two slabs and
        // "the ceiling is the top of the middle one", which cannot both be true. The test's own
        // name is the three-face case, so this is that fixture: a floor at 0.0 and one slab whose
        // two faces are at 3.0 and 3.3, with no ceiling. Every assertion is the brief's, in the
        // spelling `cargo clippy -- -D warnings` accepts: `assert_eq!(x, false)` is a lint, so the
        // one about `single_sided` reads as a negation. Same content.
        let pts = three_face_box();
        let slabs = find(&pts, &BimOptions::default()).expect("the box has slabs");
        assert_eq!(slabs.len(), 2, "a floor and a middle slab; the ceiling is the top of the middle one");
        assert!(!slabs[1].single_sided, "the middle slab is the measured pair");
        assert!((slabs[1].thickness - 0.30).abs() < 0.02, "the paired faces measure the slab: {}", slabs[1].thickness);
        assert!(slabs[0].single_sided, "the ground face has no partner below");
        assert!((slabs[0].thickness - 0.20).abs() < 1e-9, "and takes the single-sided thickness");
    }

    #[test]
    fn a_face_with_two_centimetres_of_scan_roughness_still_gives_two_slabs() {
        // I-2, (e) T1. Real shells are not exact planes: jitter every point's Z and require the
        // clean box's structure to survive. WALL_STEP keeps the wall bins separate by construction;
        // the faces now straddle bin boundaries and `push_face`'s mean is exercised for the first
        // time. ±2 cm is the 4 cm spread the brief names, against a 5 cm bin.
        let mut rng = Noise::new();
        let pts: Vec<[f64; 3]> = three_face_box().into_iter().map(|p| [p[0], p[1], p[2] + (rng.next() - 0.5) * 0.04]).collect();
        let slabs = find(&pts, &BimOptions::default()).expect("rough faces are still faces");
        assert_eq!(slabs.len(), 2, "roughness must neither invent a face nor merge the slab's two");
        assert!(!slabs[1].single_sided && (slabs[1].thickness - 0.30).abs() < 0.06, "the measured gap survives jitter: {}", slabs[1].thickness);
    }

    #[test]
    fn one_trough_inside_a_rough_floor_does_not_split_it_into_a_phantom_slab() {
        // The real Ap 6 scan's failure, scaled down a hundredfold: a five-bin ceiling hump whose
        // tallest bin holds 9 480 points, and a floor spread over four bins — 5 500 / 4 291 /
        // 5 459 / 4 790 — so the 50 % cut (4 740) falls through a trough inside the floor's hump.
        // Read bin by bin the floor is two faces, the upper half pairs with the ceiling into a
        // "slab" 2.42 m thick, and the scan yields no storey. One bridged trough later there is a
        // floor, a ceiling and a room between them.
        let pts = rough_floor_under_a_dense_ceiling();
        let slabs = find(&pts, &BimOptions::default()).expect("the floor and the ceiling are faces");
        assert_eq!(slabs.len(), 2, "one floor face and one ceiling face; got {slabs:?}");
        let floor = &slabs[0];
        assert!(floor.single_sided, "the ground face has no partner below");
        assert!(
            (floor.top_z - 1.0919).abs() < 0.001,
            "the face is the mean of the whole hump, bins 0 through 3, not its lowest bin: {}",
            floor.top_z
        );
        assert!((floor.thickness - 0.20).abs() < 1e-9, "a found floor takes the named single-sided thickness");
        let ceiling = &slabs[1];
        assert!(ceiling.single_sided, "the ceiling's partner must not be the floor's upper half");
        assert!((ceiling.base_z - 3.5729).abs() < 0.001, "the ceiling stands at the mean of its own five bins: {}", ceiling.base_z);
        assert!((ceiling.thickness - 0.20).abs() < 1e-9, "the body is the named thickness, not the 2.4 m gap the split floor used to measure");
        let storeys = crate::bim::storeys::split(&slabs, &BimOptions::default());
        assert_eq!(storeys.len(), 1, "the room between the floor and the ceiling exists");
    }

    /// The Ap 6 histogram, scaled down a hundredfold: a five-bin ceiling hump whose tallest bin is
    /// 9 480, and a floor spread over four bins with a 4 291-point trough under the 4 740 cut.
    /// Every level but the lowest sits half a bin inside its own bin, so floating-point bin
    /// arithmetic cannot fold two of them together.
    fn rough_floor_under_a_dense_ceiling() -> Vec<[f64; 3]> {
        const BASE: f64 = 1.0;
        let mut pts = Vec::new();
        let levels = [(0, 5_500), (1, 4_291), (2, 5_459), (3, 4_790), (49, 5_366), (50, 9_480), (51, 8_377), (52, 7_835), (53, 5_388)];
        for (bin, count) in levels {
            let z = if bin == 0 { BASE } else { BASE + (bin as f64 + 0.5) * 0.05 };
            for k in 0..count {
                // A 6 x 4 m footprint on the 5 cm lattice, so the plan stage grids a real room.
                pts.push([(k % 121) as f64 * 0.05, ((k / 121) % 81) as f64 * 0.05, z]);
            }
        }
        pts
    }

    #[test]
    fn a_slab_face_erased_past_the_ratio_cut_is_read_as_a_ceiling_not_a_slab() {
        // I-1, (e) T2: the occlusion case, pinned rather than fixed — B3's rule stays. 40 % of the
        // lower face remains (below the 0.5 cut): the pairing sees faces [0.0, 3.3] only.
        let mut pts: Vec<[f64; 3]> = Vec::new();
        plane_between(0.0, 0, 120, &mut pts); // full floor
        plane_between(3.0, 0, 48, &mut pts); // 49 x 81 = 3,969 < 0.5 x 9,801
        plane_between(3.3, 0, 120, &mut pts); // full top
        let slabs = find(&pts, &BimOptions::default()).expect("the surviving faces pair");
        assert_eq!(slabs.len(), 2, "floor and one ceiling — the slab's body is NOT recovered");
        assert!(slabs[1].single_sided, "the 3.3 face lost its partner below");
        assert!(
            (slabs[1].base_z - 3.3).abs() < 1e-6 && (slabs[1].top_z - 3.5).abs() < 1e-6,
            "B3: the unpaired face above the room is a ceiling, body above — the storey below \
             then extends into the real slab body; Task 3 must know this"
        );
    }

    #[test]
    fn the_bin_budget_admits_999m95_of_band_and_refuses_a_centimetre_more() {
        // M-1: both end bins count, so the largest band the budget admits at the paper's z_step is
        // 999.95 m, not the 1 000 m this doc used to claim — and the refusal comes before a byte is
        // allocated, because the quotient is compared in `f64` and never cast.
        let at = |span: f64| find(&[[0.0, 0.0, 0.0], [0.0, 0.0, span]], &BimOptions::default());
        assert!(at(999.95).is_ok(), "exactly MAX_Z_BINS bins (inclusive) is admitted");
        assert!(
            matches!(at(1000.0), Err(BimError::TooLarge { what: "z histogram", limit: 20_000 })),
            "20,001 bins is refused before a byte is allocated — fix MAX_Z_BINS' doc if this flips"
        );
    }

    #[test]
    fn the_fixture_is_the_size_its_doc_claims() {
        // The arithmetic every other test here reasons about: one full plane is 121 x 81 points,
        // one wall level is 404, and the box is three planes plus 34 levels.
        assert_eq!(FACE_POINTS, 121 * 81);
        assert_eq!(WALL_POINTS_PER_LEVEL, 404);
        assert_eq!(three_face_box().len(), 3 * FACE_POINTS + 34 * WALL_POINTS_PER_LEVEL);
    }

    #[test]
    fn twenty_percent_of_the_points_floating_in_the_room_invents_no_slab() {
        // Step 5, and the paper's own weakness: a density histogram is only as good as its
        // density, so the cloud that is a fifth floating points must still give the corrected
        // three-face fixture's two slabs at their exact heights (B8).
        let mut pts = three_face_box();
        let clean_len = pts.len();
        let clean = find(&pts, &BimOptions::default()).expect("the clean box has slabs");
        let mut rng = Noise::new();
        for _ in 0..clean_len / 4 {
            // Uniform over the room: the plan's whole footprint, and the storey's height between
            // the margin lines the split itself would draw (0.1 .. 2.9), kept off the faces.
            pts.push([rng.next() * PLAN_X, rng.next() * PLAN_Y, 0.15 + rng.next() * 2.7]);
        }
        assert_eq!(pts.len() - clean_len, clean_len / 4, "a fifth of the resulting cloud is noise");
        let noisy = find(&pts, &BimOptions::default()).expect("the noisy box has slabs");
        assert_eq!(noisy.len(), 2, "noise adds points, not storeys");
        for (clean, noisy) in clean.iter().zip(&noisy) {
            assert_eq!(clean.single_sided, noisy.single_sided);
            assert!((clean.base_z - noisy.base_z).abs() < 1e-12, "the floor does not move: {} vs {}", clean.base_z, noisy.base_z);
            assert!((clean.top_z - noisy.top_z).abs() < 1e-12, "nor does the slab: {} vs {}", clean.top_z, noisy.top_z);
            assert!((clean.thickness - noisy.thickness).abs() < 1e-12);
            assert_eq!(clean.polygon.len(), noisy.polygon.len(), "and the plan is the same ring");
            assert!((ring_area(&clean.polygon) - ring_area(&noisy.polygon)).abs() < 1e-9);
        }

        // How strong the fixture actually is, stated rather than assumed, because "it passed" is
        // not evidence: the tallest bin that is not a face has to sit under the candidate cut, and
        // a cloud whose noise ever reached that cut would fail here before the slab count moved.
        let o = BimOptions::default();
        let zmin = pts.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let zmax = pts.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        let bins = ((zmax - zmin) / o.z_step).floor() as usize + 1;
        let mut hist = vec![0u32; bins];
        for p in &pts {
            let i = (((p[2] - zmin) / o.z_step).floor() as usize).min(bins - 1);
            if let Some(c) = hist.get_mut(i) {
                *c = c.saturating_add(1);
            }
        }
        let tallest = hist.iter().copied().max().unwrap_or(0);
        let cut = f64::from(tallest) * o.slab_ratio;
        let loudest_noise = hist
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let z = zmin + (*i as f64 + 0.5) * o.z_step;
                ![0.0, 3.0, 3.3].iter().any(|f| (z - f).abs() <= o.z_step)
            })
            .map(|(_, c)| *c)
            .max()
            .unwrap_or(0);
        assert!(
            f64::from(loudest_noise) < cut,
            "the noisiest non-face bin holds {} points and a face starts at {:.0} (tallest bin {})",
            loudest_noise,
            cut,
            tallest
        );
    }

    #[test]
    fn a_paired_slab_takes_the_plan_of_both_faces_and_not_just_one() {
        // The paper's reason for rasterising a slab's two faces together: "the slab mirrors the
        // ceiling shape of the floor below", so whichever face the furniture erased, the other
        // still describes. Here the lower face is blocked past x = 4 m and the ceiling is whole.
        let half = half_lower_face_box();
        let tuning = Tuning::read(&BimOptions::default()).expect("the defaults are usable");
        let alone = plan(&half, &[3.0], &tuning).expect("the blocked face rasterises").0;
        let merged = plan(&half, &[3.0, 3.3], &tuning).expect("both faces together rasterise").0;
        assert!((ring_area(&alone).abs() - 16.0).abs() < 0.01, "the lower face alone draws 4 m x 4 m, got {}", ring_area(&alone).abs());
        assert!((ring_area(&merged).abs() - 24.0).abs() < 0.01, "the merged band draws the whole 6 m x 4 m, got {}", ring_area(&merged).abs());
        let slabs = find(&half, &BimOptions::default()).expect("the half box has slabs");
        assert_eq!(slabs.len(), 2);
        let paired = &slabs[1];
        assert!(!paired.single_sided);
        assert!(
            (ring_area(&paired.polygon).abs() - 24.0).abs() < 0.01,
            "the plan kept its size even though the underside was half missing: {:.2} m2",
            ring_area(&paired.polygon).abs()
        );
    }

    #[test]
    fn a_single_sided_body_hangs_below_the_lowest_face_and_stands_above_any_other() {
        // B3's rule, asserted where it is set. The scanner stands on the lowest surface, so that
        // face is the *top* of its body; a face above with no partner is a ceiling or a roof, so
        // it is the *bottom* of its body. The storey split reads these numbers directly.
        let slabs = find(&four_face_box(), &BimOptions::default()).expect("the box has three slabs");
        assert_eq!(slabs.len(), 3, "floor, slab, ceiling");
        assert!(slabs.windows(2).all(|w| w[0].base_z <= w[1].base_z), "the result is ordered by base_z");
        let ground = &slabs[0];
        assert!(ground.single_sided);
        assert!(ground.top_z.abs() < 1e-9, "the measured floor is the top of the ground slab, got {}", ground.top_z);
        assert!((ground.base_z + 0.20).abs() < 1e-9, "and the body hangs the manual thickness under it");
        assert!(!slabs[1].single_sided, "the middle pair is measured, not invented");
        let roof = &slabs[2];
        assert!(roof.single_sided);
        assert!((roof.base_z - 6.3).abs() < 1e-6, "the measured ceiling is the underside of the roof, got {}", roof.base_z);
        assert!((roof.top_z - 6.5).abs() < 1e-6, "and the body stands the manual thickness over it, got {}", roof.top_z);
    }

    #[test]
    fn a_slab_polygon_is_the_outer_ring_of_its_plan_in_metres() {
        // The declared shape of `polygon`: world metres, the outer ring only, first corner not
        // repeated. The fixture is 6 m x 4 m at the default 0.05 m cell, so the ring is exactly
        // the 120 x 80 grid's border: 400 corners.
        let slabs = find(&three_face_box(), &BimOptions::default()).expect("the box has slabs");
        for slab in &slabs {
            assert!(!slab.polygon.is_empty(), "a whole plane under a face leaves a plan");
            assert_eq!(slab.polygon.len(), 400, "2 * (120 + 80) grid corners");
            assert_ne!(slab.polygon.first(), slab.polygon.last(), "the ring does not repeat its first corner");
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for p in &slab.polygon {
                for (k, v) in p.iter().enumerate() {
                    assert!(v.is_finite(), "a plan corner is a number: {p:?}");
                    if *v < lo[k] {
                        lo[k] = *v;
                    }
                    if *v > hi[k] {
                        hi[k] = *v;
                    }
                }
            }
            assert!((lo[0] - 0.0).abs() < 1e-9 && (hi[0] - PLAN_X).abs() < 1e-9, "the ring spans x 0..{PLAN_X}, got {lo:?}..{hi:?}");
            assert!((lo[1] - 0.0).abs() < 1e-9 && (hi[1] - PLAN_Y).abs() < 1e-9, "and y 0..{PLAN_Y}");
            assert!((ring_area(&slab.polygon).abs() - PLAN_X * PLAN_Y).abs() < 1e-9, "and encloses its own footprint");
        }
    }

    #[test]
    fn an_unusable_option_is_refused_rather_than_defaulted() {
        // B5: a number the stage cannot work with is `InvalidCell` — not the default, not a panic.
        // Every length and ratio the slab stage reads is checked, because a default that quietly
        // wins over a typo in the caller's options is a result the caller cannot explain.
        let junk = [-0.05, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        let lengths: [(&str, fn(&mut BimOptions, f64)); 4] = [
            ("z_step", |o, v| o.z_step = v),
            ("cell", |o, v| o.cell = v),
            ("single_sided_slab_thickness", |o, v| o.single_sided_slab_thickness = v),
            ("slab_ratio", |o, v| o.slab_ratio = v),
        ];
        for (name, set) in lengths {
            for v in junk.iter().copied().chain([0.0]) {
                let mut o = BimOptions::default();
                set(&mut o, v);
                assert!(
                    matches!(find(&three_face_box(), &o), Err(BimError::InvalidCell)),
                    "a {name} of {v} cannot be worked with: refused, not defaulted"
                );
            }
        }
        // `threshold` is the one number that may be zero, because zero is `mask`'s documented
        // raw-occupancy mode rather than a mistake.
        for v in junk {
            assert!(
                matches!(find(&three_face_box(), &BimOptions { threshold: v, ..BimOptions::default() }), Err(BimError::InvalidCell)),
                "a threshold of {v} is refused, not defaulted"
            );
        }
        let flat = find(&three_face_box(), &BimOptions { threshold: 0.0, ..BimOptions::default() })
            .expect("a zero density cut is the raw occupancy mode mask documents");
        assert_eq!(flat.len(), 2, "and it still reads the box's two slabs");
    }

    #[test]
    fn a_band_taller_than_the_histogram_budget_is_refused_before_it_is_allocated() {
        // B5: 5 km of band at z_step 0.05 is 100,001 bins, past MAX_Z_BINS. The quotient is
        // checked in f64 before the cast, so the bins are never allocated.
        assert_eq!(MAX_Z_BINS, 20_000, "999.95 m of building at the paper's z_step");
        let wide = [[0.0, 0.0, 0.0], [1.0, 1.0, 5_000.0]];
        let read = find(&wide, &BimOptions::default());
        assert!(
            matches!(read, Err(BimError::TooLarge { what: "z histogram", limit: MAX_Z_BINS })),
            "5 km of band asks for 100,001 bins and must be refused by name: {read:?}"
        );
        // And the genuinely hostile one, which would ask for twenty million bins: refused on the
        // same check, so nothing is ever allocated to find out.
        let absurd = [[0.0, 0.0, 0.0], [1.0, 1.0, 1_000_000.0]];
        assert!(matches!(find(&absurd, &BimOptions::default()), Err(BimError::TooLarge { what: "z histogram", limit: MAX_Z_BINS })));
    }

    #[test]
    fn a_plan_too_wide_for_the_grid_is_an_error_and_not_an_empty_polygon() {
        // B6: Raster::new answers a grid that will not fit with None, and a slab that silently
        // carries no polygon would look like an empty scan rather than a 100 km one.
        let mut pts: Vec<[f64; 3]> = Vec::new();
        for ix in 0..=50 {
            for iy in 0..=50 {
                pts.push([ix as f64 * 2_000.0, iy as f64 * 2_000.0, 0.0]);
            }
        }
        assert_eq!(pts.len(), 2_601, "a 100 km x 100 km floor, 2 km between its points");
        let read = find(&pts, &BimOptions::default());
        assert!(
            matches!(read, Err(BimError::TooLarge { what: "slab raster", limit: MAX_RASTER_CELLS })),
            "the grid is refused by name, not swallowed: {read:?}"
        );
    }

    #[test]
    fn a_close_radius_past_the_budget_is_refused_before_the_grid_is_walked() {
        let o = BimOptions { close_radius: MAX_CLOSE_RADIUS + 1, ..BimOptions::default() };
        assert!(
            matches!(find(&three_face_box(), &o), Err(BimError::TooLarge { what: "close radius", limit: MAX_CLOSE_RADIUS })),
            "a radius the JSON path cannot even ask for is refused here too"
        );
    }

    #[test]
    fn a_cloud_with_nothing_usable_in_it_is_empty_and_never_panics() {
        let o = BimOptions::default();
        for hostile in
            [Vec::new(), vec![[f64::NAN, 0.0, 0.0]], vec![[0.0, f64::INFINITY, 0.0]], vec![[0.0, 0.0, f64::NAN], [f64::NAN, f64::NAN, f64::NAN]]]
        {
            assert!(matches!(find(&hostile, &o), Err(BimError::Empty)), "a cloud of {} points with no finite Z in it answers Empty", hostile.len());
        }
        // A single finite point is a whole cloud: one bin, one face, one slab, one cell of plan.
        let one = find(&[[2.0, 3.0, 4.0]], &o).expect("one point is still a surface");
        assert_eq!(one.len(), 1);
        assert!(one.first().is_some_and(|s| s.single_sided && (s.thickness - 0.20).abs() < 1e-9));
    }

    #[test]
    fn slab_with_rectangular_hole_extracts_inner_hole_contour() {
        // Floor slab at z = 0.0 with outer dimensions 10 x 10 m and a hole from [3.0, 3.0] to [7.0, 7.0].
        let mut pts = Vec::new();
        let step = 0.05;
        let n_steps = 200; // 0.0 to 10.0 m
        for ix in 0..=n_steps {
            let x = ix as f64 * step;
            for iy in 0..=n_steps {
                let y = iy as f64 * step;
                if (3.0..=7.0).contains(&x) && (3.0..=7.0).contains(&y) {
                    continue; // hole void
                }
                pts.push([x, y, 0.0]);
            }
        }
        let slabs = find(&pts, &BimOptions::default()).expect("finds the slab");
        assert_eq!(slabs.len(), 1, "exactly one floor slab");
        assert_eq!(slabs[0].holes.len(), 1, "exactly one hole detected");
        let hole = &slabs[0].holes[0];
        let min_x = hole.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let max_x = hole.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        let min_y = hole.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let max_y = hole.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        assert!((min_x - 3.0).abs() <= 0.2, "hole min x matches 3.0 within cell tolerance: {min_x}");
        assert!((max_x - 7.0).abs() <= 0.2, "hole max x matches 7.0 within cell tolerance: {max_x}");
        assert!((min_y - 3.0).abs() <= 0.2, "hole min y matches 3.0 within cell tolerance: {min_y}");
        assert!((max_y - 7.0).abs() <= 0.2, "hole max y matches 7.0 within cell tolerance: {max_y}");
    }

    #[test]
    fn detect_split_levels_finds_elevation_step_and_emits_regions() {
        let mut pts = Vec::new();
        // Left side: [0, 4] x [0, 4] at z = 0.0
        // Right side: [4, 8] x [0, 4] at z = 0.50 (step = 0.50 m in [0.15, 1.50])
        let step = 0.10;
        for ix in 0..=80 {
            let x = ix as f64 * step;
            for iy in 0..=40 {
                let y = iy as f64 * step;
                let z = if x < 4.0 { 0.0 } else { 0.50 };
                pts.push([x, y, z]);
            }
        }
        let slab = Slab {
            base_z: -0.20,
            top_z: 0.0,
            thickness: 0.20,
            polygon: vec![[0.0, 0.0], [8.0, 0.0], [8.0, 4.0], [0.0, 4.0]],
            holes: Vec::new(),
            single_sided: true,
            regions: Vec::new(),
        };
        let regions = super::detect_split_levels(&pts, &slab, &BimOptions::default());
        assert_eq!(regions.len(), 2, "detects two distinct split-level regions: {regions:?}");
        assert!((regions[0].top_z - 0.0).abs() <= 0.05, "lower region around z=0.0: got {}", regions[0].top_z);
        assert!((regions[1].top_z - 0.50).abs() <= 0.05, "upper region around z=0.50: got {}", regions[1].top_z);
        assert!(!regions[0].is_sloped, "lower region is flat");
        assert!(!regions[1].is_sloped, "upper region is flat");
        assert!(regions[0].polygon.len() >= 3);
        assert!(regions[1].polygon.len() >= 3);
    }

    #[test]
    fn detect_split_levels_detects_sloped_slab_ramp() {
        let mut pts = Vec::new();
        // Ramp: [0, 6] x [0, 3], z = 0.10 * x -> slope ~5.7 deg (in [3.0, 45.0])
        let step = 0.10;
        for ix in 0..=60 {
            let x = ix as f64 * step;
            for iy in 0..=30 {
                let y = iy as f64 * step;
                let z = 0.10 * x;
                pts.push([x, y, z]);
            }
        }
        let slab = Slab {
            base_z: -0.20,
            top_z: 0.30,
            thickness: 0.20,
            polygon: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [0.0, 3.0]],
            holes: Vec::new(),
            single_sided: true,
            regions: Vec::new(),
        };
        let regions = super::detect_split_levels(&pts, &slab, &BimOptions::default());
        assert_eq!(regions.len(), 1, "detects 1 sloped region: {regions:?}");
        assert!(regions[0].is_sloped, "marked as sloped");
        assert!((regions[0].slope_deg - 5.7).abs() <= 0.6, "slope ~5.7 deg: got {}", regions[0].slope_deg);
        assert!(regions[0].polygon.len() >= 3);
    }

    #[test]
    fn detect_split_levels_flat_slab_returns_empty_regions() {
        let mut pts = Vec::new();
        let step = 0.10;
        for ix in 0..=40 {
            let x = ix as f64 * step;
            for iy in 0..=40 {
                let y = iy as f64 * step;
                pts.push([x, y, 0.0]);
            }
        }
        let slab = Slab {
            base_z: -0.20,
            top_z: 0.0,
            thickness: 0.20,
            polygon: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
            holes: Vec::new(),
            single_sided: true,
            regions: Vec::new(),
        };
        let regions = super::detect_split_levels(&pts, &slab, &BimOptions::default());
        assert!(regions.is_empty(), "flat uniform slab emits no split-level regions: {regions:?}");
    }

    #[test]
    fn detect_stairs_in_hole_cylinder_finds_periodic_steps() {
        let mut pts = Vec::new();
        // 5 steps: riser dz = 0.18 m, tread dx = 0.28 m, width = 1.0 m (y in [1.5, 2.5])
        for k in 0..5 {
            let zk = 0.18 + k as f64 * 0.18;
            let cx = 1.30 + k as f64 * 0.28;
            for ix in 0..10 {
                let x = cx - 0.10 + ix as f64 * 0.02;
                for iy in 0..10 {
                    let y = 1.55 + iy as f64 * 0.10;
                    pts.push([x, y, zk]);
                }
            }
        }

        let slab_with_hole = Slab {
            base_z: 2.80,
            top_z: 3.00,
            thickness: 0.20,
            polygon: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0]],
            holes: vec![vec![[1.0, 1.0], [4.0, 1.0], [4.0, 4.0], [1.0, 4.0]]],
            single_sided: false,
            regions: Vec::new(),
        };

        let stairs = super::detect_stairs(&pts, &[slab_with_hole], 0, 0.0, 2.80);
        assert_eq!(stairs.len(), 1, "detects exactly one stair flight: {stairs:?}");
        let st = &stairs[0];
        assert_eq!(st.storey_index, 0);
        assert_eq!(st.num_risers, 5, "all 5 steps detected");
        assert!((st.riser_height - 0.18).abs() <= 0.02, "riser height ~0.18: got {}", st.riser_height);
        assert!((st.tread_depth - 0.28).abs() <= 0.03, "tread depth ~0.28: got {}", st.tread_depth);
        assert!((st.width - 1.0).abs() <= 0.20, "stair width ~1.0: got {}", st.width);
        assert!(st.top_z > st.base_z);
    }

    #[test]
    fn detect_stairs_standalone_in_storey_points() {
        let mut pts = Vec::new();
        // 4 steps without hole
        for k in 0..4 {
            let zk = 0.20 + k as f64 * 0.17;
            let cx = 2.0 + k as f64 * 0.26;
            for ix in 0..8 {
                let x = cx - 0.08 + ix as f64 * 0.02;
                for iy in 0..8 {
                    let y = 2.0 + iy as f64 * 0.12;
                    pts.push([x, y, zk]);
                }
            }
        }

        let stairs = super::detect_stairs(&pts, &[], 1, 0.0, 2.80);
        assert_eq!(stairs.len(), 1, "detects stair without holes: {stairs:?}");
        assert_eq!(stairs[0].num_risers, 4);
        assert_eq!(stairs[0].storey_index, 1);
        assert!((stairs[0].riser_height - 0.17).abs() <= 0.02);
    }
}
