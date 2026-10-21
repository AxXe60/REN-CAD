//! 3D snapping rules and drawing constraints for the point cloud view.
//!
//! The 3D view has no depth buffer, so a click under the cursor could otherwise snap to a point on
//! the far wall, seen through a gap in the near one. The rules here fix that without a depth
//! buffer: the candidates under the cursor are ranked by their distance to the camera, and only
//! the front layer — the shallowest ones plus everything within [`FRONT_LAYER_M`] behind them —
//! can be snapped to.
//!
//! The same module holds the drawing constraints: the Z lock (a horizontal plane through the first
//! vertex of the stroke) and the axis lock (a segment that follows the cardinal axis it is closest
//! to). Both are pure functions, so they can be tested without a window.

/// How a click picks its point among the cloud points under the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapMode {
    /// The exact cloud point under the cursor, inside the front layer.
    Exact,
    /// The average of the cloud points around the front point: the scan noise (2-5 mm) averages
    /// out, which is what a wall or a floor wants.
    #[default]
    Smooth,
}

impl SnapMode {
    /// The name used by the command, the palette and the reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Smooth => "smooth",
        }
    }

    /// Read a mode from a name: `exact` or `smooth`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "exact" | "point" => Some(Self::Exact),
            "smooth" | "centroid" | "average" => Some(Self::Smooth),
            _ => None,
        }
    }
}

/// How far behind the front point a candidate may sit and still be snapped to, in metres.
pub const FRONT_LAYER_M: f64 = 0.15;

/// The radius of the sphere the smooth mode averages over, in metres.
pub const SMOOTH_RADIUS_M: f64 = 0.03;

/// A cloud point under the cursor: where it is in the world, how far it is from the camera and
/// where it lands on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    pub point: [f64; 3],
    /// Distance to the camera along the view direction, in metres.
    pub depth: f64,
    pub screen: [f32; 2],
}

/// The candidates in the front layer: the shallowest ones, plus everything within `layer` metres
/// behind them. The rest sit behind a wall and must not be snapped to.
pub fn front_layer(hits: &[SnapHit], layer: f64) -> Vec<SnapHit> {
    let min = hits.iter().map(|h| h.depth).fold(f64::INFINITY, f64::min);
    if !min.is_finite() {
        return Vec::new();
    }
    let limit = min + layer;
    hits.iter().copied().filter(|h| h.depth <= limit).collect()
}

/// The candidate closest to the cursor on screen.
pub fn nearest(hits: &[SnapHit], cursor: [f32; 2]) -> Option<SnapHit> {
    hits.iter().copied().min_by(|a, b| {
        let da = (a.screen[0] - cursor[0]).powi(2) + (a.screen[1] - cursor[1]).powi(2);
        let db = (b.screen[0] - cursor[0]).powi(2) + (b.screen[1] - cursor[1]).powi(2);
        da.total_cmp(&db)
    })
}

/// The average of the points within `radius` of `center`; `center` itself when there are none.
pub fn smooth_centroid<I: IntoIterator<Item = [f64; 3]>>(center: [f64; 3], points: I, radius: f64) -> [f64; 3] {
    let r2 = radius * radius;
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for p in points {
        let d = (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2);
        if d <= r2 {
            sum[0] += p[0];
            sum[1] += p[1];
            sum[2] += p[2];
            count += 1;
        }
    }
    if count == 0 {
        return center;
    }
    let n = count as f64;
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// The Z lock: a point keeps the X and Y of the snap but takes the height of the stroke's first
/// vertex, so the whole stroke stays in one horizontal plane.
pub fn z_locked(p: [f64; 3], first: Option<[f64; 3]>, z_lock: bool) -> [f64; 3] {
    match (z_lock, first) {
        (true, Some(f)) => [p[0], p[1], f[2]],
        _ => p,
    }
}

/// The axis lock: the new point keeps the two coordinates it does not move along, so the segment
/// leaves the last vertex along one cardinal axis.
pub fn axis_locked(p: [f64; 3], last: [f64; 3]) -> [f64; 3] {
    let d = [p[0] - last[0], p[1] - last[1], p[2] - last[2]];
    let mut axis = 0usize;
    let mut best = d[0].abs();
    for (k, v) in d.iter().enumerate().skip(1) {
        if v.abs() > best {
            best = v.abs();
            axis = k;
        }
    }
    let mut out = last;
    out[axis] = p[axis];
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(x: f64, y: f64, depth: f64) -> SnapHit {
        SnapHit { point: [x, y, depth], depth, screen: [x as f32, 0.0] }
    }

    #[test]
    fn the_mode_reads_its_name_and_ignores_case() {
        assert_eq!(SnapMode::parse("exact"), Some(SnapMode::Exact));
        assert_eq!(SnapMode::parse(" Smooth "), Some(SnapMode::Smooth));
        assert_eq!(SnapMode::parse("smooth").map(|m| m.name()), Some("smooth"));
        assert_eq!(SnapMode::parse("nope"), None);
        assert_eq!(SnapMode::default(), SnapMode::Smooth, "smooth is the default: walls want the average");
    }

    /// The point of the front layer: a far wall seen through a gap in the near one must not be
    /// snapped to.
    #[test]
    fn the_front_layer_keeps_the_shallow_candidates_only() {
        let hits = vec![hit(0.0, 0.0, 4.0), hit(1.0, 0.0, 4.1), hit(2.0, 0.0, 4.12), hit(3.0, 0.0, 6.0)];
        let front = front_layer(&hits, FRONT_LAYER_M);
        assert_eq!(front.len(), 3, "the 2 m deeper point is behind a wall: {front:?}");
        assert!(front.iter().all(|h| h.depth <= 4.0 + FRONT_LAYER_M));
    }

    #[test]
    fn an_empty_or_single_candidate_list_behaves() {
        assert!(front_layer(&[], FRONT_LAYER_M).is_empty());
        let one = vec![hit(0.0, 0.0, 2.0)];
        assert_eq!(front_layer(&one, FRONT_LAYER_M).len(), 1);
        assert_eq!(nearest(&[], [0.0, 0.0]), None);
    }

    /// The winner is the point closest to the cursor, not the shallowest one: inside the front
    /// layer they are all at the same wall.
    #[test]
    fn the_nearest_candidate_wins_on_the_cursor() {
        let hits = vec![hit(9.0, 0.0, 4.0), hit(1.0, 0.0, 4.1), hit(5.0, 0.0, 4.2)];
        let winner = nearest(&hits, [0.0, 0.0]);
        assert_eq!(winner.map(|h| h.point), Some([1.0, 0.0, 4.1]));
    }

    /// The smooth mode averages the scan noise out: the points of a wall sit within a few
    /// millimetres of the true surface, and their average is on it.
    #[test]
    fn the_centroid_of_the_neighbourhood_cancels_the_noise() {
        let noisy = vec![[1.0, 0.0, 2.002], [1.001, 0.001, 1.998], [0.999, -0.001, 2.001]];
        let c = smooth_centroid([1.0, 0.0, 2.0], noisy, SMOOTH_RADIUS_M);
        assert!((c[0] - 1.0).abs() < 1e-3 && (c[2] - 2.0).abs() < 1e-3, "the average is on the wall: {c:?}");
    }

    #[test]
    fn the_centroid_ignores_the_points_outside_the_sphere() {
        let points = vec![[1.0, 0.0, 2.0], [1.05, 0.0, 2.0]];
        let c = smooth_centroid([1.0, 0.0, 2.0], points.clone(), SMOOTH_RADIUS_M);
        assert_eq!(c, [1.0, 0.0, 2.0], "5 cm away is outside the 3 cm sphere");
        let c = smooth_centroid([1.0, 0.0, 2.0], points, 0.1);
        assert!((c[0] - 1.025).abs() < 1e-9, "with a 10 cm sphere both count: {c:?}");
    }

    #[test]
    fn the_centroid_of_nothing_is_the_centre() {
        assert_eq!(smooth_centroid([1.0, 2.0, 3.0], Vec::<[f64; 3]>::new(), SMOOTH_RADIUS_M), [1.0, 2.0, 3.0]);
    }

    /// The Z lock: the height of the first vertex wins, the plan position comes from the snap.
    #[test]
    fn the_z_lock_holds_the_plane_of_the_first_vertex() {
        let first = [1.0, 2.0, 3.0];
        assert_eq!(z_locked([5.0, 6.0, 9.0], Some(first), true), [5.0, 6.0, 3.0]);
        assert_eq!(z_locked([5.0, 6.0, 9.0], Some(first), false), [5.0, 6.0, 9.0], "off: the point is untouched");
        assert_eq!(z_locked([5.0, 6.0, 9.0], None, true), [5.0, 6.0, 9.0], "no first vertex yet: nothing to lock");
    }

    /// The axis lock: the segment follows the cardinal axis it is closest to.
    #[test]
    fn the_axis_lock_follows_the_dominant_direction() {
        let last = [0.0, 0.0, 0.0];
        assert_eq!(axis_locked([3.0, 0.2, 0.1], last), [3.0, 0.0, 0.0], "mostly X");
        assert_eq!(axis_locked([0.1, -2.0, 0.2], last), [0.0, -2.0, 0.0], "mostly Y");
        assert_eq!(axis_locked([0.1, 0.2, -4.0], last), [0.0, 0.0, -4.0], "mostly Z");
    }
}
