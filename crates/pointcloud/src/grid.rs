//! A uniform grid over a set of points, with the buckets in one flat array.
//!
//! One million cells cost one allocation, not a million small ones: every cell owns a run of the
//! flat array and the runs are laid out back to back. The grid is what the comparison, the
//! classification and the clustering look the neighbours up in.
//!
//! The cell key is one mixed `u64`, so a lookup costs one hash probe instead of a triple of
//! comparisons. Two cells that mix to the same key share a run: the points of both are then found
//! together, which costs a little extra work but never a wrong answer.

/// The mix of the three cell coordinates into one key.
fn mix(x: i64, y: i64, z: i64) -> u64 {
    (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
}

/// The table slot a key probes first: the golden multiply spreads the high bits of the key over a
/// table of two-to-the-`shift` slots, and linear probing does the rest.
fn slot(key: u64, shift: u32) -> usize {
    (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> shift) as usize
}

/// A grid over a set of points.
pub struct Grid {
    inv: f64,
    /// The cell keys, stored plus one: a zero is a free slot, so a probe stops at the key it
    /// wants or at the zero behind it.
    keys: Vec<u64>,
    /// For every key: where its run starts and how long it is.
    runs: Vec<(u32, u32)>,
    /// The probe of the key: the multiply and the shift that pick the first slot.
    shift: u32,
    /// One less than the table length, masked onto the probe to walk it around.
    mask: usize,
    /// The points of every cell, back to back.
    buckets: Vec<u32>,
}

impl Grid {
    /// The grid over the points; `cell` is the size of a cell in metres.
    ///
    /// A cell that is not a number, or not positive, falls back to a metre.
    pub fn new(points: &[[f64; 3]], cell: f64) -> Self {
        let cell = if cell > 0.0 && cell.is_finite() { cell } else { 1.0 };
        let inv = 1.0 / cell;
        // The cells are counted in one open table: a probe is a multiply and a shift, and the
        // table grows by doubling so it stays half empty and the probes stay short.
        let mut table = Counting::new(points.len().min(1 << 21));
        for p in points {
            table.add(mix((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64, (p[2] * inv).floor() as i64));
        }
        // The runs are laid out back to back: the first number of a cell is where its run ends, the
        // second how long it is. Walking the table in slot order fixes one order for the runs, so
        // the same points build the same grid every time.
        let mut total = 0u32;
        for (k, v) in table.keys.iter().zip(table.runs.iter_mut()) {
            if *k != 0 {
                v.0 = total + v.1;
                total += v.1;
            }
        }
        let mut grid = Self { inv, keys: table.keys, runs: table.runs, shift: table.shift, mask: table.mask, buckets: vec![0u32; points.len()] };
        for (i, p) in points.iter().enumerate() {
            let k = mix((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64, (p[2] * inv).floor() as i64);
            let j = grid.find(k);
            let v = &mut grid.runs[j];
            // The points fill the run from its end back: the same walk as before the flat table,
            // so every point keeps its place.
            v.0 = v.0.saturating_sub(1);
            grid.buckets[v.0 as usize] = i as u32;
        }
        grid
    }

    /// The indices of the points in the 27 cells around a point, gathered into the caller's buffer.
    ///
    /// The buffer is reused from one call to the next: a hot loop must not allocate.
    pub fn around(&self, p: [f64; 3], out: &mut Vec<u32>) {
        out.clear();
        let cx = (p[0] * self.inv).floor() as i64;
        let cy = (p[1] * self.inv).floor() as i64;
        let cz = (p[2] * self.inv).floor() as i64;
        for dx in -1i64..=1 {
            let x = cx + dx;
            for dy in -1i64..=1 {
                let y = cy + dy;
                for dz in -1i64..=1 {
                    let i = self.find(mix(x, y, cz + dz));
                    if self.keys[i] == 0 {
                        continue;
                    }
                    let (start, count) = self.runs[i];
                    out.extend_from_slice(&self.buckets[start as usize..(start + count) as usize]);
                }
            }
        }
    }

    /// The slot of a key: the first probe, then a step or two along the collision run.
    fn find(&self, key: u64) -> usize {
        let want = key.wrapping_add(1);
        let mut i = slot(key, self.shift);
        while self.keys[i] != want && self.keys[i] != 0 {
            i = (i + 1) & self.mask;
        }
        i
    }
}

/// The open table while the grid counts its cells: one key with a point count per slot, grown by
/// doubling to keep the table half empty. It becomes the grid's own lookup afterwards.
struct Counting {
    keys: Vec<u64>,
    runs: Vec<(u32, u32)>,
    /// How many slots the keys fill.
    used: usize,
    shift: u32,
    mask: usize,
}

impl Counting {
    /// Room for a table of about the size of the point set, rounded up to a power of two.
    fn new(hint: usize) -> Self {
        let len = hint.max(4).next_power_of_two();
        Self { keys: vec![0; len], runs: vec![(0, 0); len], used: 0, shift: 64 - len.trailing_zeros(), mask: len - 1 }
    }

    /// One more point in the key's cell, growing the table first when it reaches half full.
    fn add(&mut self, key: u64) {
        if self.used * 2 >= self.keys.len() {
            self.double();
        }
        let i = self.find(key);
        if self.keys[i] == 0 {
            self.keys[i] = key.wrapping_add(1);
            self.used += 1;
        }
        self.runs[i].1 = self.runs[i].1.saturating_add(1);
    }

    /// The slot of a key: the first probe, then a step or two along the collision run.
    fn find(&self, key: u64) -> usize {
        let want = key.wrapping_add(1);
        let mut i = slot(key, self.shift);
        while self.keys[i] != want && self.keys[i] != 0 {
            i = (i + 1) & self.mask;
        }
        i
    }

    /// Twice the table, the keys carried over by probing them anew.
    fn double(&mut self) {
        let old_keys = std::mem::take(&mut self.keys);
        let old_runs = std::mem::take(&mut self.runs);
        let len = old_keys.len().saturating_mul(2);
        self.keys = vec![0; len];
        self.runs = vec![(0, 0); len];
        self.shift = 64 - len.trailing_zeros();
        self.mask = len - 1;
        for (k, r) in old_keys.into_iter().zip(old_runs) {
            if k != 0 {
                let i = self.find(k.wrapping_sub(1));
                self.keys[i] = k;
                self.runs[i] = r;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_finds_its_neighbours() {
        let points = vec![[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [9.0, 0.0, 0.0]];
        let grid = Grid::new(&points, 1.0);
        let mut out = Vec::new();
        grid.around([0.0, 0.0, 0.0], &mut out);
        assert_eq!(out.len(), 2, "the two points of the first cell");
        assert!(out.contains(&0) && out.contains(&1));
        grid.around([9.0, 0.0, 0.0], &mut out);
        assert_eq!(out.len(), 1);
        assert!(out.contains(&2));
    }

    #[test]
    fn a_point_far_from_everything_finds_nothing() {
        let points = vec![[0.0, 0.0, 0.0]];
        let grid = Grid::new(&points, 1.0);
        let mut out = Vec::new();
        grid.around([50.0, 50.0, 50.0], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn every_point_is_in_its_own_cell() {
        let points: Vec<[f64; 3]> = (0..100).map(|i| [i as f64 * 0.1, 0.0, 0.0]).collect();
        let grid = Grid::new(&points, 1.0);
        let mut out = Vec::new();
        for (i, p) in points.iter().enumerate() {
            grid.around(*p, &mut out);
            assert!(out.contains(&(i as u32)), "point {i} finds itself");
        }
    }
}
