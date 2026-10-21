//! Measuring a segment in 3D: the length, the horizontal run, the rise and the slope.
//!
//! The numbers are the same for the `3DMEASURE` command, for the live readout on the rubber band
//! and for the palette, so they are computed once, here.

/// What a segment between two points measures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// The length of the segment in space.
    pub length: f64,
    /// Its length in plan (the XY distance), the one a tape on the floor would read.
    pub flat: f64,
    /// How much it rises (positive) or falls (negative).
    pub rise: f64,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    /// The angle against the horizontal plane, in degrees.
    pub slope_deg: f64,
    /// The slope in percent, or `None` for a vertical segment (no horizontal run to compare).
    pub slope_pct: Option<f64>,
}

/// Measure the segment from `a` to `b`.
pub fn measure(a: [f64; 3], b: [f64; 3]) -> Measure {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let slope_deg = d[2].atan2(flat).to_degrees();
    let slope_pct = if flat > f64::EPSILON { Some(100.0 * d[2] / flat) } else { None };
    Measure { length, flat, rise: d[2], dx: d[0], dy: d[1], dz: d[2], slope_deg, slope_pct }
}

/// The measurement as a readable line: `3.214 m · dXY 2.870 · dZ 1.450`.
pub fn readout(m: &Measure) -> String {
    format!("{:.3} m · dXY {:.3} · dZ {:.3}", m.length, m.flat, m.rise)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagonal_measures_its_length_and_its_run() {
        let m = measure([0.0, 0.0, 0.0], [3.0, 4.0, 12.0]);
        assert!((m.length - 13.0).abs() < 1e-9, "{}", m.length);
        assert!((m.flat - 5.0).abs() < 1e-9, "{}", m.flat);
        assert!((m.rise - 12.0).abs() < 1e-9);
        assert!((m.dx - 3.0).abs() < 1e-9 && (m.dy - 4.0).abs() < 1e-9 && (m.dz - 12.0).abs() < 1e-9);
        assert!((m.slope_deg - 67.3801).abs() < 0.001, "{}", m.slope_deg);
        assert!((m.slope_pct.unwrap_or(0.0) - 240.0).abs() < 1e-9);
    }

    #[test]
    fn a_flat_segment_has_no_slope() {
        let m = measure([0.0, 0.0, 1.0], [2.0, 0.0, 1.0]);
        assert!((m.length - 2.0).abs() < 1e-9);
        assert!(m.slope_deg.abs() < 1e-9);
        assert_eq!(m.slope_pct, Some(0.0));
    }

    #[test]
    fn a_vertical_segment_has_no_percentage() {
        let m = measure([0.0, 0.0, 0.0], [0.0, 0.0, 2.5]);
        assert!((m.length - 2.5).abs() < 1e-9);
        assert!((m.slope_deg - 90.0).abs() < 1e-9, "straight up is 90°");
        assert_eq!(m.slope_pct, None, "no horizontal run to compare");
    }

    #[test]
    fn the_readout_shows_the_three_numbers() {
        let text = readout(&measure([0.0, 0.0, 0.0], [1.0, 0.0, 1.0]));
        assert!(text.contains("1.414 m"), "{text}");
        assert!(text.contains("dXY 1.000"), "{text}");
        assert!(text.contains("dZ 1.000"), "{text}");
    }
}
