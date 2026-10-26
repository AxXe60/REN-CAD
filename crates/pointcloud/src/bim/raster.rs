//! The metric grid a cloud is counted onto, and the bitmap stages that read it.
//!
//! A raster is one flat `Vec<u32>` of counts over a rectangle divided into square cells. It is
//! the only thing in `bim` that knows metres: the stages after it work on `Vec<bool>` bitmaps and
//! hand their rings back in cell coordinates, which [`Raster::to_world`] turns into metres.
//!
//! The grid is bounded by construction. A scan a hundred kilometres wide asked for a
//! five-centimetre grid is refused at [`Raster::new`] rather than allocated, so no caller has to
//! remember to check.

use cadcraft_geom::{Vec2, shoelace};

/// The largest number of cells one raster holds: 4e6 counts is 16 MB, and a grid that would need
/// more is refused instead of tried.
///
/// The budget is of *counts*, not of the whole build. [`contours`] keeps two edge slots and two
/// used flags per **lattice corner**, and corners are `(w + 1) · (h + 1)` — at most about twice
/// the cell count, which happens when one axis is a single cell — so tracing costs up to ~32 MB
/// of scratch, and [`close`] allocates four `w · h` bitmaps of its own, another 16 MB at the cap.
/// A raster at the budget is a ~64 MB build, not a 16 MB one; size a web-facing memory budget by
/// the former. Every one of those allocations is still refused before it is tried, because all of
/// them are derived from a grid that passed this check.
pub const MAX_RASTER_CELLS: usize = 4_000_000;

/// A uniform metric grid counting the points that land in each cell.
#[derive(Debug, Clone)]
pub struct Raster {
    cols: usize,
    rows: usize,
    cell: f64,
    origin: [f64; 2],
    counts: Vec<u32>,
    total: u64,
    rejected: u64,
}

impl Raster {
    /// The grid over `min..max` with `cell` metres to a side, or `None` when it would not fit: a
    /// non-finite or non-positive cell, an empty or non-finite box, or more than
    /// [`MAX_RASTER_CELLS`] cells. The span is divided by the cell and rounded up, so a partial
    /// cell at the far edge is still a cell.
    pub fn new(min: [f64; 2], max: [f64; 2], cell: f64) -> Option<Raster> {
        if !cell.is_finite() || cell <= 0.0 {
            return None;
        }
        if !min[0].is_finite() || !min[1].is_finite() || !max[0].is_finite() || !max[1].is_finite() {
            return None;
        }
        let cols = cells_across(max[0] - min[0], cell)?;
        let rows = cells_across(max[1] - min[1], cell)?;
        // `checked_mul` rather than `saturating_mul`: on a 32-bit wasm target the product of two
        // two-million-cell axes overflows the index type itself, and an overflow here must mean
        // "refused", never "a smaller grid".
        let cells = cols.checked_mul(rows)?;
        if cells > MAX_RASTER_CELLS {
            return None;
        }
        Some(Raster { cols, rows, cell, origin: min, counts: vec![0; cells], total: 0, rejected: 0 })
    }

    /// Count one point. Returns `false` and changes nothing else when any coordinate of `p` is
    /// not a number, or `p` falls outside the box — a hostile cloud is skipped at the door, never
    /// turned into an index, and never counted in [`total`](Self::total) either, so a report
    /// cannot add up to more than the scan really holds. Every such refusal is tallied in
    /// [`rejected`](Self::rejected), which is the half a hot loop cannot accidentally drop.
    pub fn add(&mut self, p: [f64; 3]) -> bool {
        if !p[2].is_finite() {
            self.rejected = self.rejected.saturating_add(1);
            return false;
        }
        let Some((ix, iy)) = self.to(p[0], p[1]) else {
            // Covers a non-finite x or y and every point outside the box, half-open at the far edge.
            self.rejected = self.rejected.saturating_add(1);
            return false;
        };
        let Some(count) = self.counts.get_mut(iy * self.cols + ix) else {
            self.rejected = self.rejected.saturating_add(1);
            return false;
        };
        // Saturating, so even a cloud with four billion points in one cell cannot wrap a count
        // into a negative-looking bin.
        *count = count.saturating_add(1);
        self.total = self.total.saturating_add(1);
        true
    }

    /// The counts, row-major: cell `(ix, iy)` is `density()[iy * cols + ix]`.
    pub fn density(&self) -> &[u32] {
        &self.counts
    }

    /// `(cols, rows)`.
    pub fn dims(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Metres to a cell side.
    pub fn cell(&self) -> f64 {
        self.cell
    }

    /// The world coordinate of cell `(0, 0)`'s near corner.
    pub fn origin(&self) -> [f64; 2] {
        self.origin
    }

    /// How many points are counted; the ones `add` refused are not in it.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// How many points `add` refused: non-finite coordinates and points outside the box. Design
    /// §7 asks that a hostile cloud be rejected at the door, counted and reported rather than
    /// merely skipped, and `total() + rejected()` is what the caller handed over — a saturated
    /// counter says "at least this many", which is the honest reading of a four-billion-point bin.
    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// The cell containing a world point, or `None` when it is not a number or falls outside.
    pub fn to(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let dx = (x - self.origin[0]) / self.cell;
        let dy = (y - self.origin[1]) / self.cell;
        if !dx.is_finite() || !dy.is_finite() || dx < 0.0 || dy < 0.0 {
            return None;
        }
        if dx >= self.cols as f64 || dy >= self.rows as f64 {
            return None;
        }
        Some((dx.floor() as usize, dy.floor() as usize))
    }

    /// The world centre of a cell. Indices past the grid are not checked: they answer with a
    /// point outside the box, which is arithmetic and never a panic.
    pub fn from(&self, ix: usize, iy: usize) -> [f64; 2] {
        [self.origin[0] + (ix as f64 + 0.5) * self.cell, self.origin[1] + (iy as f64 + 0.5) * self.cell]
    }

    /// Cell corners to metres: a lattice corner `[ix, iy]` becomes `origin + [ix, iy] * cell`.
    ///
    /// This is the only place the boundary rings of [`contours`] meet the metric world; the
    /// tracer itself stays a bitmap routine so it can be tested without a raster behind it.
    pub fn to_world(&self, ring: &[[f64; 2]]) -> Vec<[f64; 2]> {
        ring.iter().map(|p| [self.origin[0] + p[0] * self.cell, self.origin[1] + p[1] * self.cell]).collect()
    }
}

/// How many cells of `cell` metres span `distance`, rounded up, or `None` when the answer is not
/// a usable count: no span at all, not a number, or past the whole budget on one axis alone.
fn cells_across(distance: f64, cell: f64) -> Option<usize> {
    let ratio = distance / cell;
    if !ratio.is_finite() {
        return None;
    }
    let cells = ratio.ceil();
    if cells < 1.0 || cells > MAX_RASTER_CELLS as f64 {
        return None;
    }
    Some(cells as usize)
}

/// The density cut: a cell is set when its count is at least `threshold` times the tallest bin.
///
/// The tallest bin is found once and the comparison is a multiply, so no division by zero is
/// possible: a raster with nothing counted (`total() == 0`) answers with an all-unset vector, and
/// so does a non-finite threshold. A cell with zero points is never set, whatever the threshold,
/// which is what makes a `threshold` of `0.0` here mean "every cell that holds anything at all"
/// rather than "everything" — the raw occupancy view the later stages ask for when they want to
/// see a thin wall at all. That value is reachable only from Rust: the `threshold` knob in
/// [`BimOptions::from_params`](crate::bim::BimOptions::from_params) is a unit ratio and refuses
/// `0.0` (and anything above `1`), because a user who asks for a zero density cut has made a
/// mistake worth reporting rather than choosing a mode.
pub fn mask(raster: &Raster, threshold: f64) -> Vec<bool> {
    let density = raster.density();
    let mut out = vec![false; density.len()];
    let Some(tallest) = density.iter().max().copied() else {
        return out;
    };
    if raster.total() == 0 || tallest == 0 || !threshold.is_finite() {
        return out;
    }
    let cut = f64::from(tallest) * threshold;
    for (count, set) in density.iter().zip(out.iter_mut()) {
        *set = *count > 0 && f64::from(*count) >= cut;
    }
    out
}

/// The morphological close of a bitmap: a square dilation, then the matching square erosion.
///
/// The convention, stated because the alternative silently eats the border: a cell is **dilated**
/// on when any cell within `radius` (Chebyshev, clamped to the array) is set, and a cell is
/// **kept by the erosion** when every cell within `radius` that lies *inside the array* is set —
/// out-of-bounds neighbours are skipped, not counted as empty. With out-of-bounds treated as unset
/// every border cell would be cleared, and a band touching the edge of the scan would never close.
///
/// Each pass is separable, so a radius-`k` close costs `O(w·h·k)`, not `O(w·h·k²)`: dilation and
/// erosion each run over the rows and then the columns, which composes into the square. The
/// window is clamped per axis to the array — `kx` to the columns, `ky` to the rows — so a radius
/// wider than one axis saturates that axis and still reaches its full width along the other; a
/// `40 × 1` raster closed at radius 5 bridges a 7-cell gap along the row. The radius is first
/// clamped to [`MAX_CLOSE_RADIUS`](crate::bim::MAX_CLOSE_RADIUS), the same ceiling the JSON door
/// enforces: this function returns a `Vec`, not a `Result`, so it cannot report the refusal, and a
/// caller that reaches it from Rust with `usize::MAX` must not be handed a loop bound — or an
/// `x + k` — sized by a number no grid could use. A bitmap whose `w × h` does not fit the budget,
/// or is shorter than `w × h`, is refused with an empty vector rather than indexed past its end.
pub fn close(mask: &[bool], w: usize, h: usize, radius: usize) -> Vec<bool> {
    let Some(cells) = w.checked_mul(h) else {
        return Vec::new();
    };
    if w == 0 || h == 0 || cells > MAX_RASTER_CELLS || mask.len() < cells {
        return Vec::new();
    }
    // Per axis, and not `radius.min(w).min(h)`: reducing the radius to the shortest axis would
    // quietly weaken the close on the thin rasters a wall slice produces.
    let radius = radius.min(crate::bim::MAX_CLOSE_RADIUS);
    let (kx, ky) = (radius.min(w), radius.min(h));
    erode(&dilate(mask, w, h, kx, ky), w, h, kx, ky)
}

/// Set a cell when any cell within `kx` along the row or `ky` along the column is set.
fn dilate(mask: &[bool], w: usize, h: usize, kx: usize, ky: usize) -> Vec<bool> {
    let cells = w * h;
    let mut out = vec![false; cells];
    // Rows first, into the same buffer the columns then read: a cell looks along its own row.
    for y in 0..h {
        for x in 0..w {
            let from = x.saturating_sub(kx);
            let to = (x + kx).min(w - 1);
            let set = (from..=to).any(|xx| mask.get(y * w + xx).copied().unwrap_or(false));
            if let Some(cell) = out.get_mut(y * w + x) {
                *cell = set;
            }
        }
    }
    let mut wide = vec![false; cells];
    for y in 0..h {
        for x in 0..w {
            let from = y.saturating_sub(ky);
            let to = (y + ky).min(h - 1);
            let set = (from..=to).any(|yy| out.get(yy * w + x).copied().unwrap_or(false));
            if let Some(cell) = wide.get_mut(y * w + x) {
                *cell = set;
            }
        }
    }
    wide
}

/// Keep a cell only when every cell within `kx` along the row or `ky` along the column is set —
/// skipping the neighbours that fall outside the array.
fn erode(mask: &[bool], w: usize, h: usize, kx: usize, ky: usize) -> Vec<bool> {
    let cells = w * h;
    let mut out = vec![true; cells];
    for y in 0..h {
        for x in 0..w {
            let from = x.saturating_sub(kx);
            let to = (x + kx).min(w - 1);
            let keep = (from..=to).all(|xx| mask.get(y * w + xx).copied().unwrap_or(true));
            if let Some(cell) = out.get_mut(y * w + x) {
                *cell = keep;
            }
        }
    }
    let mut kept = vec![true; cells];
    for y in 0..h {
        for x in 0..w {
            let from = y.saturating_sub(ky);
            let to = (y + ky).min(h - 1);
            let keep = (from..=to).all(|yy| out.get(yy * w + x).copied().unwrap_or(true));
            if let Some(cell) = kept.get_mut(y * w + x) {
                *cell = keep;
            }
        }
    }
    kept
}

/// A closed boundary ring of a bitmap, and whether it bounds the object or one of its holes.
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    /// The ring's corners on the cell lattice, in **cell coordinates** — not metres. A vertex
    /// `[ix, iy]` is the lattice corner shared by the cells around it, so its metre position is
    /// `x = ix as f64 * cell + origin[0]`, which [`Raster::to_world`] applies. `contours` receives
    /// no origin and no cell, so this is as close to the world as the tracer can get.
    ///
    /// The ring is closed by joining the last corner back to the first: the first corner is
    /// **not** repeated at the end, so `n` corners mean `n` unit edges. A writer that wants a
    /// closed polyline (an `LWPOLYLINE`, a `simplify` that appends its closing vertex) adds the
    /// corner itself; one that counts corners as edges must not subtract one.
    pub ring: Vec<[f64; 2]>,
    /// True when the ring bounds a region of set cells (an outside) rather than a hole.
    pub outer: bool,
}

/// The four unit steps of the lattice, numbered so that a rising index is a counter-clockwise
/// quarter turn: `E → N → W → S`.
const E: u8 = 0;
const N: u8 = 1;
const W: u8 = 2;
const S: u8 = 3;
/// An empty edge slot: a lattice corner with no boundary edge leaving in that direction.
const NO_DIR: u8 = u8::MAX;

/// The lattice step a direction travels.
fn step(dir: u8) -> (isize, isize) {
    match dir {
        E => (1, 0),
        N => (0, 1),
        W => (-1, 0),
        S => (0, -1),
        _ => (0, 0),
    }
}

/// The boundary rings of a bitmap, as unit steps on the cell lattice.
///
/// A cell edge is a boundary edge when the neighbour across it is unset or off the array, and
/// every boundary edge is directed so that **the set cell is on its left**. That one choice is
/// what gives `outer` a meaning: an outside then winds counter-clockwise and a hole clockwise,
/// and the sign of [`cadcraft_geom::shoelace`] tells them apart instead of a guess.
///
/// Where two objects touch corner to corner, four boundary edges meet at one lattice corner and
/// the next edge is a choice; the walk takes the smallest counter-clockwise turn available, which
/// keeps each object's ring its own. Do not "fix" that to the clockwise equivalent: it silently
/// inverts every hole in every drawing.
///
/// The rings are the unit steps themselves — nothing here simplifies them. The walk is bounded: a
/// ring may take no more steps than there are boundary edges, and one that does not close is
/// dropped and counted, never looped on. A `w × h` that does not fit the cell budget, or a bitmap
/// shorter than `w × h`, answers with no rings at all.
pub fn contours(mask: &[bool], w: usize, h: usize) -> Vec<Contour> {
    trace(mask, w, h).0
}

/// `contours` plus what it had to throw away: a ring that did not close, a ring too short to be
/// a polygon, or — the case the slot proof below says cannot happen — a boundary edge a corner
/// could not hold.
///
/// The public signature has nowhere to put that count, but a bitmap stage must not lose a ring
/// silently, so it is `pub(crate)`: the pipeline reports it (design §7's "rejected at the door,
/// counted, reported") instead of the test module being its only reader. A nonzero count means a
/// bitmap reached the tracer that its producer did not mean — do not treat it as noise.
pub(crate) fn trace(mask: &[bool], w: usize, h: usize) -> (Vec<Contour>, usize) {
    let Some(cells) = w.checked_mul(h) else {
        return (Vec::new(), 0);
    };
    if w == 0 || h == 0 || cells > MAX_RASTER_CELLS || mask.len() < cells {
        return (Vec::new(), 0);
    }
    let stride = match w.checked_add(1) {
        Some(n) => n,
        None => return (Vec::new(), 0),
    };
    let verts = match stride.checked_mul(h + 1) {
        Some(n) => n,
        None => return (Vec::new(), 0),
    };
    // Is a lattice cell set? Off the array is unset, which is what makes the border of the scan a
    // boundary like any other.
    let filled = |x: i64, y: i64| -> bool {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask.get(y as usize * w + x as usize).copied().unwrap_or(false)
    };

    // One pair of edge slots per lattice corner. Two *perpendicular* directions can never share a
    // corner (each pair would need one cell both set and unset), so the only way to fill both
    // slots is the opposite pair where two objects touch corner to corner, and a third edge at one
    // corner cannot happen. If it ever did, `put` refuses it and the ring is counted as dropped
    // rather than written over an edge or indexing past the pair.
    let mut slots = vec![[NO_DIR, NO_DIR]; verts];
    let mut edges = 0usize;
    let mut dropped = 0usize;
    for y in 0..h {
        for x in 0..w {
            if !filled(x as i64, y as i64) {
                continue;
            }
            // The four sides of the cell: the neighbour across, the corner the directed edge
            // starts from, and the direction that keeps this cell on the left.
            for (nx, ny, sx, sy, dir) in [
                (x as i64, y as i64 - 1, x, y, E),
                (x as i64 + 1, y as i64, x + 1, y, N),
                (x as i64, y as i64 + 1, x + 1, y + 1, W),
                (x as i64 - 1, y as i64, x, y + 1, S),
            ] {
                if !filled(nx, ny) {
                    if put(&mut slots, sx, sy, stride, dir) {
                        edges += 1;
                    } else {
                        dropped += 1;
                    }
                }
            }
        }
    }

    let mut used = vec![[false; 2]; verts];
    let mut rings: Vec<Contour> = Vec::new();
    let budget = edges + 1;
    for vy in 0..=h {
        for vx in 0..=w {
            let start = vy * stride + vx;
            for k in 0..2 {
                let Some(dir) = slots.get(start).and_then(|s| s.get(k)).copied().filter(|d| *d != NO_DIR) else {
                    continue;
                };
                if used.get(start).and_then(|u| u.get(k)).copied().unwrap_or(true) {
                    continue;
                }
                let (ring, closed) = walk(&slots, &mut used, vx, vy, dir, stride, budget);
                if !closed {
                    // The walk hit its step budget or ran out of edges: drop it, do not loop on it.
                    dropped += 1;
                    continue;
                }
                if ring.len() < 3 {
                    // Closed, but too short to bound anything: a polygon needs three corners.
                    // Counted, because the doc above promises every lost ring is.
                    dropped += 1;
                    continue;
                }
                let pts: Vec<Vec2> = ring.iter().map(|p| Vec2::new(p[0], p[1])).collect();
                rings.push(Contour { ring, outer: shoelace(&pts) > 0.0 });
            }
        }
    }
    (rings, dropped)
}

/// Record that lattice corner `(sx, sy)` owns a boundary edge leaving in direction `dir`.
fn put(slots: &mut [[u8; 2]], sx: usize, sy: usize, stride: usize, dir: u8) -> bool {
    let Some(slot) = slots.get_mut(sy * stride + sx) else {
        return false;
    };
    if slot[0] == NO_DIR {
        slot[0] = dir;
        return true;
    }
    if slot[1] == NO_DIR {
        slot[1] = dir;
        return true;
    }
    false
}

/// Follow the ring that starts at `(vx, vy)` travelling `dir`, marking every edge it walks.
/// Returns the corners in order and whether the ring closed back onto its own start edge.
fn walk(slots: &[[u8; 2]], used: &mut [[bool; 2]], vx: usize, vy: usize, dir: u8, stride: usize, budget: usize) -> (Vec<[f64; 2]>, bool) {
    let mut ring: Vec<[f64; 2]> = Vec::new();
    let (start_v, start_d) = (vy * stride + vx, dir);
    let (mut cx, mut cy, mut d) = (vx, vy, dir);
    for _ in 0..budget {
        let v = cy * stride + cx;
        let Some(k) = slot_of(slots, v, d) else {
            return (ring, false);
        };
        if used.get(v).and_then(|u| u.get(k)).copied().unwrap_or(true) {
            // An already-walked edge: the ring is whole only if it is the one we started on.
            return (ring, v == start_v && d == start_d);
        }
        if let Some(slot) = used.get_mut(v).and_then(|u| u.get_mut(k)) {
            *slot = true;
        }
        ring.push([cx as f64, cy as f64]);
        let (dx, dy) = step(d);
        let (Some(nx), Some(ny)) = (cx.checked_add_signed(dx), cy.checked_add_signed(dy)) else {
            return (ring, false);
        };
        (cx, cy) = (nx, ny);
        let Some(nd) = next_dir(slots, ny * stride + nx, d) else {
            return (ring, false);
        };
        d = nd;
    }
    (ring, false)
}

/// Which of a corner's two slots holds the edge travelling `dir`.
fn slot_of(slots: &[[u8; 2]], v: usize, dir: u8) -> Option<usize> {
    let slot = slots.get(v)?;
    if slot[0] == dir {
        Some(0)
    } else if slot[1] == dir {
        Some(1)
    } else {
        None
    }
}

/// The next edge of the walk: of the corner's outgoing edges, the **least** counter-clockwise
/// turn available from the direction we arrived travelling — the sharpest left, not the widest.
/// (Amendment A7; the clockwise equivalent, or the *fullest* left turn, would invert every hole.
/// The two are only the same choice when a corner holds exactly one edge, which is most of them.)
fn next_dir(slots: &[[u8; 2]], v: usize, arrived: u8) -> Option<u8> {
    let slot = slots.get(v)?;
    let mut best: Option<u8> = None;
    let mut best_turn = 4;
    for dir in slot.iter().copied() {
        if dir == NO_DIR {
            continue;
        }
        let turn = (i32::from(dir) - i32::from(arrived)).rem_euclid(4);
        if turn < best_turn {
            best_turn = turn;
            best = Some(dir);
        }
    }
    best
}

/// Douglas-Peucker on a closed ring: the corners a tolerance of `epsilon` metres cannot tell from
/// the straight runs between them are dropped, and every dropped corner stays within `epsilon` of
/// the ring that remains.
///
/// The ring is a list of corners closed by the last one joining the first, the shape
/// [`Contour::ring`] hands over. Running the algorithm over `ring + [ring[0]]` would be wrong: the
/// two anchors would be the same point, the "line" through them a point, and the corners on the
/// diagonals from it would vanish while the opposite corner survived. The cut is therefore taken
/// at the pair **furthest apart** — the first corner and the one furthest from it — and the two
/// chains between them are simplified as open polylines.
///
/// A corner is kept exactly when it lies further than `epsilon` from the chord of the run it
/// belongs to, which is what makes `epsilon` a bound on how far the simplified ring departs from
/// the traced one. A tolerance of zero, below zero, or not a number simplifies nothing: that is
/// the harmless reading of a hostile option, and the JSON door already refuses it. A ring shorter
/// than four corners is already a triangle. A ring holding a coordinate that is not a number — or
/// one whose magnitude is past 1e150 m, where the squares a distance needs would overflow — is
/// returned unchanged, because every comparison against a NaN is false and a "simplification"
/// built on those would silently collapse the ring onto its anchors.
///
/// The recursion is a work stack on the heap, not the thread's: the split depth is the data's
/// choice and a scan may hand this a ring of many thousands of corners.
pub fn simplify(ring: &[[f64; 2]], epsilon: f64) -> Vec<[f64; 2]> {
    let n = ring.len();
    if n < 4 || !epsilon.is_finite() || epsilon <= 0.0 {
        return ring.to_vec();
    }
    let usable = |p: &[f64; 2]| p[0].is_finite() && p[1].is_finite() && p[0].abs() <= 1e150 && p[1].abs() <= 1e150;
    if !ring.iter().all(usable) {
        return ring.to_vec();
    }
    let Some(cut) = farthest_from_first(ring) else {
        return ring.to_vec();
    };
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[cut] = true;
    simplify_chain(&ring[..=cut], epsilon, &mut keep[..=cut]);
    // The other chain runs from the cut back round to the first corner; the closing edge is part
    // of it, so the chain ends on a copy of `ring[0]` that maps to an already-kept corner.
    let mut tail = ring[cut..].to_vec();
    tail.push(ring[0]);
    let mut kept_tail = vec![false; tail.len()];
    simplify_chain(&tail, epsilon, &mut kept_tail);
    for (i, kept) in kept_tail.iter().enumerate() {
        if !*kept {
            continue;
        }
        if let Some(k) = keep.get_mut(cut + i) {
            *k = true;
        }
    }
    ring.iter().zip(keep.iter()).filter(|(_, kept)| **kept).map(|(p, _)| *p).collect()
}

/// The index of the corner furthest from the first, or `None` when the ring has no extent at all.
fn farthest_from_first(ring: &[[f64; 2]]) -> Option<usize> {
    let p0 = ring.first()?;
    let mut best = 0.0;
    let mut at = None;
    for (i, p) in ring.iter().enumerate().skip(1) {
        let d = (p[0] - p0[0]).powi(2) + (p[1] - p0[1]).powi(2);
        if d > best {
            best = d;
            at = Some(i);
        }
    }
    at
}

/// Douglas-Peucker over one open chain, marking the corners to keep.
fn simplify_chain(chain: &[[f64; 2]], epsilon: f64, keep: &mut [bool]) {
    let n = chain.len();
    if n < 3 || keep.len() != n {
        for k in keep.iter_mut() {
            *k = true;
        }
        return;
    }
    keep[0] = true;
    keep[n - 1] = true;
    let limit = epsilon * epsilon;
    let mut stack: Vec<(usize, usize)> = vec![(0, n - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (chain[a], chain[b]);
        let mut worst = limit;
        let mut at = None;
        let Some(middle) = chain.get((a + 1)..b) else {
            continue;
        };
        for (offset, &p) in middle.iter().enumerate() {
            let d = line_distance_sq(p, pa, pb);
            if d > worst {
                worst = d;
                at = Some(a + 1 + offset);
            }
        }
        if let Some(i) = at {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
}

/// The squared distance from `p` to the line through `a` and `b`; to the point when they coincide.
fn line_distance_sq(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 > 0.0 {
        let cross = (p[0] - a[0]) * dy - (p[1] - a[1]) * dx;
        cross * cross / len2
    } else {
        (p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raster_refuses_a_grid_that_would_not_fit() {
        // 100 km across at 0.05 m would be 2e6 x 2e6 cells: 4e12 counts.
        assert!(Raster::new([0.0, 0.0], [100_000.0, 100_000.0], 0.05).is_none(), "a grid that cannot fit is refused, not allocated");
        let r = Raster::new([0.0, 0.0], [6.0, 4.0], 0.05).expect("a room fits");
        assert_eq!(r.dims(), (120, 80), "cells are the span divided by the cell, rounded up");
    }

    #[test]
    fn points_land_in_the_cell_that_contains_them_and_world_round_trips() {
        let mut r = Raster::new([0.0, 0.0], [2.0, 2.0], 0.5).expect("fits");
        r.add([0.1, 0.1, 1.0]);
        r.add([0.1, 0.1, 1.5]);
        r.add([1.6, 1.6, 2.0]);
        r.add([f64::NAN, 0.5, 3.0]);
        r.add([9.0, 9.0, 4.0]); // outside the bounds: skipped, never indexed
        assert_eq!(r.density()[0], 2, "the (0,0) cell holds the two points at 0.1");
        assert_eq!(r.density()[3 * 4 + 3], 1, "the far corner cell holds one");
        assert_eq!(r.total(), 3, "the non-finite and out-of-bounds points are not counted");
        let c = r.from(3, 3);
        assert!((c[0] - 1.75).abs() < 1e-9 && (c[1] - 1.75).abs() < 1e-9, "a cell reports its centre");
    }

    #[test]
    fn the_mask_is_the_density_cut_at_the_tallest_bin() {
        // A wall column at x = 1.05 with ten points in every cell, and one lone speck.
        let mut r = Raster::new([0.0, 0.0], [4.0, 4.0], 0.1).expect("fits");
        for i in 0..40 {
            for k in 0..10 {
                r.add([1.05, f64::from(i) * 0.1, 0.2 * f64::from(k)]);
            }
        }
        r.add([3.0, 3.0, 1.0]);
        let m = mask(&r, 0.5);
        let (cols, _) = r.dims();
        let (wx, wy) = r.to(1.05, 1.05).expect("in");
        assert!(m[wy * cols + wx], "the wall is at the tallest bin");
        let (sx, sy) = r.to(3.0, 3.0).expect("in");
        assert!(!m[sy * cols + sx], "the speck is a tenth of the tallest bin");
    }

    #[test]
    fn closing_bridges_a_one_cell_crack_and_keeps_a_wide_gap_open() {
        let w = 10;
        let mut m = vec![false; w * w];
        for y in 0..w {
            for x in 0..5 {
                m[y * w + x] = true;
            }
            for x in 6..w {
                m[y * w + x] = true;
            }
        }
        let closed = close(&m, w, w, 1);
        assert!(closed.iter().all(|set| *set), "a 1-cell crack at radius 1 closes");
        let mut wide = m.clone();
        for y in 0..w {
            for x in 3..7 {
                wide[y * w + x] = false;
            }
        }
        let kept = close(&wide, w, w, 1);
        assert!(!kept.iter().all(|set| *set), "a 4-cell gap is a real gap, not noise");
    }

    #[test]
    fn a_thin_raster_closes_at_a_radius_wider_than_its_short_axis() {
        // Task 3's wall slice (`slice_from 0.9` to `slice_to 1.0`) over a storey rastered at
        // 0.05 m is a long thin array: here 40 cells across and one row tall. The window is
        // clamped per axis to the array, so a radius-5 close still reaches five cells along
        // the row; it must not be weakened to a radius of one because the short axis is one.
        let w = 40;
        let h = 1;
        // Two bands with a seven-cell gap between them: cells 10..=16 are empty.
        let thin: Vec<bool> = (0..w * h).map(|i| !(10..17).contains(&i)).collect();
        let closed = close(&thin, w, h, 5);
        assert_eq!(closed.len(), w * h);
        assert!(closed.iter().all(|set| *set), "radius 5 reaches five each way, so a 7-cell gap closes");
        // A close is not a fill: the same gap at radius 3 has only six cells of bridge.
        let short_reach = close(&thin, w, h, 3);
        assert!(!short_reach.iter().all(|set| *set), "radius 3 leaves a 7-cell gap open");

        // The companion two-row case: a radius past the short axis here too, and one inside
        // both axes that keeps the square-kernel answer. Together they pin that the clamp is
        // per axis and still there, so the `(x + k).min(w - 1)` window never runs off a row.
        let w2 = 20;
        let h2 = 2;
        // The same shape on two rows: cells 9..=11 of each row are empty.
        let band: Vec<bool> = (0..w2 * h2).map(|i| !(9..12).contains(&(i % w2))).collect();
        let bridged = close(&band, w2, h2, 5);
        assert!(bridged.iter().all(|set| *set), "a 3-cell gap at radius 5 closes on two rows too");
        let kept = close(&band, w2, h2, 1);
        assert!(!kept.iter().all(|set| *set), "radius 1 does not reach across a 3-cell gap");
    }

    #[test]
    fn a_filled_square_gives_one_outer_ring_and_a_hole_gives_two() {
        let w = 20;
        let mut m = vec![false; w * w];
        for y in 4..16 {
            for x in 4..16 {
                m[y * w + x] = true;
            }
        }
        let one = contours(&m, w, w);
        assert_eq!(one.len(), 1, "one object has one boundary");
        assert!(one[0].outer, "and it is the outside, not a hole");
        // A 12x12 block of set cells traced as unit steps walks 48 edges.
        assert_eq!(one[0].ring.len(), 4 * 12, "the ring is the unit steps around the block");

        for y in 8..12 {
            for x in 8..12 {
                m[y * w + x] = false;
            }
        }
        let two = contours(&m, w, w);
        assert_eq!(two.len(), 2, "the block and its hole");
        assert_eq!(two.iter().filter(|c| c.outer).count(), 1, "still one outside");
        assert_eq!(two.iter().filter(|c| !c.outer).count(), 1, "and now one hole");
        // The hole ring is the 4x4 inner block's own perimeter, walked the other way.
        let hole = two.iter().find(|c| !c.outer).expect("the hole");
        assert_eq!(hole.ring.len(), 4 * 4, "a 4x4 hole has sixteen unit edges");
    }

    #[test]
    fn a_ring_of_cell_corners_becomes_metres_through_the_raster() {
        let r = Raster::new([10.0, 20.0], [16.0, 26.0], 0.5).expect("fits");
        let world = r.to_world(&[[4.0, 4.0], [16.0, 4.0]]);
        assert_eq!(world[0], [12.0, 22.0], "corner (4,4) is 4 cells of 0.5 m from the origin");
        assert_eq!(world[1], [18.0, 22.0]);
    }

    #[test]
    fn two_cells_touching_corner_to_corner_stay_two_rings() {
        // The pinch: four boundary edges meet at lattice corner (1, 1), so the next edge is a
        // choice, and the choice is what decides whether the two objects stay themselves.
        let w = 4;
        let mut m = vec![false; w * w];
        m[0] = true; // cell (0, 0)
        m[w + 1] = true; // cell (1, 1): the two share lattice corner (1, 1) and nothing else
        let rings = contours(&m, w, w);
        assert_eq!(rings.len(), 2, "a corner touch is two boundaries, not one figure eight");
        assert!(rings.iter().all(|c| c.outer), "both wind counter-clockwise: both are outsides");
        assert!(rings.iter().all(|c| c.ring.len() == 4), "a lone cell is four unit steps");
        let (count, dropped) = trace(&m, w, w);
        assert_eq!(count.len(), 2);
        assert_eq!(dropped, 0, "nothing here failed to close");
    }

    #[test]
    fn a_hostile_bitmap_is_refused_rather_than_traced() {
        assert!(contours(&[true; 8], 3, 3).is_empty(), "a bitmap shorter than w x h is refused");
        assert!(contours(&[], 0, 0).is_empty(), "no grid has no rings");
        assert!(close(&[true; 8], 3, 3, 1).is_empty(), "the close refuses the same short bitmap");
        assert!(close(&[true; 9], 3, 3, usize::MAX).len() == 9, "an absurd radius is clamped to the grid");
        let over = MAX_RASTER_CELLS + 1;
        assert!(contours(&[true], over, 1).is_empty(), "a grid past the cell budget is refused, not traced");
        assert!(close(&[true], over, 1, 1).is_empty(), "and so is the close over it");
        let (rings, dropped) = trace(&[false; 20], 4, 5);
        assert!(rings.is_empty() && dropped == 0, "an all-unset bitmap has no boundary at all");
    }

    #[test]
    fn an_empty_raster_masks_to_nothing_and_never_divides_by_zero() {
        let r = Raster::new([0.0, 0.0], [1.0, 1.0], 0.25).expect("fits");
        let m = mask(&r, 0.01);
        assert_eq!(m.len(), 16);
        assert!(m.iter().all(|set| !*set), "nothing counted means nothing set");
        assert!(contours(&m, 4, 4).is_empty(), "and the tracer finds no ring in it");
        assert!(mask(&r, f64::NAN).iter().all(|set| !*set), "a threshold that is not a number sets nothing");
    }

    #[test]
    fn the_raster_counts_every_point_it_refuses() {
        // Design §7: hostile coordinates are rejected at the door, counted and reported. `add`'s
        // `bool` is easy for a hot loop to ignore, so the raster keeps the tally itself.
        let mut r = Raster::new([0.0, 0.0], [1.0, 1.0], 0.5).expect("fits");
        assert_eq!(r.rejected(), 0, "a fresh raster has refused nothing");
        r.add([f64::NAN, 0.1, 0.0]); // x is not a number
        r.add([0.1, f64::NAN, 0.0]); // y is not a number
        r.add([0.1, 0.1, f64::NAN]); // nor is the height
        r.add([-0.1, 0.1, 0.0]); // outside on the near side
        r.add([0.1, -0.1, 0.0]);
        r.add([1.0, 0.1, 0.0]); // x of max is past the last cell
        r.add([0.1, 1.5, 0.0]); // well outside on y
        assert_eq!(r.rejected(), 7, "every point `add` refused is counted");
        assert!(r.add([0.1, 0.1, 0.0]), "a good point still lands");
        assert_eq!(r.rejected(), 7, "and an accepted point does not move the tally");
        assert_eq!(r.total(), 1);
        assert_eq!(r.total() + r.rejected(), 8, "counted plus rejected is what the caller handed over: a report cannot lose a point");
    }

    #[test]
    fn a_raster_refuses_hostile_bounds_and_cells() {
        assert!(Raster::new([0.0, 0.0], [1.0, 1.0], 0.0).is_none(), "a cell of zero is no grid");
        assert!(Raster::new([0.0, 0.0], [1.0, 1.0], f64::NAN).is_none(), "nor is a cell that is not a number");
        assert!(Raster::new([0.0, 0.0], [1.0, 1.0], f64::INFINITY).is_none(), "nor an infinite one");
        assert!(Raster::new([1.0, 1.0], [0.0, 0.0], 0.5).is_none(), "an inverted box has no span");
        assert!(Raster::new([0.0, 0.0], [0.0, 0.0], 0.5).is_none(), "nor does an empty one");
        assert!(Raster::new([f64::NAN, 0.0], [1.0, 1.0], 0.5).is_none(), "a bound that is not a number");
        let r = Raster::new([0.0, 0.0], [0.2, 0.2], 5.0).expect("one cell");
        assert_eq!(r.dims(), (1, 1), "a cell wider than the box still gives one cell, not zero");
        // Half-open cells: the far edge of the box belongs to the grid beyond it, so it is outside.
        let mut room = Raster::new([0.0, 0.0], [1.0, 1.0], 0.5).expect("fits");
        assert_eq!(room.dims(), (2, 2));
        assert!(room.to(1.0, 0.5).is_none(), "x of max is past the last cell");
        assert!(!room.add([1.0, 0.5, 0.0]), "and add reports the point it refused");
        assert!(room.to(f64::INFINITY, 0.0).is_none(), "a coordinate that is not a number is never an index");
        assert!(!room.add([0.4, 0.4, f64::NAN]), "a height that is not a number is refused too: nothing non-finite is counted");
        assert_eq!(room.total(), 0, "and neither refused point reached a count");
        assert!(room.add([0.4, 0.4, 0.9]), "the point that is wholly a number lands");
        assert_eq!(room.total(), 1);
    }

    #[test]
    fn a_radius_past_the_budget_closes_exactly_like_the_budget() {
        // P1, carried from Task 1: `close` is public, and a direct Rust caller can hand it any
        // `usize`. The JSON door refuses a radius past `MAX_CLOSE_RADIUS`, but the primitive must
        // not spend a loop bound it never agreed to. Clamping is observable: an absurd radius has
        // to give the same bitmap as the budget's own radius, whatever the grid.
        let (w, h) = (400usize, 3usize);
        let mut m = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                m[y * w + x] = !(125..275).contains(&x);
            }
        }
        assert_eq!(
            close(&m, w, h, usize::MAX),
            close(&m, w, h, crate::bim::MAX_CLOSE_RADIUS),
            "an unbounded radius must be the budget's radius, not the grid's width"
        );
    }

    /// The corners of a square ring walked in unit steps, the shape [`contours`] hands over.
    fn square_ring(lo: f64, hi: f64) -> Vec<[f64; 2]> {
        let side = (hi - lo) as usize;
        let mut ring = Vec::new();
        for i in 0..side {
            ring.push([lo + i as f64, lo]);
        }
        for i in 0..side {
            ring.push([hi, lo + i as f64]);
        }
        for i in 0..side {
            ring.push([hi - i as f64, hi]);
        }
        for i in 0..side {
            ring.push([lo, hi - i as f64]);
        }
        ring
    }

    /// The distance from `p` to the segment `a`–`b`: Douglas-Peucker bounds the distance to the
    /// line through an anchor pair, which is never shorter than this, so a test that holds here
    /// holds for the promise the stage makes.
    fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        if len2 == 0.0 {
            return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
        }
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
        ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
    }

    #[test]
    fn a_ring_of_unit_steps_simplifies_to_its_corners() {
        // The tracer's output is one vertex per cell corner: a 12 x 12 block is 48 corners that
        // describe four walls. The tolerance is well under the half millimetre a lattice edge
        // needs to survive, so the corners are all the ring should keep.
        let ring = square_ring(4.0, 16.0);
        assert_eq!(ring.len(), 48);
        assert_eq!(simplify(&ring, 0.02), vec![[4.0, 4.0], [16.0, 4.0], [16.0, 16.0], [4.0, 16.0]], "the corners, in the ring's own order");
        // The anchors are the first corner and the one furthest from it, not the first corner
        // twice: a naive closed-ring run would cut the ring at a point and drop the two corners
        // that lie on the diagonals from it.
        assert_eq!(simplify(&square_ring(0.0, 8.0), 0.0), square_ring(0.0, 8.0), "a zero tolerance keeps every lattice step");
    }

    #[test]
    fn every_dropped_vertex_lies_within_epsilon_of_the_kept_ring() {
        // The guarantee the wall axes rest on. A 200-gon at radius 5 has a 2 cm sagitta at about
        // 35 corners, so the ring must lose most of its vertices and still be within 2 cm
        // everywhere.
        let n = 200;
        let ring: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                [5.0 * a.cos(), 5.0 * a.sin()]
            })
            .collect();
        let eps = 0.02;
        let out = simplify(&ring, eps);
        assert!(out.len() >= 3, "a ring stays a ring");
        assert!(out.len() < n / 2, "a 200-gon needs far fewer than half its corners at 2 cm, got {}", out.len());
        for p in &ring {
            let d = (0..out.len()).map(|i| to_segment(*p, out[i], out[(i + 1) % out.len()])).fold(f64::INFINITY, f64::min);
            assert!(d <= eps + 1e-9, "a dropped vertex is {d:.4} from the ring the stage kept");
        }
    }

    #[test]
    fn a_tolerance_that_is_not_a_usable_number_simplifies_nothing() {
        let ring = square_ring(0.0, 8.0);
        for hostile in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
            assert_eq!(simplify(&ring, hostile), ring, "{hostile} must read as no simplification at all");
        }
        assert_eq!(simplify(&ring[..3], 10.0), ring[..3], "a triangle has no interior vertex to drop");
        assert_eq!(simplify(&ring[..2], 10.0), ring[..2], "and a pair is not a ring");
        assert!(simplify(&[], 0.02).is_empty(), "nor is nothing");
    }

    #[test]
    fn a_ring_with_a_coordinate_that_is_not_a_number_is_returned_untouched() {
        // Douglas-Peucker compares distances, and every comparison against a NaN is false: the
        // honest degradation is to hand the ring back, not to collapse it onto two anchors.
        let same = |a: &[[f64; 2]], b: &[[f64; 2]]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(p, q)| {
                    let same_cell = |u: f64, v: f64| u == v || (u.is_nan() && v.is_nan());
                    same_cell(p[0], q[0]) && same_cell(p[1], q[1])
                })
        };
        let mut ring = square_ring(0.0, 8.0);
        ring[5] = [f64::NAN, 3.0];
        assert!(same(&simplify(&ring, 0.02), &ring), "a NaN corner hands the whole ring back");
        // A coordinate whose square overflows is the same class of number: the distances that
        // decide a split are no longer numbers, so the ring is handed back as it came.
        let mut huge = square_ring(0.0, 8.0);
        huge[7] = [1e200, 3.0];
        assert!(same(&simplify(&huge, 0.02), &huge), "a coordinate past 1e150 m is no lattice corner, and is refused the same way");
        let repeated = vec![[1.0, 1.0]; 6];
        assert_eq!(simplify(&repeated, 0.02), repeated, "a ring with no extent keeps its corners and its length");
    }

    #[test]
    fn a_long_zigzag_terminates_without_driving_the_stack_from_the_data() {
        // Twenty thousand vertices that all matter: a recursive Douglas-Peucker would nest as deep
        // as the split tree, which on this shape is the data's choice. The work stack is explicit,
        // so the depth is the heap's, not the thread's.
        let n = 20_000;
        let ring: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                let r = if i % 2 == 0 { 1.0 } else { 3.0 };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let out = simplify(&ring, 0.01);
        assert!(out.len() <= n && out.len() >= 3, "the zigzag survives as a ring, got {}", out.len());
    }
}
