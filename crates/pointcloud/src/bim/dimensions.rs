//! Automatic interior room dimensioning for RENCAD.
//!
//! Generates aligned dimensions along the inner faces of each room zone,
//! offset inward towards the room interior.

use crate::bim::BimOptions;
use crate::rlv::StoreyModel;
use cadcraft_geom::Vec2;

/// An automatically generated linear/aligned dimension along a room wall.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoDimension {
    /// First measurement point on the wall face.
    pub p1: [f64; 2],
    /// Second measurement point on the wall face.
    pub p2: [f64; 2],
    /// Placement point for the dimension line (offset inward).
    pub at: [f64; 2],
    /// Measured length rounded to two decimal places (metres).
    pub length_m: f64,
    /// Formatted measurement label (e.g. "4.00 m").
    pub label: String,
}

/// The area centroid of a closed ring, in world metres.
fn ring_centroid(ring: &[[f64; 2]]) -> [f64; 2] {
    let n = ring.len();
    let mut twice_area = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else {
            continue;
        };
        let cross = a[0] * b[1] - b[0] * a[1];
        twice_area += cross;
        cx += (a[0] + b[0]) * cross;
        cy += (a[1] + b[1]) * cross;
    }
    if twice_area.abs() > f64::EPSILON {
        let denom = 3.0 * twice_area;
        let c = [cx / denom, cy / denom];
        if c[0].is_finite() && c[1].is_finite() {
            return c;
        }
    }
    if n == 0 {
        return [0.0, 0.0];
    }
    let count = n as f64;
    let sx = ring.iter().filter(|p| p[0].is_finite()).map(|p| p[0]).sum::<f64>();
    let sy = ring.iter().filter(|p| p[1].is_finite()).map(|p| p[1]).sum::<f64>();
    [sx / count, sy / count]
}

/// Generates interior room dimensions for all zones across the given storeys.
pub fn generate_dimensions(storeys: &[StoreyModel], opts: &BimOptions) -> Vec<AutoDimension> {
    let mut dims = Vec::new();
    let min_len = if opts.min_wall_length.is_finite() && opts.min_wall_length > 0.0 { opts.min_wall_length } else { 0.5 };

    for storey in storeys {
        for zone in &storey.zones {
            let n = zone.ring.len();
            if n < 3 {
                continue;
            }
            let c = ring_centroid(&zone.ring);
            let centroid = Vec2::new(c[0], c[1]);

            for i in 0..n {
                let Some(a) = zone.ring.get(i) else { continue };
                let Some(b) = zone.ring.get((i + 1) % n) else { continue };

                if !a[0].is_finite() || !a[1].is_finite() || !b[0].is_finite() || !b[1].is_finite() {
                    continue;
                }

                let dx = b[0] - a[0];
                let dy = b[1] - a[1];
                let len = dx.hypot(dy);

                if !len.is_finite() || len < min_len {
                    continue;
                }

                let d = Vec2::new(dx / len, dy / len);
                let mut n_vec = Vec2::new(-d.y, d.x);

                let mid = (Vec2::new(a[0], a[1]) + Vec2::new(b[0], b[1])) * 0.5;
                let to_centroid = centroid - mid;

                // Verify orientation into the room using centroid
                if n_vec.dot(to_centroid) < 0.0 {
                    n_vec = -n_vec;
                }

                let at = mid + n_vec * 0.25;
                let length_m = (len * 100.0).round() / 100.0;
                let label = format!("{:.2} m", length_m);

                dims.push(AutoDimension { p1: *a, p2: *b, at: [at.x, at.y], length_m, label });
            }
        }
    }

    dims
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bim::{Storey, Zone};

    fn make_storey_model(zones: Vec<Zone>) -> StoreyModel {
        StoreyModel {
            storey: Storey { index: 0, base_z: 0.0, top_z: 2.8 },
            walls: Vec::new(),
            zones,
            rooms: Vec::new(),
            columns: Vec::new(),
            openings: Vec::new(),
            stairs: Vec::new(),
        }
    }

    #[test]
    fn generates_dimensions_for_rectangular_room() {
        let zone = Zone { ring: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 3.0], [0.0, 3.0]], area: 15.0, perimeter: 16.0 };
        let model = make_storey_model(vec![zone]);
        let dims = generate_dimensions(&[model], &BimOptions::default());

        assert_eq!(dims.len(), 4, "all 4 edges of the room are dimensioned");

        // Edge 1: (0,0) -> (5,0), length 5.0m, offset inward (y > 0)
        assert_eq!(dims[0].p1, [0.0, 0.0]);
        assert_eq!(dims[0].p2, [5.0, 0.0]);
        assert!((dims[0].length_m - 5.0).abs() < 1e-6);
        assert_eq!(dims[0].label, "5.00 m");
        assert!((dims[0].at[0] - 2.5).abs() < 1e-6);
        assert!((dims[0].at[1] - 0.25).abs() < 1e-6, "inward normal offset 0.25 m into room");

        // Edge 2: (5,0) -> (5,3), length 3.0m, offset inward (x < 5.0)
        assert_eq!(dims[1].p1, [5.0, 0.0]);
        assert_eq!(dims[1].p2, [5.0, 3.0]);
        assert!((dims[1].length_m - 3.0).abs() < 1e-6);
        assert_eq!(dims[1].label, "3.00 m");
        assert!((dims[1].at[0] - 4.75).abs() < 1e-6, "inward normal offset 0.25 m into room");
        assert!((dims[1].at[1] - 1.5).abs() < 1e-6);

        // Edge 3: (5,3) -> (0,3), length 5.0m, offset inward (y < 3.0)
        assert_eq!(dims[2].p1, [5.0, 3.0]);
        assert_eq!(dims[2].p2, [0.0, 3.0]);
        assert!((dims[2].length_m - 5.0).abs() < 1e-6);
        assert_eq!(dims[2].label, "5.00 m");
        assert!((dims[2].at[0] - 2.5).abs() < 1e-6);
        assert!((dims[2].at[1] - 2.75).abs() < 1e-6, "inward normal offset 0.25 m into room");

        // Edge 4: (0,3) -> (0,0), length 3.0m, offset inward (x > 0.0)
        assert_eq!(dims[3].p1, [0.0, 3.0]);
        assert_eq!(dims[3].p2, [0.0, 0.0]);
        assert!((dims[3].length_m - 3.0).abs() < 1e-6);
        assert_eq!(dims[3].label, "3.00 m");
        assert!((dims[3].at[0] - 0.25).abs() < 1e-6, "inward normal offset 0.25 m into room");
        assert!((dims[3].at[1] - 1.5).abs() < 1e-6);
    }

    #[test]
    fn edges_shorter_than_min_wall_length_are_filtered_out() {
        let zone = Zone {
            ring: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 0.2], [0.0, 0.2]], // 0.2 m short sides
            area: 1.0,
            perimeter: 10.4,
        };
        let opts = BimOptions { min_wall_length: 0.50, ..BimOptions::default() };
        let model = make_storey_model(vec![zone]);
        let dims = generate_dimensions(&[model], &opts);

        assert_eq!(dims.len(), 2, "only the two 5 m edges survive");
        assert_eq!(dims[0].length_m, 5.0);
        assert_eq!(dims[1].length_m, 5.0);
    }

    #[test]
    fn hostile_and_degenerate_geometry_never_panics() {
        let zone1 = Zone { ring: vec![[f64::NAN, 0.0], [5.0, f64::INFINITY], [5.0, 3.0]], area: 0.0, perimeter: 0.0 };
        let zone2 = Zone {
            ring: vec![[0.0, 0.0], [1.0, 1.0]], // only 2 points
            area: 0.0,
            perimeter: 0.0,
        };
        let model = make_storey_model(vec![zone1, zone2]);
        let dims = generate_dimensions(&[model], &BimOptions::default());
        assert!(dims.is_empty(), "bad geometry produces no dimensions safely");
    }
}
