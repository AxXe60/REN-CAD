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
    let lo = storey.base_z + t.slice_from * height;
    let hi = storey.base_z + t.slice_to * height;
    let band: Vec<[f64; 3]> = storeys::points_in(points, storey, opts).into_iter().filter(|p| p[2] >= lo && p[2] <= hi).collect();
    if band.is_empty() {
        return Ok(Vec::new());
    }
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for p in &band {
        for k in 0..2 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
    }
    // The far edge gets padding because a raster is half-open there: the slice's own extreme point
    // must land, not fall just past the last cell. The rest of the padding is the close's: its
    // erosion treats cells beyond the array as "keep" (that is what stops a band on the scan's
    // edge from being eaten), so a shape near the array would otherwise *grow* into the padding.
    // Empty cells all round make the square erosion exact, and the padding is a whole number of
    // cells, so the lattice stays on the world's cell lines.
    let pad = (2 * t.close_radius + 2) as f64 * t.cell;
    let origin = [(low[0] / t.cell).floor() * t.cell - pad, (low[1] / t.cell).floor() * t.cell - pad];
    let far = [high[0] + pad, high[1] + pad];
    let Some(mut raster) = Raster::new(origin, far, t.cell) else {
        return Err(BimError::TooLarge { what: "wall raster", limit: raster::MAX_RASTER_CELLS });
    };
    for p in &band {
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
    // A tolerance cannot be finer than the lattice it is applied to: a 45-degree face can only be
    // placed on the nearest lattice corner, up to half a cell diagonal (cell/√2) away. The paper's
    // 2 cm works against its own 5 px/mm grid, not against a 5 cm one: at 2 cm the simplifier keeps
    // the staircase instead of the wall, and the staircase's steps are shorter than any wall. The
    // floor is a whole cell rather than the half diagonal, so the comparison is never on its
    // boundary, and nothing finer than a cell was resolvable in the first place.
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

    let (mut walls, used) = pair_faces(&faces, &t, storey.base_z, height)?;
    if !t.exterior_scan {
        for (i, s) in faces.iter().enumerate() {
            if used.get(i).copied().unwrap_or(true) || !outer.get(s.ring).copied().unwrap_or(false) {
                continue;
            }
            // The tracer keeps the set cells on the left of every ring, so for an outer ring the
            // right-hand normal is the side with no wall body on it — the outside of the building.
            // A hole ring's right-hand normal points into the room, and a wall grown there would
            // eat the space the scan measured, so holes are not used.
            let (nx, ny) = (s.dir[1], -s.dir[0]);
            let shift = 0.5 * t.exterior_wall_thickness;
            walls.push(Wall {
                from: [s.a[0] + nx * shift, s.a[1] + ny * shift],
                to: [s.b[0] + nx * shift, s.b[1] + ny * shift],
                thickness: t.exterior_wall_thickness,
                base_z: storey.base_z,
                height,
                exterior: true,
            });
        }
    }
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
    Some((Wall { from, to, thickness, base_z, height, exterior: false }, overlap))
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
    use super::{Wall, extract};
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
                    z += 0.1;
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
        let levels = [2.70, 2.80, 2.90];
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
                while z <= 1.3 + 1e-9 {
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
        assert_eq!(high.len(), bare.len(), "the panel is 1.3 m tall and the slice starts at {:.2} m", 2.62);
        assert_same_walls(&high, &bare);
        // The defence is the slice, not the panel's shape: a slice through the floor sees it.
        let low = BimOptions { slice_from: 0.0, slice_to: 0.1, ..o };
        let floor = extract(&furnished, &storey_at(0.0, 3.0), &low).expect("walls");
        assert!(floor.len() > bare.len(), "a slice at the floor must see the panel, or this test proves nothing: {}", floor.len());
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
}
