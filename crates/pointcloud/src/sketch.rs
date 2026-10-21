//! 3D drafting strokes: the polylines drawn over a cloud in the 3D view.
//!
//! The drawing model is 2D, so a 3D line has nowhere to live in it without pushing Z through every
//! entity, snap and grip. The fork keeps strokes beside the clouds instead ([`crate::Cloud`]'s
//! sibling in the session), as plain world-space vertices: the 3D view projects them with the
//! cloud's camera, and [`crate::write_strokes_dxf`] turns them into ordinary DXF LINE entities.
//!
//! A stroke knows its layer (so an exported drawing can keep walls, floors and annotations apart)
//! and whether it is closed (a room outline, a section profile).

/// The layer a stroke gets when none is given.
pub const DEFAULT_LAYER: &str = "3D_DRAFT";

/// One polyline drawn in the 3D view.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke3 {
    /// The vertices, in the order they were placed.
    pub points: Vec<[f64; 3]>,
    /// The DXF layer the stroke is written on.
    pub layer: String,
    /// Whether the stroke closes back on its first vertex.
    pub closed: bool,
}

impl Stroke3 {
    /// A stroke of `points` on `layer`; an empty layer name means [`DEFAULT_LAYER`].
    pub fn new(points: Vec<[f64; 3]>, layer: &str, closed: bool) -> Self {
        let layer = layer.trim();
        Self { points, layer: if layer.is_empty() { DEFAULT_LAYER.to_string() } else { layer.to_string() }, closed }
    }

    /// The segments the stroke holds: a closed stroke of `n` vertices has `n` of them (the last
    /// one joins the first vertex), an open one has `n - 1`.
    pub fn segment_count(&self) -> usize {
        if self.points.len() < 2 {
            return 0;
        }
        if self.closed { self.points.len() } else { self.points.len() - 1 }
    }
}

/// The strokes of one session, in the order they were drawn.
#[derive(Clone, Debug, Default)]
pub struct Sketch3d {
    strokes: Vec<Stroke3>,
}

impl Sketch3d {
    /// Append a stroke; returns the number of vertices it holds. A stroke is a polyline, so one
    /// vertex is a point and two are a line.
    pub fn add(&mut self, points: &[[f64; 3]], layer: &str, closed: bool) -> usize {
        let pts: Vec<[f64; 3]> = points.iter().copied().filter(|p| p.iter().all(|v| v.is_finite())).collect();
        if !pts.is_empty() {
            self.strokes.push(Stroke3::new(pts, layer, closed));
        }
        self.strokes.last().map_or(0, |s| s.points.len())
    }

    /// Drop the last stroke and return how many vertices it had (`None` when there is nothing).
    pub fn undo(&mut self) -> Option<usize> {
        self.strokes.pop().map(|s| s.points.len())
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

    pub fn strokes(&self) -> &[Stroke3] {
        &self.strokes
    }

    /// Strokes drawn so far.
    pub fn count(&self) -> usize {
        self.strokes.len()
    }

    /// Vertices placed so far.
    pub fn points(&self) -> usize {
        self.strokes.iter().map(|s| s.points.len()).sum()
    }

    /// Line segments the strokes hold (a one-vertex stroke has none, a closed one counts its
    /// closing segment).
    pub fn segments(&self) -> usize {
        self.strokes.iter().map(Stroke3::segment_count).sum()
    }

    /// Every segment as `(from, to)`, for the 3D overlay. A closed stroke also yields the segment
    /// that joins its last vertex back to its first.
    pub fn iter_segments(&self) -> impl Iterator<Item = ([f64; 3], [f64; 3])> + '_ {
        self.strokes.iter().flat_map(|s| {
            let mut segs: Vec<([f64; 3], [f64; 3])> = s.points.windows(2).map(|w| (w[0], w[1])).collect();
            if s.closed && s.points.len() >= 2 {
                let (last, first) = (s.points[s.points.len() - 1], s.points[0]);
                if last != first {
                    segs.push((last, first));
                }
            }
            segs.into_iter()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_keep_their_order_and_count() {
        let mut s = Sketch3d::default();
        assert!(s.is_empty());
        assert_eq!(s.add(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], "", false), 3);
        assert_eq!(s.add(&[[0.0, 0.0, 2.0]], "", false), 1);
        assert_eq!((s.count(), s.points(), s.segments()), (2, 4, 2));
        assert_eq!(s.iter_segments().count(), 2);
    }

    #[test]
    fn a_stroke_keeps_its_layer_and_the_default_one() {
        let mut s = Sketch3d::default();
        s.add(&[[0.0; 3], [1.0; 3]], "WALLS", false);
        s.add(&[[2.0; 3], [3.0; 3]], "  ", false);
        assert_eq!(s.strokes()[0].layer, "WALLS");
        assert_eq!(s.strokes()[1].layer, DEFAULT_LAYER, "an empty layer name means the default");
    }

    /// A closed stroke has one segment more: the one that joins the last vertex to the first.
    #[test]
    fn closing_adds_the_segment_back_to_the_first_vertex() {
        let mut s = Sketch3d::default();
        s.add(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], "", false);
        assert_eq!(s.segments(), 2);
        s.add(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]], "", true);
        assert_eq!(s.segments(), 6, "two open segments, four closed ones");
        assert_eq!(s.iter_segments().count(), 6);
        let last = s.iter_segments().last();
        assert_eq!(last, Some(([0.0, 2.0, 0.0], [0.0, 0.0, 0.0])), "the closing segment goes back to the start");
        assert!(s.strokes()[1].closed);
    }

    /// The Z lock keeps a stroke in one horizontal plane; the stroke records the height it was
    /// drawn at, so the export can check it.
    #[test]
    fn a_locked_stroke_keeps_the_height_of_its_first_vertex() {
        let mut s = Sketch3d::default();
        let first = [1.0, 2.0, 3.0];
        let mut pts = vec![first];
        for p in [[4.0, 5.0, 9.0], [6.0, 7.0, -1.0]] {
            let locked = crate::snap::z_locked(p, pts.first().copied(), true);
            assert_eq!(locked[2], first[2], "the plane holds");
            pts.push(locked);
        }
        s.add(&pts, "", false);
        assert!(s.strokes()[0].points.iter().all(|p| p[2] == first[2]));
    }

    #[test]
    fn undo_drops_the_last_stroke_only() {
        let mut s = Sketch3d::default();
        s.add(&[[0.0; 3], [1.0; 3]], "", false);
        s.add(&[[2.0; 3]], "", false);
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
        s.add(&[[0.0; 3]], "", false);
        s.add(&[[1.0; 3]], "", false);
        assert_eq!(s.clear(), 2);
        assert_eq!(s.clear(), 0);
    }

    #[test]
    fn a_stroke_with_no_finite_vertex_is_dropped() {
        let mut s = Sketch3d::default();
        assert_eq!(s.add(&[[f64::NAN, 0.0, 0.0], [0.0, f64::INFINITY, 0.0]], "", false), 0);
        assert!(s.is_empty());
        assert_eq!(s.add(&[[1.0, 2.0, 3.0], [f64::NAN, 0.0, 0.0]], "", false), 1, "the bad vertex goes, the good one stays");
    }
}
