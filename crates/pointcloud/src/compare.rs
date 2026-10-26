//! Comparing two clouds: how far is every point of one from the other?
//!
//! The measurement a survey office makes after a second visit: the distance from every point of
//! the tested cloud to the nearest point of the reference, and the statistics of those distances
//! (the mean, the standard deviation, the RMS, the extremes and the 95th percentile).
//!
//! Two rules from the practice (Zhang et al. 2015, the CloudCompare C2C): the reference must be
//! **denser** than the tested cloud, and it must cover it — a point outside the reference has no
//! distance, and the report says so instead of inventing one.

use std::cmp::Ordering;

use crate::grid::Grid;

/// The statistics of a set of distances, in metres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeviationStats {
    /// The points that found a neighbour.
    pub count: usize,
    /// The points that found none (outside the reference).
    pub missing: usize,
    pub mean: f64,
    pub std: f64,
    /// The root mean square: the summary number a survey report quotes.
    pub rms: f64,
    pub min: f64,
    pub max: f64,
    /// The 95th percentile: the number under which 95% of the points fall.
    pub p95: f64,
}

impl DeviationStats {
    /// The report line.
    pub fn summary(&self) -> String {
        format!(
            "{} points: mean {:.4} m, σ {:.4} m, RMS {:.4} m, min {:.4}, max {:.4}, p95 {:.4} m{}",
            self.count,
            self.mean,
            self.std,
            self.rms,
            self.min,
            self.max,
            self.p95,
            if self.missing > 0 { format!(" ({} outside the reference)", self.missing) } else { String::new() }
        )
    }
}

/// The distance from every point of `tested` to the nearest point of `reference`.
///
/// `cell` is the size of the search grid: it should be a few times the spacing of the reference's
/// points, so the nearest neighbour is in the neighbourhood that is looked at. A distance of
/// `f32::NAN` means the point found no neighbour: it lies outside the reference.
pub fn distances(tested: &[[f64; 3]], reference: &[[f64; 3]], cell: f64) -> Vec<f32> {
    if tested.is_empty() || reference.is_empty() {
        return vec![f32::NAN; tested.len()];
    }
    let cell = if cell > 0.0 && cell.is_finite() { cell } else { 1.0 };
    // The reference in a grid: every cell owns a run of one flat array, so a million cells cost
    // one allocation and not a million small ones. The built grid is handed to the shared path.
    let grid = Grid::new(reference, cell);
    distances_with_grid(tested, reference, &grid)
}

/// The same search against a grid that is built already: the caller builds it once over the
/// reference and reuses it for every level of the tested cloud, instead of building it again per
/// level. `grid` must be the grid of `reference`, the same points and the same cell.
///
/// `cell` and the NaN rule are those of `distances`: the distances come out the same, to the last
/// bit, because both paths search the same grid.
pub fn distances_with_grid(tested: &[[f64; 3]], reference: &[[f64; 3]], grid: &Grid) -> Vec<f32> {
    if tested.is_empty() || reference.is_empty() {
        return vec![f32::NAN; tested.len()];
    }
    nearest_of(tested, reference, grid)
}

/// The search loop the two entry points share: for every tested point the nearest point of the
/// reference in the neighbourhood of its cell.
fn nearest_of(tested: &[[f64; 3]], reference: &[[f64; 3]], grid: &Grid) -> Vec<f32> {
    let mut out = vec![f32::NAN; tested.len()];
    let mut near = Vec::new();
    for (i, p) in tested.iter().enumerate() {
        // The 3x3x3 neighbourhood: with a cell of a few point spacings, the nearest point is in it.
        grid.around(*p, &mut near);
        let mut best = f64::INFINITY;
        for &j in near.iter() {
            let q = &reference[j as usize];
            let d0 = q[0] - p[0];
            let d1 = q[1] - p[1];
            let d2 = q[2] - p[2];
            let d = d0 * d0 + d1 * d1 + d2 * d2;
            if d < best {
                best = d;
            }
        }
        if best.is_finite() {
            out[i] = best.sqrt() as f32;
        }
    }
    out
}

/// The statistics of a set of distances (the missing ones are counted, not averaged).
///
/// One pass gathers the mean, the standard deviation and the RMS, and the same pass counts the
/// values into coarse bands of their widening order, from which the minimum, the maximum and the
/// 95th percentile are picked. The distances are never copied into a second vector and sorted,
/// which for four million points used to cost tens of megabytes and a full sort per report.
pub fn stats(values: &[f32]) -> DeviationStats {
    let mut out = DeviationStats::default();
    let mut sum = 0.0f64;
    let mut sum2 = 0.0f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut top = [0usize; 1 << HISTOGRAM_BITS];
    for v in values {
        if v.is_finite() {
            out.count += 1;
            let x = f64::from(*v);
            sum += x;
            sum2 += x * x;
            if x.total_cmp(&min) == Ordering::Less {
                min = x;
            }
            if x.total_cmp(&max) == Ordering::Greater {
                max = x;
            }
            // The coarse band the value falls into: bands of bits are ranges of values, so the
            // percentile can later be found by counting instead of sorting.
            top[(sort_key(*v) >> (32 - HISTOGRAM_BITS)) as usize] += 1;
        } else {
            out.missing += 1;
        }
    }
    if out.count == 0 {
        return out;
    }
    let n = out.count as f64;
    let mean = sum / n;
    out.mean = mean;
    out.rms = (sum2 / n).sqrt();
    out.std = (sum2 / n - mean * mean).max(0.0).sqrt();
    out.min = min;
    out.max = max;
    out.p95 = percentile(values, out.count, &top);
    out
}

/// The bands of the widening order that the percentile walks down: 11 bits are counted with the
/// main pass, 11 and 10 more in two passes over the values inside the band the rank fell into.
const HISTOGRAM_BITS: u32 = 11;

/// The widening order of `f32::total_cmp` as one unsigned key: ascending, NaN excluded, so a
/// range of keys is a range of values and rank of key is rank of value.
fn sort_key(v: f32) -> u32 {
    let bits = v.to_bits() as i32;
    // `total_cmp` turns the sign-magnitude bits into two's complement order by flipping the low
    // 31 bits of a negative; the sign bit flipped on top turns that into an unsigned order.
    let ordered = bits ^ ((((bits >> 31) as u32) >> 1) as i32);
    (ordered as u32) ^ 0x8000_0000
}

/// The value of one widening key: the inverse of `sort_key`, used once the band walk has pinned
/// all 32 bits, which means one bit pattern, one exact value.
fn key_point(key: u32) -> f32 {
    let ordered = (key ^ 0x8000_0000) as i32;
    f32::from_bits((ordered ^ ((((ordered >> 31) as u32) >> 1) as i32)) as u32)
}

/// The value at the rank the survey quotes, found by counting instead of sorting.
///
/// The rank is walked down the widening key in bands: 11 bits were counted with the main pass in
/// `top`, then 11 and 10 more in two passes over `values`, each narrowing to the band the rank
/// fell into. Three bands cover all 32 bits, and the last one is one bit pattern, one exact
/// value: nothing is copied, nothing is sorted.
fn percentile(values: &[f32], count: usize, top: &[usize; 1 << HISTOGRAM_BITS]) -> f64 {
    // The same index the sorted array used to answer with, from the smallest value up.
    let mut rank = ((count as f64 * 0.95).ceil() as usize).saturating_sub(1).min(count - 1);
    let mut counts = [0usize; 1 << HISTOGRAM_BITS];
    let mut known = 0u32;
    let mut prefix = 0u32;
    for band_bits in [HISTOGRAM_BITS, HISTOGRAM_BITS, 32 - 2 * HISTOGRAM_BITS] {
        if known == 0 {
            counts.copy_from_slice(top);
        } else {
            counts.fill(0);
            let below = 32 - known - band_bits;
            let band_mask = (1u32 << band_bits) - 1;
            for v in values {
                if v.is_finite() {
                    let k = sort_key(*v);
                    if (k >> (32 - known)) == prefix {
                        counts[((k >> below) & band_mask) as usize] += 1;
                    }
                }
            }
        }
        // The band the rank falls into: everything under it stays behind, everything above is
        // dropped.
        let mut under = 0usize;
        let mut band = counts.len() - 1;
        for (b, &c) in counts.iter().enumerate() {
            if under + c > rank {
                band = b;
                break;
            }
            under += c;
        }
        rank = rank.saturating_sub(under);
        prefix = (prefix << band_bits) | band as u32;
        known += band_bits;
    }
    f64::from(key_point(prefix))
}

/// The grey of a point the reference has nothing for: a cloud that was never compared, and a point
/// that found no neighbour in the one it was compared with. The legend names it, so the key holds
/// every colour the drawing can produce.
pub const DEVIATION_NO_DATA: [u8; 3] = [120, 120, 120];

/// The colour of a deviation, on the scale survey offices read: blue for a small deviation, green
/// in the middle, red for a large one. The point outside the reference stays grey.
pub fn deviation_color(distance: f32, scale: f32) -> [u8; 3] {
    if !distance.is_finite() {
        return DEVIATION_NO_DATA;
    }
    let scale = if scale > 0.0 && scale.is_finite() { scale } else { 1.0 };
    let t = (f64::from(distance) / f64::from(scale)).clamp(0.0, 1.0) as f32;
    // Blue → cyan → green → yellow → red.
    const STOPS: [[f32; 3]; 5] = [[0.15, 0.30, 0.85], [0.15, 0.70, 0.85], [0.20, 0.80, 0.35], [0.95, 0.85, 0.20], [0.90, 0.20, 0.15]];
    let x = t * (STOPS.len() - 1) as f32;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    let f = x - i as f32;
    let mix = |k: usize| ((STOPS[i][k] * (1.0 - f) + STOPS[i + 1][k] * f) * 255.0).round().clamp(0.0, 255.0) as u8;
    [mix(0), mix(1), mix(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_finds_its_neighbour() {
        let reference = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let tested = vec![[0.01, 0.0, 0.0], [0.5, 0.5, 0.0]];
        let d = distances(&tested, &reference, 1.0);
        assert!((d[0] - 0.01).abs() < 1e-6, "{d:?}");
        assert!((d[1] - 0.5_f32.sqrt()).abs() < 1e-6, "{d:?}");
    }

    /// A point outside the reference has no distance: the report says so instead of inventing one.
    #[test]
    fn a_point_outside_the_reference_has_no_distance() {
        let reference = vec![[0.0, 0.0, 0.0]];
        let tested = vec![[100.0, 0.0, 0.0]];
        let d = distances(&tested, &reference, 1.0);
        assert!(d[0].is_nan(), "{d:?}");
        let s = stats(&d);
        assert_eq!(s.count, 0);
        assert_eq!(s.missing, 1);
    }

    #[test]
    fn the_statistics_speak_the_survey_language() {
        let values = [0.01f32, 0.02, 0.03];
        let s = stats(&values);
        assert_eq!(s.count, 3);
        assert!((s.mean - 0.02).abs() < 1e-9, "{s:?}");
        assert!((s.rms - ((0.0001f64 + 0.0004 + 0.0009) / 3.0).sqrt()).abs() < 1e-9, "{s:?}");
        assert!((s.min - 0.01).abs() < 1e-9 && (s.max - 0.03).abs() < 1e-9, "{s:?}");
        assert!((s.p95 - 0.03).abs() < 1e-9, "{s:?}");
        assert!(s.summary().contains("RMS"), "{}", s.summary());
    }

    #[test]
    fn the_colour_scale_runs_from_blue_to_red() {
        let low = deviation_color(0.0, 1.0);
        let high = deviation_color(1.0, 1.0);
        assert!(low[2] > low[0], "small deviation is blue: {low:?}");
        assert!(high[0] > high[2], "large deviation is red: {high:?}");
        assert_eq!(deviation_color(f32::NAN, 1.0), [120, 120, 120], "outside the reference is grey");
    }

    /// The grid is a flat array of buckets: the run of every cell must hold exactly its own
    /// points. A layout that slips by one point would show it here.
    #[test]
    fn the_grid_finds_the_neighbour_of_every_point() {
        let mut reference = Vec::new();
        for i in 0..200 {
            for j in 0..200 {
                reference.push([i as f64 * 0.02, j as f64 * 0.02, 0.0]);
            }
        }
        // The tested cloud is the reference, moved a little: the deviation must be that move.
        let shift = [0.003, -0.004, 0.0];
        let tested: Vec<[f64; 3]> = reference.iter().map(|p| [p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]]).collect();
        let d = distances(&tested, &reference, 0.08);
        let s = stats(&d);
        assert_eq!(s.count, reference.len());
        assert_eq!(s.missing, 0);
        let expected = (shift[0] * shift[0] + shift[1] * shift[1]).sqrt();
        assert!((s.mean - expected).abs() < 1e-4, "{s:?}");
        assert!(s.max < expected * 1.5, "{s:?}");
    }

    /// The shared grid answers with the same numbers as the one-shot search: the caller builds
    /// the grid once over the reference and every level reads from it.
    #[test]
    fn the_shared_grid_answers_like_the_one_shot_one() {
        let mut reference = Vec::new();
        for i in 0..40 {
            for j in 0..40 {
                reference.push([i as f64 * 0.05, j as f64 * 0.05, (i % 7) as f64 * 0.1]);
            }
        }
        let tested: Vec<[f64; 3]> = reference.iter().map(|p| [p[0] + 0.013, p[1] - 0.007, p[2] + 0.02]).collect();
        let one_shot = distances(&tested, &reference, 0.1);
        let grid = Grid::new(&reference, 0.1);
        let shared = distances_with_grid(&tested, &reference, &grid);
        assert_eq!(one_shot, shared, "the two paths agree to the last bit");
        let s = stats(&shared);
        assert_eq!(s.missing, 0, "every point is inside the reference");
        // Out of the reference the answer stays no distance, both ways.
        let outside = [[100.0, 100.0, 100.0]];
        let one = distances(&outside, &reference, 0.1);
        let with = distances_with_grid(&outside, &reference, &grid);
        assert!(one[0].is_nan() && with[0].is_nan(), "{one:?} against {with:?}");
    }
}
