//! 3D drafting strokes: the polylines drawn over a cloud in the 3D view.
//!
//! The drawing model is 2D, so a 3D line has nowhere to live in it without pushing Z through every
//! entity, snap and grip. The fork keeps strokes beside the clouds instead ([`crate::Cloud`]'s
//! sibling in the session), as plain world-space vertices: the 3D view projects them with the
//! cloud's camera, and [`crate::write_strokes_dxf`] turns them into ordinary DXF LINE entities.

/// The strokes of one session, in the order they were drawn.
#[derive(Clone, Debug, Default)]
pub struct Sketch3d {
    strokes: Vec<Vec<[f64; 3]>>,
}

impl Sketch3d {
    /// Append a stroke; returns the number of vertices it holds. A stroke is a polyline, so one
    /// vertex is a point and two are a line.
    pub fn add(&mut self, points: &[[f64; 3]]) -> usize {
        let pts: Vec<[f64; 3]> = points.iter().copied().filter(|p| p.iter().all(|v| v.is_finite())).collect();
        if !pts.is_empty() {
            self.strokes.push(pts);
        }
        self.strokes.last().map_or(0, Vec::len)
    }

    /// Drop the last stroke and return how many vertices it had (`None` when there is nothing).
    pub fn undo(&mut self) -> Option<usize> {
        self.strokes.pop().map(|s| s.len())
    }

    /// Drop every stroke; returns how many there were.
    pub fn clear(&mut self) -> usize {
        let n = self.strokes.len();
        self.strokes.clear();
        n
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    pub fn strokes(&self) -> &[Vec<[f64; 3]>] {
        &self.strokes
    }

    /// Strokes drawn so far.
    pub fn count(&self) -> usize {
        self.strokes.len()
    }

    /// Vertices placed so far.
    pub fn points(&self) -> usize {
        self.strokes.iter().map(Vec::len).sum()
    }

    /// Line segments the strokes hold (a one-vertex stroke has none).
    pub fn segments(&self) -> usize {
        self.strokes.iter().map(|s| s.len().saturating_sub(1)).sum()
    }

    /// Every segment as `(from, to)`, for the 3D overlay.
    pub fn iter_segments(&self) -> impl Iterator<Item = ([f64; 3], [f64; 3])> + '_ {
        self.strokes.iter().flat_map(|s| s.windows(2).map(|w| (w[0], w[1])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_keep_their_order_and_count() {
        let mut s = Sketch3d::default();
        assert!(s.is_empty());
        assert_eq!(s.add(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]), 3);
        assert_eq!(s.add(&[[0.0, 0.0, 2.0]]), 1);
        assert_eq!((s.count(), s.points(), s.segments()), (2, 4, 2));
        assert_eq!(s.iter_segments().count(), 2);
    }

    #[test]
    fn undo_drops_the_last_stroke_only() {
        let mut s = Sketch3d::default();
        s.add(&[[0.0; 3], [1.0; 3]]);
        s.add(&[[2.0; 3]]);
        assert_eq!(s.undo(), Some(1));
        assert_eq!(s.count(), 1);
        assert_eq!(s.points(), 2);
        assert_eq!(s.undo(), Some(2));
        assert_eq!(s.undo(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn clear_reports_the_strokes_it_dropped() {
        let mut s = Sketch3d::default();
        s.add(&[[0.0; 3]]);
        s.add(&[[1.0; 3]]);
        assert_eq!(s.clear(), 2);
        assert_eq!(s.clear(), 0);
    }

    #[test]
    fn a_stroke_with_no_finite_vertex_is_dropped() {
        let mut s = Sketch3d::default();
        assert_eq!(s.add(&[[f64::NAN, 0.0, 0.0], [0.0, f64::INFINITY, 0.0]]), 0);
        assert!(s.is_empty());
        assert_eq!(s.add(&[[1.0, 2.0, 3.0], [f64::NAN, 0.0, 0.0]]), 1, "the bad vertex goes, the good one stays");
    }
}
