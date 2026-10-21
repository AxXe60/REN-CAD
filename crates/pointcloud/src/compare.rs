//! Comparing two clouds: how far is every point of one from the other?
//!
//! The measurement a survey office makes after a second visit: the distance from every point of
//! the tested cloud to the nearest point of the reference, and the statistics of those distances
//! (the mean, the standard deviation, the RMS, the extremes and the 95th percentile).
//!
//! Two rules from the practice (Zhang et al. 2015, the CloudCompare C2C): the reference must be
//! **denser** than the tested cloud, and it must cover it — a point outside the reference has no
//! distance, and the report says so instead of inventing one.

use std::collections::HashMap;

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
    let mut out = vec![f32::NAN; tested.len()];
    if tested.is_empty() || reference.is_empty() {
        return out;
    }
    let cell = if cell > 0.0 && cell.is_finite() { cell } else { 1.0 };
    let inv = 1.0 / cell;
    // The reference in a grid: the cell of every point, with the point's index.
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (i, p) in reference.iter().enumerate() {
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64, (p[2] * inv).floor() as i64);
        grid.entry(key).or_default().push(i as u32);
    }
    for (i, p) in tested.iter().enumerate() {
        let cx = (p[0] * inv).floor() as i64;
        let cy = (p[1] * inv).floor() as i64;
        let cz = (p[2] * inv).floor() as i64;
        let mut best = f64::INFINITY;
        // The 3x3x3 neighbourhood: with a cell of a few point spacings, the nearest point is in it.
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = grid.get(&(cx + dx, cy + dy, cz + dz)) else { continue };
                    for &j in bucket {
                        let q = reference[j as usize];
                        let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2);
                        if d < best {
                            best = d;
                        }
                    }
                }
            }
        }
        if best.is_finite() {
            out[i] = best.sqrt() as f32;
        }
    }
    out
}

/// The statistics of a set of distances (the missing ones are counted, not averaged).
pub fn stats(values: &[f32]) -> DeviationStats {
    let mut out = DeviationStats::default();
    let mut sorted: Vec<f64> = Vec::with_capacity(values.len());
    for v in values {
        if v.is_finite() {
            out.count += 1;
            sorted.push(f64::from(*v));
        } else {
            out.missing += 1;
        }
    }
    if out.count == 0 {
        return out;
    }
    sorted.sort_by(f64::total_cmp);
    let n = out.count as f64;
    let sum: f64 = sorted.iter().sum();
    let mean = sum / n;
    let sum2: f64 = sorted.iter().map(|v| v * v).sum();
    let rms = (sum2 / n).sqrt();
    let variance = (sum2 / n - mean * mean).max(0.0);
    out.mean = mean;
    out.rms = rms;
    out.std = variance.sqrt();
    out.min = sorted.first().copied().unwrap_or(0.0);
    out.max = sorted.last().copied().unwrap_or(0.0);
    out.p95 = sorted[((out.count as f64 * 0.95).ceil() as usize).saturating_sub(1).min(out.count - 1)];
    out
}

/// The colour of a deviation, on the scale survey offices read: blue for a small deviation, green
/// in the middle, red for a large one. The point outside the reference stays grey.
pub fn deviation_color(distance: f32, scale: f32) -> [u8; 3] {
    if !distance.is_finite() {
        return [120, 120, 120];
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
}
