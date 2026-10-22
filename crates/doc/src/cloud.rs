//! A point cloud that travels with the file.
//!
//! The cloud is not a drawing entity: it is millions of points, and the drawing's undo history
//! keeps whole copies of the drawing. So the cloud lives beside the drawing — in the session, and
//! in the file it is attached to — and the file carries it as ordinary `POINT` entities plus a
//! header variable that says what they are.
//!
//! That way the file opens in any program (AutoCAD included: the points are points), and RENCAD
//! recognises them again as the cloud they were.

/// A point cloud kept with a drawing file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointCloud {
    /// The name the cloud is known by.
    pub name: String,
    /// The layer its points are written on.
    pub layer: String,
    /// Where the cloud came from (the LAS/LAZ file).
    pub source: String,
    /// The coordinate system the file says (the EPSG code, or the WKT).
    pub crs: Option<String>,
    /// The colour mode the cloud is drawn on ("rgb", "intensity", "elevation", "class").
    pub color: String,
    /// The points, in the drawing's own units.
    pub points: Vec<[f64; 3]>,
    /// The colour of every point, as the scanner gave it.
    pub colors: Vec<[u8; 3]>,
    /// The intensity of every point.
    pub intensities: Vec<u16>,
    /// The class of every point (the LAS classification).
    pub classes: Vec<u8>,
}

impl PointCloud {
    /// An empty cloud with a name and a layer.
    pub fn new(name: impl Into<String>, layer: impl Into<String>) -> Self {
        Self { name: name.into(), layer: layer.into(), color: "rgb".into(), ..Self::default() }
    }

    /// The number of points.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the cloud is empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The bounds of the points.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = *self.points.first()?;
        let mut min = first;
        let mut max = first;
        for p in &self.points {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        Some((min, max))
    }
}

/// The name of the header variable that carries the clouds' description.
pub const CLOUD_HEADER_VAR: &str = "RENCAD_CLOUDS";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bounds_of_a_cloud_are_read() {
        let mut c = PointCloud::new("scan", "PCLOUD-scan");
        assert!(c.bounds().is_none(), "an empty cloud has no bounds");
        c.points = vec![[0.0, 0.0, 0.0], [2.0, -1.0, 3.0]];
        let (min, max) = c.bounds().unwrap_or_default();
        assert_eq!(min, [0.0, -1.0, 0.0]);
        assert_eq!(max, [2.0, 0.0, 3.0]);
        assert_eq!(c.len(), 2);
    }
}
