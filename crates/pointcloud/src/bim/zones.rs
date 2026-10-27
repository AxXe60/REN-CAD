//! Zones: the rooms the walls enclose, as closed polygons on the wall faces.
//!
//! The walls arrive as axes with a thickness. This stage snaps their ends together, turns each
//! wall into the rectangle it occupies, and asks the geometry crate's region tracer for the loop
//! around a seed point in the free space beside a wall face. The loop comes back on the *faces*,
//! not on the axes, so a zone is the room a person stands in.
//!
//! Like the rest of `bim`, the module owns plain data: no file, no environment, no network.

use cadcraft_geom::region;
use cadcraft_geom::{Vec2, point_in_polygon, shoelace};

use crate::bim::BimError;
use crate::bim::BimOptions;
use crate::bim::walls::Wall;

/// A room: the ring of its inner wall faces, in metres, counter-clockwise.
///
/// The ring is closed by joining the last vertex back to the first: the first vertex is **not**
/// repeated at the end, so `n` vertices mean `n` edges, the convention [`Contour`](crate::bim::Contour)
/// already uses. `area` is the enclosed surface, `perimeter` the sum of the `n` edges.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    /// The room's outline on the wall faces, counter-clockwise, first vertex not repeated.
    pub ring: Vec<[f64; 2]>,
    /// Enclosed area, in square metres.
    pub area: f64,
    /// Length of the outline, in metres.
    pub perimeter: f64,
}

/// The most walls the stage will snap and trace.
///
/// Snapping compares every wall end with every other axis, so the budget keeps that quadratic
/// pass bounded no matter what a caller hands in.
pub const MAX_ZONE_WALLS: usize = 4096;

/// The most seed probes the stage will trace. Every room needs one hit, and a wall contributes at
/// most seven probes per side, so a plan with more rooms than this is refused rather than run.
pub const MAX_ZONE_SEEDS: usize = 2048;

/// The rooms `walls` enclose, sorted by centroid (x, then y).
///
/// A wall end within [`BimOptions::zone_gap`] of another wall's line is snapped onto it, so a
/// corner whose walls stop short of each other still closes; a room smaller than
/// [`BimOptions::min_zone_area`] is not a room and is dropped. Walls that are not numbers, have
/// no length or no thickness are skipped, never indexed. Nothing here panics on hostile input:
/// the only failures are [`BimError::InvalidCell`] for an unusable option and
/// [`BimError::TooLarge`] when the wall or seed budget is exceeded.
pub fn build(walls: &[Wall], opts: &BimOptions) -> Result<Vec<Zone>, BimError> {
    let t = Tuning::read(opts)?;
    if walls.len() > MAX_ZONE_WALLS {
        return Err(BimError::TooLarge { what: "zone walls", limit: MAX_ZONE_WALLS });
    }

    // 1. Structural-Only Filtering:
    // Filter walls to retain only full-height structural walls (!w.is_parapet && w.height >= height * 0.75
    // where height = walls.iter().map(|w| w.height).fold(0.0, f64::max)).
    // Partial-height parapets and furniture must NOT participate in zone boundary tracing.
    let max_height = walls.iter().map(|w| w.height).filter(|h| h.is_finite()).fold(0.0f64, f64::max);
    let structural_walls: Vec<Wall> =
        walls.iter().filter(|w| !w.is_parapet && w.height.is_finite() && w.height >= max_height * 0.75).cloned().collect();

    let mut axes = usable_axes(&structural_walls);
    if axes.is_empty() {
        return Ok(Vec::new());
    }
    snap(&mut axes, t.zone_gap);
    split_t_junctions(&mut axes);

    // 2. Degree-1 Dead-end Pruning:
    // After snapping and splitting T-junctions, prune degree-1 antenna stubs (< 0.60m)
    // from centerline axes before tracing boundaries, leaving structural walls intact.
    add_doorway_closures_to_axes(&mut axes, 2.00);
    prune_degree_one_axes(&mut axes);

    if axes.is_empty() {
        return Ok(Vec::new());
    }

    let rects: Vec<Vec<Vec2>> = axes.iter().filter(|a| a.thickness > 1e-6).map(rect).collect();
    let mut soup = segments(&axes);
    add_doorway_closures(&axes, &mut soup, 2.00);
    if soup.is_empty() {
        return Ok(Vec::new());
    }
    let bridge_gap = t.zone_gap.max(1.60);
    let mut zones: Vec<Zone> = Vec::new();
    let mut traced = 0usize;
    for a in &axes {
        if a.thickness <= 1e-6 {
            continue;
        }
        let d = a.to - a.from;
        let len = d.len();
        if !len.is_finite() || len <= EPS_LEN {
            continue;
        }
        let along = d / len;
        for clearance in [0.05, 0.20, 0.40] {
            let out = Vec2::new(-along.y, along.x) * (a.thickness / 2.0 + clearance);
            for side in [1.0, -1.0] {
                for f in STATIONS {
                    let seed = a.from + d * f + out * side;
                    if !seed.is_finite() || rects.iter().any(|r| point_in_polygon(r, seed)) {
                        continue;
                    }
                    if zones.iter().any(|z| ring_holds(z, seed)) {
                        break;
                    }
                    if traced >= MAX_ZONE_SEEDS {
                        return Err(BimError::TooLarge { what: "zone seeds", limit: MAX_ZONE_SEEDS });
                    }
                    traced += 1;
                    let Some(boundary) = region::boundary_at_gap(&soup, seed, TRACE_TOL, bridge_gap) else {
                        continue;
                    };
                    let Some(zone) = zone_of(&boundary.outer, t.min_zone_area) else {
                        continue;
                    };
                    if !zones.iter().any(|existing| is_same_zone(existing, &zone)) {
                        zones.push(zone);
                    }
                    break;
                }
            }
        }
    }

    // Grid probe seeds across bounding box (0.60m step) to capture open-plan living spaces.
    let min_x = axes.iter().map(|a| a.from.x.min(a.to.x)).fold(f64::INFINITY, f64::min);
    let max_x = axes.iter().map(|a| a.from.x.max(a.to.x)).fold(f64::NEG_INFINITY, f64::max);
    let min_y = axes.iter().map(|a| a.from.y.min(a.to.y)).fold(f64::INFINITY, f64::min);
    let max_y = axes.iter().map(|a| a.from.y.max(a.to.y)).fold(f64::NEG_INFINITY, f64::max);
    if min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite() {
        let mut gx = min_x + 0.60;
        while gx < max_x {
            let mut gy = min_y + 0.60;
            while gy < max_y {
                let seed = Vec2::new(gx, gy);
                gy += 0.60;
                if rects.iter().any(|r| point_in_polygon(r, seed)) {
                    continue;
                }
                if zones.iter().any(|z| ring_holds(z, seed)) {
                    continue;
                }
                if traced >= MAX_ZONE_SEEDS {
                    break;
                }
                traced += 1;
                let Some(boundary) = region::boundary_at_gap(&soup, seed, TRACE_TOL, bridge_gap) else {
                    continue;
                };
                let Some(zone) = zone_of(&boundary.outer, t.min_zone_area) else {
                    continue;
                };
                if !zones.iter().any(|existing| is_same_zone(existing, &zone)) {
                    zones.push(zone);
                }
            }
            gx += 0.60;
        }
    }
    // Quantized keys, not raw floats: two rooms of one row trace at x = 1.4999999999999993 and
    // 1.4999999999999996, and comparing those puts the row in an order the eye cannot see. A
    // micrometre is far below any room dimension and makes the x-then-y order the intended one.
    zones.sort_by_key(centroid_key);
    Ok(zones)
}

fn centroid_key(z: &Zone) -> (i64, i64) {
    let (x, y) = centroid(z);
    ((x * 1e6).round() as i64, (y * 1e6).round() as i64)
}

fn is_same_zone(a: &Zone, b: &Zone) -> bool {
    let (ax, ay) = centroid(a);
    let (bx, by) = centroid(b);
    (ax - bx).hypot(ay - by) < 0.20 && (a.area - b.area).abs() < 0.20
}

/// The options this stage reads, checked once so the loops below are arithmetic only.
struct Tuning {
    zone_gap: f64,
    min_zone_area: f64,
}

impl Tuning {
    fn read(o: &BimOptions) -> Result<Self, BimError> {
        let usable = |v: f64| v.is_finite() && v >= 0.0;
        if !usable(o.zone_gap) || !usable(o.min_zone_area) {
            return Err(BimError::InvalidCell);
        }
        Ok(Tuning { zone_gap: o.zone_gap, min_zone_area: o.min_zone_area })
    }
}

/// The shortest wall the stage will place a seed beside, in metres. A wall shorter than this has
/// no room to stand in and would only put its seeds inside the neighbouring walls.
const EPS_LEN: f64 = 1e-9;

/// Vertex-merging tolerance handed to the region tracer, in metres: the walls were snapped and
/// the soup is deduplicated on a micrometre, so only float noise is left to absorb.
const TRACE_TOL: f64 = 1e-6;

/// How many passes of snap-then-merge run before the axes are taken as final. Two ends that
/// choose each other meet on the first pass; a chain of three needs one more.
const SNAP_PASSES: usize = 3;

/// Where along a wall a seed is tried, as a fraction of its length. The midpoint is first because
/// it is usually free; the rest step outward so a wall crossed by another still offers a free
/// stretch on at least one side.
const STATIONS: [f64; 7] = [0.5, 0.25, 0.75, 0.125, 0.375, 0.625, 0.875];

/// One wall's axis after the ends have been made to meet its neighbours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub from: Vec2,
    pub to: Vec2,
    pub thickness: f64,
}

impl Axis {
    pub fn new(from: Vec2, to: Vec2, thickness: f64) -> Self {
        Self { from, to, thickness }
    }
}

/// The walls that can be traced at all: numbers, some length, and a positive thickness. A wall
/// that fails any of these is a fact the scan cannot hold, not an error for the caller.
fn usable_axes(walls: &[Wall]) -> Vec<Axis> {
    let mut out = Vec::new();
    for w in walls {
        let from = Vec2::new(w.from[0], w.from[1]);
        let to = Vec2::new(w.to[0], w.to[1]);
        if !from.is_finite() || !to.is_finite() || !w.thickness.is_finite() || w.thickness <= 0.0 {
            continue;
        }
        if from.near(to, EPS_LEN) {
            continue;
        }
        out.push(Axis { from, to, thickness: w.thickness });
    }
    out
}

/// Snap wall ends onto the lines of other walls within `gap`, then merge ends that are within
/// `gap` of each other, repeating until nothing moves (bounded by [`SNAP_PASSES`]).
///
/// Projection is onto the *line* of the other wall, allowed up to `gap` past either of its ends:
/// that is what closes a corner where both walls stop short of each other. Merging handles the
/// collinear case, where a projection would be the point itself. A non-positive or non-finite
/// gap is no snapping at all.
fn snap(axes: &mut [Axis], gap: f64) {
    if !gap.is_finite() || gap <= 0.0 || axes.len() < 2 {
        return;
    }
    for _ in 0..SNAP_PASSES {
        let before: Vec<(Vec2, Vec2)> = axes.iter().map(|a| (a.from, a.to)).collect();
        let targets: Vec<(Option<Vec2>, Option<Vec2>)> =
            (0..axes.len()).map(|i| (nearest_line(axes, i, axes[i].from, gap), nearest_line(axes, i, axes[i].to, gap))).collect();
        for (i, (from, to)) in targets.into_iter().enumerate() {
            if let Some(a) = axes.get_mut(i) {
                if let Some(p) = from {
                    a.from = p;
                }
                if let Some(p) = to {
                    a.to = p;
                }
            }
        }
        merge_ends(axes, gap);
        let moved = axes.iter().zip(&before).any(|(a, (f, t))| !a.from.near(*f, 1e-12) || !a.to.near(*t, 1e-12));
        if !moved {
            break;
        }
    }
}

/// The nearest point on another wall's line that is within `gap` of `p`, if any. The projection
/// may fall `gap` past either end of that wall, so two walls that stop short of a corner still
/// meet at it.
fn nearest_line(axes: &[Axis], skip: usize, p: Vec2, gap: f64) -> Option<Vec2> {
    let mut best: Option<(f64, Vec2)> = None;
    for (j, b) in axes.iter().enumerate() {
        if j == skip {
            continue;
        }
        let d = b.to - b.from;
        let len = d.len();
        if !len.is_finite() || len <= EPS_LEN {
            continue;
        }
        let u = d / len;
        let along = (p - b.from).dot(u);
        if !along.is_finite() || along < -gap || along > len + gap {
            continue;
        }
        let q = b.from + u * along;
        let dist = p.dist(q);
        if !dist.is_finite() || dist > gap {
            continue;
        }
        if best.is_none_or(|(bd, _)| dist < bd) {
            best = Some((dist, q));
        }
    }
    best.map(|(_, q)| q)
}

/// Move ends that are within `gap` of each other to the midpoint, so a collinear break in a wall
/// becomes one line. Both ends of a mutual pair land on the same point; a chain converges over
/// the passes [`snap`] allows.
fn merge_ends(axes: &mut [Axis], gap: f64) {
    let mut ends: Vec<(usize, bool, Vec2)> = Vec::with_capacity(axes.len() * 2);
    for (i, a) in axes.iter().enumerate() {
        ends.push((i, true, a.from));
        ends.push((i, false, a.to));
    }
    let mut partner: Vec<Option<usize>> = vec![None; ends.len()];
    for (i, &(ai, _, p)) in ends.iter().enumerate() {
        let mut best: Option<(f64, usize)> = None;
        for (j, &(aj, _, q)) in ends.iter().enumerate() {
            // A wall's own two ends are not a pair to merge.
            if j == i || aj == ai {
                continue;
            }
            let d = p.dist(q);
            if d.is_finite() && d <= gap && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, j));
            }
        }
        partner[i] = best.map(|(_, j)| j);
    }
    // Every midpoint is measured from the positions as they were, before any is applied: two ends
    // that choose each other must land on the same point.
    let moved: Vec<(usize, bool, Vec2)> = ends
        .iter()
        .enumerate()
        .filter_map(|(i, &(axis, from, p))| {
            let j = partner[i]?;
            let (_, _, q) = ends.get(j)?;
            Some((axis, from, (p + *q) / 2.0))
        })
        .collect();
    for (axis, from, p) in moved {
        if let Some(a) = axes.get_mut(axis) {
            if from {
                a.from = p;
            } else {
                a.to = p;
            }
        }
    }
}

/// The rectangle a wall occupies, as a closed ring (`first` repeated) for point containment and
/// for the tracer's soup.
fn rect(a: &Axis) -> Vec<Vec2> {
    let d = a.to - a.from;
    let n = Vec2::new(-d.y, d.x).normalized() * (a.thickness / 2.0);
    let corners = [a.from + n, a.to + n, a.to - n, a.from - n];
    let Some(first) = corners.first().copied() else {
        return Vec::new();
    };
    let mut ring = corners.to_vec();
    ring.push(first);
    ring
}

/// Every wall edge as a two-point polyline, the line soup the region tracer walks. A zero-length
/// edge is dropped: it is not a line and would only put a self-loop in the graph.
/// Project T-junction axis endpoints onto target axis lines and split target axes so T-junctions
/// become explicit vertices in the planar graph.
fn split_t_junctions(axes: &mut Vec<Axis>) {
    // 1. Snap qualifying endpoints to target axis lines.
    let mut new_from: Vec<Option<Vec2>> = vec![None; axes.len()];
    let mut new_to: Vec<Option<Vec2>> = vec![None; axes.len()];

    for i in 0..axes.len() {
        let (from_pt, to_pt) = (axes[i].from, axes[i].to);
        for (is_from, pt) in [(true, from_pt), (false, to_pt)] {
            let mut best: Option<(f64, Vec2)> = None;
            for (j, aj) in axes.iter().enumerate() {
                if j == i {
                    continue;
                }
                let dj = aj.to - aj.from;
                let len_j = dj.len();
                if len_j < 0.10 {
                    continue;
                }
                let uj = dj / len_j;
                let t = (pt - aj.from).dot(uj);
                if t < 0.05 || t > len_j - 0.05 {
                    continue;
                }
                let proj = aj.from + uj * t;
                let d_perp = pt.dist(proj);
                if d_perp <= 0.35 && best.as_ref().is_none_or(|(best_d, _)| d_perp < *best_d) {
                    best = Some((d_perp, proj));
                }
            }
            if let Some((_, proj)) = best {
                if is_from {
                    new_from[i] = Some(proj);
                } else {
                    new_to[i] = Some(proj);
                }
            }
        }
    }

    for i in 0..axes.len() {
        if let Some(p) = new_from[i] {
            axes[i].from = p;
        }
        if let Some(p) = new_to[i] {
            axes[i].to = p;
        }
    }

    // 2. Planar splitting: split each axis at any internal T-junction projection points.
    let mut split_axes: Vec<Axis> = Vec::with_capacity(axes.len() * 2);
    for j in 0..axes.len() {
        let aj = axes[j];
        let dj = aj.to - aj.from;
        let len_j = dj.len();
        if len_j < 0.10 {
            split_axes.push(aj);
            continue;
        }
        let uj = dj / len_j;

        let mut split_ts: Vec<f64> = Vec::new();
        for (k, ak) in axes.iter().enumerate() {
            if k == j {
                continue;
            }
            for pt in [ak.from, ak.to] {
                let t = (pt - aj.from).dot(uj);
                if t >= 0.05 && t <= len_j - 0.05 {
                    let proj = aj.from + uj * t;
                    if pt.dist(proj) <= 1e-3 {
                        split_ts.push(t);
                    }
                }
            }
        }

        if split_ts.is_empty() {
            split_axes.push(aj);
            continue;
        }

        split_ts.sort_by(f64::total_cmp);
        let mut unique_ts: Vec<f64> = Vec::new();
        for t in split_ts {
            if unique_ts.last().is_none_or(|&last_t| (t - last_t).abs() > 0.02) {
                unique_ts.push(t);
            }
        }

        let mut prev_pt = aj.from;
        for t in unique_ts {
            let split_pt = aj.from + uj * t;
            if prev_pt.dist(split_pt) > EPS_LEN {
                split_axes.push(Axis { from: prev_pt, to: split_pt, thickness: aj.thickness });
                prev_pt = split_pt;
            }
        }
        if prev_pt.dist(aj.to) > EPS_LEN {
            split_axes.push(Axis { from: prev_pt, to: aj.to, thickness: aj.thickness });
        }
    }
    *axes = split_axes;
}

/// Close open doorway gaps between axis endpoints or adjacent walls by adding virtual closure
/// segments directly into the region tracer's line soup.
fn add_doorway_closures(axes: &[Axis], soup: &mut Vec<Vec<Vec2>>, max_gap: f64) {
    if axes.is_empty() {
        return;
    }
    let is_connected = |pt: Vec2, axis_idx: usize| -> bool {
        for (j, aj) in axes.iter().enumerate() {
            if j == axis_idx {
                continue;
            }
            if pt.dist(aj.from) <= 0.05 || pt.dist(aj.to) <= 0.05 {
                return true;
            }
            let dj = aj.to - aj.from;
            let len_j = dj.len();
            if len_j > 0.05 {
                let uj = dj / len_j;
                let t = (pt - aj.from).dot(uj);
                if t >= 0.02 && t <= len_j - 0.02 {
                    let proj = aj.from + uj * t;
                    if pt.dist(proj) <= 0.05 {
                        return true;
                    }
                }
            }
        }
        false
    };

    let mut open_ends: Vec<(usize, Vec2)> = Vec::new();
    for (i, a) in axes.iter().enumerate() {
        if !is_connected(a.from, i) {
            open_ends.push((i, a.from));
        }
        if !is_connected(a.to, i) {
            open_ends.push((i, a.to));
        }
    }

    let mut used_ends = vec![false; open_ends.len()];

    // 1. First pair open ends with other open ends across doorway openings (gap in [0.35, max_gap])
    for i in 0..open_ends.len() {
        if used_ends[i] {
            continue;
        }
        let (axis_i, pi) = open_ends[i];
        let mut best: Option<(f64, usize, Vec2)> = None;
        for j in (i + 1)..open_ends.len() {
            if used_ends[j] {
                continue;
            }
            let (axis_j, pj) = open_ends[j];
            if axis_i == axis_j {
                continue;
            }
            let d = pi.dist(pj);
            if d > 0.05 && d <= max_gap && best.as_ref().is_none_or(|(bd, ..)| d < *bd) {
                best = Some((d, j, pj));
            }
        }
        if let Some((_, j, pj)) = best {
            used_ends[i] = true;
            used_ends[j] = true;
            soup.push(vec![pi, pj]);
        }
    }

    // 2. For remaining open ends, check if they meet an adjacent wall line
    for i in 0..open_ends.len() {
        if used_ends[i] {
            continue;
        }
        let (axis_i, pi) = open_ends[i];
        let mut best_wall: Option<(f64, Vec2)> = None;
        for (j, aj) in axes.iter().enumerate() {
            if j == axis_i {
                continue;
            }
            let dj = aj.to - aj.from;
            let len_j = dj.len();
            if len_j > 0.05 {
                let uj = dj / len_j;
                let t = (pi - aj.from).dot(uj);
                if t >= 0.05 && t <= len_j - 0.05 {
                    let proj = aj.from + uj * t;
                    let d = pi.dist(proj);
                    if d > 0.05 && d <= max_gap && best_wall.as_ref().is_none_or(|(bd, _)| d < *bd) {
                        best_wall = Some((d, proj));
                    }
                }
            }
        }
        if let Some((_, proj)) = best_wall {
            used_ends[i] = true;
            soup.push(vec![pi, proj]);
        }
    }
}

fn segments(axes: &[Axis]) -> Vec<Vec<Vec2>> {
    let mut out = Vec::new();
    for a in axes {
        if a.thickness <= 1e-6 {
            if !a.from.near(a.to, EPS_LEN) {
                out.push(vec![a.from, a.to]);
            }
            continue;
        }
        let ring = rect(a);
        for w in ring.windows(2) {
            if let [p, q] = w
                && !p.near(*q, EPS_LEN)
            {
                out.push(vec![*p, *q]);
            }
        }
    }
    out
}

fn is_connected_to_any(pt: Vec2, skip_axis: usize, axes: &[Axis], tol: f64) -> bool {
    for (j, aj) in axes.iter().enumerate() {
        if j == skip_axis {
            continue;
        }
        if pt.dist(aj.from) <= tol || pt.dist(aj.to) <= tol {
            return true;
        }
        let dj = aj.to - aj.from;
        let len_j = dj.len();
        if len_j > 1e-6 {
            let uj = dj / len_j;
            let t = (pt - aj.from).dot(uj);
            if t >= -tol && t <= len_j + tol {
                let proj = aj.from + uj * t.clamp(0.0, len_j);
                if pt.dist(proj) <= tol {
                    return true;
                }
            }
        }
    }
    false
}

fn add_doorway_closures_to_axes(axes: &mut Vec<Axis>, max_gap: f64) {
    if axes.is_empty() {
        return;
    }

    let mut open_ends: Vec<(usize, Vec2)> = Vec::new();
    for (i, a) in axes.iter().enumerate() {
        if !is_connected_to_any(a.from, i, axes, 0.15) {
            open_ends.push((i, a.from));
        }
        if !is_connected_to_any(a.to, i, axes, 0.15) {
            open_ends.push((i, a.to));
        }
    }

    let mut used_ends = vec![false; open_ends.len()];
    let mut closures: Vec<Axis> = Vec::new();

    // 1. Pair open ends across doorways
    for i in 0..open_ends.len() {
        if used_ends.get(i).copied().unwrap_or(true) {
            continue;
        }
        let (axis_i, pi) = open_ends[i];
        let mut best: Option<(f64, usize, Vec2)> = None;
        for (j, &(axis_j, pj)) in open_ends.iter().enumerate().skip(i + 1) {
            if used_ends.get(j).copied().unwrap_or(true) || axis_i == axis_j {
                continue;
            }
            let d = pi.dist(pj);
            if d > 0.05 && d <= max_gap && best.as_ref().is_none_or(|(bd, ..)| d < *bd) {
                best = Some((d, j, pj));
            }
        }
        if let Some((_, j, pj)) = best {
            if let Some(u) = used_ends.get_mut(i) {
                *u = true;
            }
            if let Some(u) = used_ends.get_mut(j) {
                *u = true;
            }
            closures.push(Axis { from: pi, to: pj, thickness: 0.0 });
        }
    }

    // 2. Connect remaining open ends to nearby wall lines
    for (i, &(axis_i, pi)) in open_ends.iter().enumerate() {
        if used_ends.get(i).copied().unwrap_or(true) {
            continue;
        }
        let mut best_wall: Option<(f64, Vec2)> = None;
        for (j, aj) in axes.iter().enumerate() {
            if j == axis_i {
                continue;
            }
            let dj = aj.to - aj.from;
            let len_j = dj.len();
            if len_j > 0.05 {
                let uj = dj / len_j;
                let t = (pi - aj.from).dot(uj);
                if t >= 0.05 && t <= len_j - 0.05 {
                    let proj = aj.from + uj * t;
                    let d = pi.dist(proj);
                    if d > 0.05 && d <= max_gap && best_wall.as_ref().is_none_or(|(bd, _)| d < *bd) {
                        best_wall = Some((d, proj));
                    }
                }
            }
        }
        if let Some((_, proj)) = best_wall {
            if let Some(u) = used_ends.get_mut(i) {
                *u = true;
            }
            closures.push(Axis { from: pi, to: proj, thickness: 0.0 });
        }
    }

    axes.extend(closures);
}

/// Prune degree-1 antenna / dead-end segments from centerline axes before tracing.
pub fn prune_degree_one_axes(axes: &mut Vec<Axis>) {
    let eps = 0.35;
    loop {
        let n = axes.len();
        if n <= 3 {
            break;
        }
        let to_remove: Vec<bool> = (0..n)
            .map(|i| {
                let Some(ai) = axes.get(i) else { return false };
                let from_connected = is_connected_to_any(ai.from, i, axes, eps);
                let to_connected = is_connected_to_any(ai.to, i, axes, eps);
                !from_connected || !to_connected
            })
            .collect();
        if !to_remove.iter().any(|&r| r) {
            break;
        }

        let mut idx = 0;
        axes.retain(|_| {
            let keep = !to_remove.get(idx).copied().unwrap_or(false);
            idx += 1;
            keep
        });
    }
}

/// The zone a traced loop describes, or `None` when the loop is not a room: fewer than three
/// corners, an area that is not a number, or one below the floor.
fn zone_of(outer: &[Vec2], min_area: f64) -> Option<Zone> {
    if outer.len() < 3 {
        return None;
    }
    let area = shoelace(outer).abs();
    if !area.is_finite() || area < min_area {
        return None;
    }
    let perimeter: f64 = outer.iter().zip(outer.iter().cycle().skip(1)).map(|(p, q)| p.dist(*q)).sum();
    if !perimeter.is_finite() {
        return None;
    }
    let ring = outer.iter().map(|p| [p.x, p.y]).collect();
    Some(Zone { ring, area, perimeter })
}

/// True when `p` lies inside the zone's ring; used to skip a seed from a room already found.
fn ring_holds(z: &Zone, p: Vec2) -> bool {
    let ring: Vec<Vec2> = z.ring.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    point_in_polygon(&ring, p)
}

fn centroid(z: &Zone) -> (f64, f64) {
    let n = z.ring.len() as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for p in &z.ring {
        x += p[0];
        y += p[1];
    }
    (x / n, y / n)
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::bim::BimOptions;
    use crate::bim::walls::Wall;

    fn wall(from: [f64; 2], to: [f64; 2], thickness: f64) -> Wall {
        Wall { from, to, thickness, base_z: 0.1, height: 2.8, exterior: false, is_parapet: false }
    }

    /// A 6 × 6 m floor with one cross wall in each direction, so four rooms of 2.8 × 2.8 m once
    /// the 0.2 m walls are accounted for. `gap` trims both walls that meet at the (6, 0) corner,
    /// each by that much, which is the paper's imperfect corner.
    fn four_rooms(gap: f64) -> Vec<Wall> {
        let t = 0.20;
        vec![
            wall([0.0, 0.0], [6.0 - gap, 0.0], t),
            wall([6.0, gap], [6.0, 6.0], t),
            wall([6.0, 6.0], [0.0, 6.0], t),
            wall([0.0, 6.0], [0.0, 0.0], t),
            wall([3.0, 0.0], [3.0, 6.0], t),
            wall([0.0, 3.0], [6.0, 3.0], t),
        ]
    }

    /// A single 4 × 4 m room, axes on the square, `thickness` thick.
    fn one_room(thickness: f64) -> Vec<Wall> {
        vec![
            wall([0.0, 0.0], [4.0, 0.0], thickness),
            wall([4.0, 0.0], [4.0, 4.0], thickness),
            wall([4.0, 4.0], [0.0, 4.0], thickness),
            wall([0.0, 4.0], [0.0, 0.0], thickness),
        ]
    }

    fn bbox(ring: &[[f64; 2]]) -> [f64; 4] {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in ring {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        b
    }

    /// A ring is closed when it has at least three finite vertices, does not repeat its first
    /// vertex at the end (the canonical form this module returns), and encloses something.
    fn ring_is_closed(ring: &[[f64; 2]]) -> bool {
        ring.len() >= 3 && ring.iter().all(|p| p[0].is_finite() && p[1].is_finite()) && ring.first() != ring.last() && ring_area(ring).abs() > 1e-9
    }

    fn ring_area(ring: &[[f64; 2]]) -> f64 {
        let n = ring.len();
        let mut s = 0.0;
        for i in 0..n {
            if let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) {
                s += a[0] * b[1] - b[0] * a[1];
            }
        }
        s / 2.0
    }

    /// No two non-adjacent edges cross, and adjacent edges share only their common endpoint.
    fn ring_is_simple(ring: &[[f64; 2]]) -> bool {
        let n = ring.len();
        if n < 3 {
            return false;
        }
        let edge = |i: usize| -> Option<([f64; 2], [f64; 2])> { Some((*ring.get(i)?, *ring.get((i + 1) % n)?)) };
        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        for i in 0..n {
            let Some((a, b)) = edge(i) else { return false };
            for j in (i + 1)..n {
                // Neighbouring edges are allowed to meet at the vertex they share.
                if j == i || (j + 1) % n == i || (i + 1) % n == j {
                    continue;
                }
                let Some((c, d)) = edge(j) else { return false };
                let (d1, d2) = (cross(a, b, c), cross(a, b, d));
                let (d3, d4) = (cross(c, d, a), cross(c, d, b));
                if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn a_thirty_five_centimetre_corner_gap_still_closes_into_four_rooms() {
        // The paper's own failure: snapping gave up on an imperfect corner and merged two rooms.
        let walls = four_rooms(0.35);
        let zones = build(&walls, &BimOptions::default()).expect("zones");
        assert_eq!(zones.len(), 4, "the gap must not merge two rooms into three: {zones:?}");
        for z in &zones {
            assert!(ring_is_closed(&z.ring) && ring_is_simple(&z.ring), "a zone ring must be closed and simple: {:?}", z.ring);
            assert!(z.area > 0.5, "and enclose something a person could call a room: {}", z.area);
            assert!((z.area - 2.8 * 2.8).abs() < 1e-6, "every room is 2.8 x 2.8 m, got {}", z.area);
        }
    }

    #[test]
    fn four_plain_rooms_give_the_same_four_zones() {
        // The gap test must pass for the right reason: the same walls with no gap answer alike.
        let zones = build(&four_rooms(0.0), &BimOptions::default()).expect("zones");
        assert_eq!(zones.len(), 4, "{zones:?}");
        for z in &zones {
            assert!((z.area - 2.8 * 2.8).abs() < 1e-6, "area {}", z.area);
        }
    }

    #[test]
    fn a_zone_is_inside_the_walls_not_on_the_axes() {
        let thickness = 0.20;
        let zones = build(&one_room(thickness), &BimOptions::default()).expect("a room");
        assert_eq!(zones.len(), 1, "one room, one zone: {zones:?}");
        let z = &zones[0];
        let b = bbox(&z.ring);
        for (got, want) in [(b[0], thickness / 2.0), (b[1], thickness / 2.0), (b[2], 4.0 - thickness / 2.0), (b[3], 4.0 - thickness / 2.0)] {
            assert!((got - want).abs() < 1e-9, "the ring sits on the inner faces: bbox {b:?}");
        }
        assert!((z.area - (4.0 - thickness) * (4.0 - thickness)).abs() < 1e-9, "area {}", z.area);
        assert!((z.perimeter - 4.0 * (4.0 - thickness)).abs() < 1e-9, "perimeter {}", z.perimeter);
        // Every edge lies half a thickness from a wall's axis, not on it.
        let axes = one_room(thickness);
        let n = z.ring.len();
        for i in 0..n {
            let (Some(a), Some(b)) = (z.ring.get(i), z.ring.get((i + 1) % n)) else { continue };
            let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let best = axes
                .iter()
                .map(|w| {
                    let (dx, dy) = (w.to[0] - w.from[0], w.to[1] - w.from[1]);
                    let len = dx.hypot(dy);
                    ((m[0] - w.from[0]) * dy - (m[1] - w.from[1]) * dx).abs() / len
                })
                .fold(f64::INFINITY, f64::min);
            assert!((best - thickness / 2.0).abs() < 1e-3, "an edge is {best:.4} m from an axis, not {:.1} m", thickness / 2.0);
        }
    }

    #[test]
    fn a_zone_below_the_area_floor_is_dropped() {
        let walls = one_room(0.20);
        let dropped = BimOptions { min_zone_area: 100.0, ..BimOptions::default() };
        assert!(build(&walls, &dropped).expect("zones").is_empty(), "14.44 m2 is below a 100 m2 floor");
        let kept = BimOptions { min_zone_area: 0.0, ..BimOptions::default() };
        assert_eq!(build(&walls, &kept).expect("zones").len(), 1, "no floor keeps every closed loop");
    }

    #[test]
    fn a_single_free_standing_wall_encloses_nothing() {
        let walls = vec![wall([0.0, 0.0], [3.0, 0.0], 0.20)];
        assert!(build(&walls, &BimOptions::default()).expect("not an error").is_empty());
    }

    #[test]
    fn no_walls_is_no_zones_not_an_error() {
        assert!(build(&[], &BimOptions::default()).expect("empty is a fact").is_empty());
    }

    #[test]
    fn hostile_options_are_refused_rather_than_guessed() {
        for o in [
            BimOptions { zone_gap: f64::NAN, ..BimOptions::default() },
            BimOptions { zone_gap: -0.1, ..BimOptions::default() },
            BimOptions { zone_gap: f64::INFINITY, ..BimOptions::default() },
            BimOptions { min_zone_area: f64::NAN, ..BimOptions::default() },
            BimOptions { min_zone_area: -1.0, ..BimOptions::default() },
        ] {
            assert!(matches!(build(&one_room(0.2), &o), Err(crate::bim::BimError::InvalidCell)), "{o:?} must be refused, not guessed at");
        }
    }

    #[test]
    fn hostile_walls_are_skipped_not_traced() {
        let mut walls = one_room(0.20);
        walls.push(wall([f64::NAN, 0.0], [1.0, 0.0], 0.2));
        walls.push(wall([0.0, 0.0], [0.0, 0.0], 0.2));
        walls.push(wall([0.0, 0.0], [1.0, 0.0], -0.2));
        walls.push(wall([0.0, 0.0], [1.0, 0.0], f64::INFINITY));
        walls.push(Wall { from: [0.0, 0.0], to: [1.0, 0.0], thickness: 0.2, base_z: f64::NAN, height: f64::NAN, exterior: false, is_parapet: false });
        let zones = build(&walls, &BimOptions::default()).expect("the room still stands");
        assert_eq!(zones.len(), 1, "the unusable walls neither add a zone nor remove the room");
        assert!(zones[0].ring.len() >= 4, "the room is a quadrilateral, not a sliver");
    }

    #[test]
    fn the_same_wall_twice_does_not_break_the_room() {
        // A caller that hands the same wall in twice is not an error; the room is still one room.
        let mut walls = one_room(0.20);
        walls.extend(one_room(0.20));
        let zones = build(&walls, &BimOptions::default()).expect("zones");
        assert_eq!(zones.len(), 1, "a doubled wall is still one wall: {zones:?}");
        assert!((zones[0].area - 3.8 * 3.8).abs() < 1e-9, "area {}", zones[0].area);
    }

    #[test]
    fn zones_come_back_in_a_deterministic_order() {
        let zones = build(&four_rooms(0.0), &BimOptions::default()).expect("zones");
        let again = build(&four_rooms(0.0), &BimOptions::default()).expect("zones");
        assert_eq!(zones, again, "the same walls must trace the same rings in the same order");
        // The four rooms sit at 1.5 and 4.5 in both axes; corners at a micrometre, so a room is
        // not ordered by float noise in its last bit. This is the order a reader expects: column
        // by column, bottom to top.
        let keys: Vec<(i64, i64)> = zones
            .iter()
            .map(|z| {
                let (mut sx, mut sy) = (0.0, 0.0);
                for p in &z.ring {
                    sx += p[0];
                    sy += p[1];
                }
                let n = z.ring.len() as f64;
                (((sx / n) * 1e6).round() as i64, ((sy / n) * 1e6).round() as i64)
            })
            .collect();
        assert_eq!(
            keys,
            vec![(1_500_000, 1_500_000), (1_500_000, 4_500_000), (4_500_000, 1_500_000), (4_500_000, 4_500_000)],
            "sorted by centroid x, then y"
        );
    }

    #[test]
    fn t_junction_splitting_and_dead_end_antenna_pruning() {
        let t = 0.20;
        let walls = vec![
            // Outer 6 x 4 rectangle
            wall([0.0, 0.0], [6.0, 0.0], t),
            wall([6.0, 0.0], [6.0, 4.0], t),
            wall([6.0, 4.0], [0.0, 4.0], t),
            wall([0.0, 4.0], [0.0, 0.0], t),
            // T-junction dividing wall from (3.0, 0.15) to (3.0, 3.85) (gap of 0.15 m <= 0.35 m at each end)
            wall([3.0, 0.15], [3.0, 3.85], t),
        ];

        let zones = build(&walls, &BimOptions::default()).expect("zones should build");
        assert_eq!(zones.len(), 2, "T-junction dividing wall splits the room into 2 zones");
        for z in &zones {
            assert!(ring_is_closed(&z.ring) && ring_is_simple(&z.ring));
            assert!((z.area - (2.8 * 3.8)).abs() < 0.2, "each room is ~2.8 x 3.6 m, got {}", z.area);
        }
    }

    #[test]
    fn doorway_closure_bridges_gap_into_closed_zone() {
        let t = 0.20;
        // A 4x4 room with an open doorway of 0.90m in the bottom wall (from x=1.55 to x=2.45)
        let walls = vec![
            wall([0.0, 0.0], [1.55, 0.0], t),
            wall([2.45, 0.0], [4.0, 0.0], t),
            wall([4.0, 0.0], [4.0, 4.0], t),
            wall([4.0, 4.0], [0.0, 4.0], t),
            wall([0.0, 4.0], [0.0, 0.0], t),
        ];

        let zones = build(&walls, &BimOptions::default()).expect("zones should bridge doorway");
        assert_eq!(zones.len(), 1, "doorway of 0.90m is bridged into a closed room");
        assert!(ring_is_closed(&zones[0].ring));
        assert!((zones[0].area - (3.8 * 3.8)).abs() < 0.3, "room area is preserved: {}", zones[0].area);
    }

    #[test]
    fn degree_one_dead_end_pruning_removes_antennas_and_isolated_segments() {
        use super::{Axis, prune_degree_one_axes};
        use cadcraft_geom::Vec2;

        let mut axes = vec![
            // Closed 4 x 4 loop (edges 0..4)
            Axis::new(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), 0.20),
            Axis::new(Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0), 0.20),
            Axis::new(Vec2::new(4.0, 4.0), Vec2::new(0.0, 4.0), 0.20),
            Axis::new(Vec2::new(0.0, 4.0), Vec2::new(0.0, 0.0), 0.20),
            // Antenna 1: single spur connected at (0.0, 2.0) sticking into room to (1.5, 2.0)
            Axis::new(Vec2::new(0.0, 2.0), Vec2::new(1.5, 2.0), 0.20),
            // Antenna 2: 2-segment chain connected at (4.0, 2.0) sticking outside
            Axis::new(Vec2::new(4.0, 2.0), Vec2::new(5.0, 2.0), 0.20),
            Axis::new(Vec2::new(5.0, 2.0), Vec2::new(6.0, 2.0), 0.20),
            // Isolated segment not connected to anything
            Axis::new(Vec2::new(10.0, 10.0), Vec2::new(12.0, 10.0), 0.20),
        ];

        prune_degree_one_axes(&mut axes);
        assert_eq!(axes.len(), 4, "all antennas and isolated segments pruned, only 4 walls of closed loop remain");
        // Verify only original 4 walls remain
        for a in &axes {
            assert!(a.from.x >= 0.0 && a.from.x <= 4.0 && a.from.y >= 0.0 && a.from.y <= 4.0);
            assert!(a.to.x >= 0.0 && a.to.x <= 4.0 && a.to.y >= 0.0 && a.to.y <= 4.0);
        }
    }

    #[test]
    fn structural_only_filtering_ignores_parapets_and_furniture() {
        let t = 0.20;
        let walls = vec![
            // Full-height 4 x 4 room (height = 2.80 m)
            wall([0.0, 0.0], [4.0, 0.0], t),
            wall([4.0, 0.0], [4.0, 4.0], t),
            wall([4.0, 4.0], [0.0, 4.0], t),
            wall([0.0, 4.0], [0.0, 0.0], t),
            // Parapet wall (is_parapet: true, height: 1.0 m)
            Wall { from: [1.0, 1.0], to: [3.0, 1.0], thickness: 0.20, base_z: 0.0, height: 1.0, exterior: false, is_parapet: true },
            // Partial-height furniture / low wall (is_parapet: false, height: 1.2 m < 2.8 * 0.75 = 2.1 m)
            Wall { from: [2.0, 1.0], to: [2.0, 3.0], thickness: 0.20, base_z: 0.0, height: 1.20, exterior: false, is_parapet: false },
        ];

        let zones = build(&walls, &BimOptions::default()).expect("zones should build");
        assert_eq!(zones.len(), 1, "parapet and low furniture are filtered out; only 1 full room zone traced");
        assert!((zones[0].area - (3.8 * 3.8)).abs() < 0.2, "room area is ~14.4 m2: {}", zones[0].area);
    }
}
