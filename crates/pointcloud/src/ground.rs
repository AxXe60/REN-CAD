//! Finding the ground under a cloud.
//!
//! A survey scan is mostly furniture, vegetation and noise; the floor is what a drawing is
//! measured from, and the LAS classification codes of the file are often missing or wrong. The
//! filter here is the morphological one every GIS tool uses, with the slope correction a bare
//! earth surface needs:
//!
//! 1. the points are binned into a grid and the lowest point of every cell seeds a surface;
//! 2. a seed far above its neighbours (a table, a car) is dropped, so it does not drag the
//!    surface up;
//! 3. the surface of every cell is a plane fitted through the seeds of its 3 × 3 neighbourhood,
//!    so a sloped floor stays a sloped floor instead of a staircase;
//! 4. a point is the ground when it sits on that plane; a point below it is noise, and a point
//!    far above its neighbours is noise too.

use std::collections::HashMap;

/// The LAS classification codes the fork writes.
pub const CLASS_UNCLASSIFIED: u8 = 1;
pub const CLASS_GROUND: u8 = 2;
/// The LAS "high noise" code.
pub const CLASS_NOISE: u8 = 18;

/// How the ground is found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundOptions {
    /// The size of the cells the surface is built on, in metres.
    pub cell: f64,
    /// How far above the surface a point may sit and still be the ground, in metres.
    pub tolerance: f64,
    /// How far above its neighbours a seed may sit before it is dropped as a bump, in metres.
    pub bump: f64,
    /// How many standard deviations above the local heights a point may sit before it is noise.
    /// 0 turns the outlier cut off.
    pub sigma: f64,
}

impl Default for GroundOptions {
    fn default() -> Self {
        // Half a metre cells and a 15 cm tolerance: a floor inside a building is flat enough for
        // that, and the plane fit follows the slope.
        Self { cell: 0.5, tolerance: 0.15, bump: 0.3, sigma: 3.0 }
    }
}

/// What the filter found.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroundResult {
    pub ground: usize,
    pub noise: usize,
    pub unclassified: usize,
}

/// Classify every point: the ground, the noise, the rest.
///
/// The points are `[x, y, z]` in world metres; the result is one LAS classification code per
/// point, in the same order.
pub fn classify(points: &[[f64; 3]], options: &GroundOptions) -> (Vec<u8>, GroundResult) {
    classify_with(points, options, |p| *p)
}

/// The same, over any point type: the cloud's own vertices are read in place, without copying
/// four million points into a new vector first.
pub fn classify_with<T>(points: &[T], options: &GroundOptions, at: impl Fn(&T) -> [f64; 3]) -> (Vec<u8>, GroundResult) {
    let n = points.len();
    let mut classes = vec![CLASS_UNCLASSIFIED; n];
    let mut result = GroundResult::default();
    if n == 0 || !options.cell.is_finite() || options.cell <= 0.0 {
        result.unclassified = n;
        return (classes, result);
    }

    // 1. The seeds: the lowest point of every cell.
    let inv = 1.0 / options.cell;
    let mut cells: HashMap<(i64, i64), f64> = HashMap::new();
    for p in points {
        let p = at(p);
        let key = cell_of(p, inv);
        let e = cells.entry(key).or_insert(p[2]);
        *e = e.min(p[2]);
    }

    // 2. The bumps: a seed far above its neighbours is not the ground and does not fit the plane.
    let seeds: HashMap<(i64, i64), f64> = cells
        .iter()
        .filter(|(key, z)| {
            let neighbours = neighbours(&cells, **key).map(|(_, nz)| nz).collect::<Vec<_>>();
            if neighbours.is_empty() {
                return true;
            }
            let median = median(&neighbours);
            **z <= median + options.bump
        })
        .map(|(&key, &z)| (key, z))
        .collect();

    // 3. The surface: a plane through the seeds of every 3 x 3 neighbourhood.
    let mut surface: HashMap<(i64, i64), f64> = HashMap::with_capacity(cells.len());
    for key in cells.keys() {
        let neighbourhood: Vec<(f64, f64, f64)> =
            neighbours(&seeds, *key).map(|(k, z)| ((k.0 as f64 + 0.5) / inv, (k.1 as f64 + 0.5) / inv, z)).collect();
        let (x0, y0) = ((key.0 as f64 + 0.5) / inv, (key.1 as f64 + 0.5) / inv);
        let value = match fit_plane(&neighbourhood) {
            Some((a, b, c)) => a + b * x0 + c * y0,
            None => neighbourhood.iter().map(|p| p.2).fold(f64::NEG_INFINITY, f64::max),
        };
        if value.is_finite() {
            surface.insert(*key, value);
        }
    }

    // 4. The classification: the ground sits on the surface, a point under it is noise.
    let mut heights: HashMap<(i64, i64), (f64, f64, usize)> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        let p = at(p);
        let key = cell_of(p, inv);
        let Some(s) = surface.get(&key) else { continue };
        let h = p[2] - s;
        if h < -options.tolerance {
            classes[i] = CLASS_NOISE;
            result.noise += 1;
        } else if h <= options.tolerance {
            classes[i] = CLASS_GROUND;
            result.ground += 1;
        }
        // The heights of the cell feed the statistical cut.
        let e = heights.entry(key).or_insert((0.0, 0.0, 0));
        e.0 += h;
        e.1 += h * h;
        e.2 += 1;
    }

    // 5. The noise: the points far above the others of their cell.
    if options.sigma > 0.0 {
        for (i, p) in points.iter().enumerate() {
            if classes[i] != CLASS_UNCLASSIFIED {
                continue;
            }
            let p = at(p);
            let key = cell_of(p, inv);
            let Some(s) = surface.get(&key) else { continue };
            let Some((sum, sum2, count)) = heights.get(&key) else { continue };
            if *count < 3 {
                continue;
            }
            let n = *count as f64;
            let mean = sum / n;
            let variance = (sum2 / n - mean * mean).max(0.0);
            if p[2] - s > mean + options.sigma * variance.sqrt() {
                classes[i] = CLASS_NOISE;
                result.noise += 1;
            }
        }
    }
    result.unclassified = n - result.ground - result.noise;
    (classes, result)
}

/// The cell a point falls in.
fn cell_of(p: [f64; 3], inv: f64) -> (i64, i64) {
    ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64)
}

/// The 3 × 3 neighbourhood of a cell, the cell itself included.
fn neighbours(cells: &HashMap<(i64, i64), f64>, key: (i64, i64)) -> impl Iterator<Item = ((i64, i64), f64)> + '_ {
    let (x, y) = key;
    (0..3)
        .flat_map(move |dx| (0..3).map(move |dy| ((x + dx - 1, y + dy - 1), cells.get(&(x + dx - 1, y + dy - 1)).copied())))
        .filter_map(|(k, v)| v.map(|v| (k, v)))
}

/// The middle value of a list.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// The plane `z = a + b·x + c·y` through the points, by least squares. `None` when the points do
/// not span a plane (too few, or all in a line).
fn fit_plane(points: &[(f64, f64, f64)]) -> Option<(f64, f64, f64)> {
    let n = points.len() as f64;
    if points.len() < 3 {
        return None;
    }
    let (mut sx, mut sy, mut sz) = (0.0, 0.0, 0.0);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let (mut sxz, mut syz) = (0.0, 0.0);
    for &(x, y, z) in points {
        sx += x;
        sy += y;
        sz += z;
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        sxz += x * z;
        syz += y * z;
    }
    // The normal equations of the least squares fit.
    let m = [[n, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]];
    let r = [sz, sxz, syz];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-9 {
        return None;
    }
    let mut sol = [0.0; 3];
    for i in 0..3 {
        let mut sub = m;
        sub[i] = r;
        let d = sub[0][0] * (sub[1][1] * sub[2][2] - sub[1][2] * sub[2][1]) - sub[0][1] * (sub[1][0] * sub[2][2] - sub[1][2] * sub[2][0])
            + sub[0][2] * (sub[1][0] * sub[2][1] - sub[1][1] * sub[2][0]);
        sol[i] = d / det;
    }
    Some((sol[0], sol[1], sol[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor at 0 m with a table on it: the table's points are not the ground.
    #[test]
    fn a_table_is_not_the_ground() {
        let mut points = Vec::new();
        for i in 0..40 {
            let x = f64::from(i) * 0.1;
            for j in 0..40 {
                let y = f64::from(j) * 0.1;
                points.push([x, y, 0.0]);
            }
        }
        // The table: a slab of points 0.75 m above the floor.
        for i in 0..10 {
            for j in 0..10 {
                points.push([1.0 + f64::from(i) * 0.1, 1.0 + f64::from(j) * 0.1, 0.75]);
            }
        }
        let (classes, result) = classify(&points, &GroundOptions { cell: 0.5, tolerance: 0.15, bump: 0.3, sigma: 0.0 });
        assert_eq!(result.ground, 1600, "the floor is the ground: {result:?}");
        assert_eq!(result.unclassified, 100, "the table is not: {result:?}");
        assert!(classes[..1600].iter().all(|c| *c == CLASS_GROUND));
        assert!(classes[1600..].iter().all(|c| *c == CLASS_UNCLASSIFIED));
    }

    /// A single point far above its neighbours is noise.
    #[test]
    fn a_point_far_above_the_others_is_noise() {
        let mut points = Vec::new();
        for i in 0..100 {
            points.push([f64::from(i) * 0.1, 0.0, 0.0]);
        }
        points.push([0.5, 0.0, 12.0]);
        let (classes, result) = classify(&points, &GroundOptions { cell: 0.5, tolerance: 0.15, bump: 0.3, sigma: 2.0 });
        assert_eq!(result.noise, 1, "the spike is noise: {result:?}");
        assert_eq!(classes[100], CLASS_NOISE);
        assert_eq!(result.ground, 100, "the line is the ground: {result:?}");
    }

    /// A sloped floor stays the ground: the plane fit follows the slope.
    #[test]
    fn a_sloped_floor_is_still_the_ground() {
        let mut points = Vec::new();
        for i in 0..40 {
            let x = f64::from(i) * 0.1;
            for j in 0..40 {
                points.push([x, f64::from(j) * 0.1, x * 0.2]);
            }
        }
        let (_, result) = classify(&points, &GroundOptions { cell: 0.5, tolerance: 0.15, bump: 0.3, sigma: 0.0 });
        assert_eq!(result.ground, 1600, "the whole slope is the ground: {result:?}");
    }

    /// A point under the ground (a spike downwards) is noise, not ground.
    #[test]
    fn a_point_under_the_ground_is_noise() {
        let mut points = Vec::new();
        for i in 0..100 {
            points.push([f64::from(i) * 0.1, 0.0, 0.0]);
        }
        points.push([0.5, 0.0, -3.0]);
        let (classes, result) = classify(&points, &GroundOptions { cell: 0.5, tolerance: 0.15, bump: 0.3, sigma: 0.0 });
        assert_eq!(classes[100], CLASS_NOISE, "it is under the floor: {result:?}");
    }

    #[test]
    fn an_empty_cloud_is_not_a_panic() {
        let (classes, result) = classify(&[], &GroundOptions::default());
        assert!(classes.is_empty());
        assert_eq!(result, GroundResult::default());
    }

    #[test]
    fn a_cell_of_zero_does_nothing() {
        let points = [[0.0, 0.0, 0.0]];
        let (classes, result) = classify(&points, &GroundOptions { cell: 0.0, ..Default::default() });
        assert_eq!(classes, vec![CLASS_UNCLASSIFIED]);
        assert_eq!(result.unclassified, 1);
    }
}
