//! A uniform grid over a set of points, with the buckets in one flat array.
//!
//! One million cells cost one allocation, not a million small ones: every cell owns a run of the
//! flat array and the runs are laid out back to back. The grid is what the comparison, the
//! classification and the clustering look the neighbours up in.
//!
//! The cell key is one mixed `u64`, so a lookup costs one hash probe instead of a triple of
//! comparisons. Two cells that mix to the same key share a run: the points of both are then found
//! together, which costs a little extra work but never a wrong answer.

use std::collections::HashMap;

/// The mix of the three cell coordinates into one key.
fn mix(x: i64, y: i64, z: i64) -> u64 {
    (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
}

/// A grid over a set of points.
pub struct Grid {
    inv: f64,
    /// For every cell: where its run starts and how long it is.
    cells: HashMap<u64, (u32, u32)>,
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
        let mut cells: HashMap<u64, (u32, u32)> = HashMap::with_capacity(points.len().min(1 << 21));
        for p in points {
            let k = mix((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64, (p[2] * inv).floor() as i64);
            cells.entry(k).or_insert((0, 0)).1 += 1;
        }
        // The runs are laid out back to back: the first number of a cell is where its run ends, the
        // second how long it is.
        let mut total = 0u32;
        for v in cells.values_mut() {
            v.0 = total + v.1;
            total += v.1;
        }
        let mut buckets = vec![0u32; points.len()];
        for (i, p) in points.iter().enumerate() {
            let k = mix((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64, (p[2] * inv).floor() as i64);
            if let Some(v) = cells.get_mut(&k) {
                v.0 = v.0.saturating_sub(1);
                buckets[v.0 as usize] = i as u32;
            }
        }
        Self { inv, cells, buckets }
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
            for dy in -1i64..=1 {
                for dz in -1i64..=1 {
                    let k = mix(cx + dx, cy + dy, cz + dz);
                    let Some(&(start, count)) = self.cells.get(&k) else { continue };
                    out.extend_from_slice(&self.buckets[start as usize..(start + count) as usize]);
                }
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
