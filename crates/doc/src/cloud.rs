//! A point cloud a drawing refers to.
//!
//! The cloud is not in the drawing: a scan of a building has millions of points, and a file that
//! carries them all is a file nobody can send. What the drawing carries is the *reference*: the
//! file the scan is in, and where the scan sits in the drawing.
//!
//! So the drawing says "the scan `D:/scan/Ap 1.las`, the band from 2.04 m to 2.20 m, on a 5 cm
//! grid, drawn in RGB, with the world point (498 162, 290 290, 0) as the drawing's (0, 0, 0)", and
//! the program loads the scan from the disk when the drawing is opened. The drawing stays small;
//! the scan stays where it was.

/// A point cloud a drawing refers to: the file it is in, and where it sits in the drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointCloud {
    /// The name the cloud is known by.
    pub name: String,
    /// The LAS/LAZ file the cloud is in. Empty means there is nothing to load.
    pub path: String,
    /// The band of Z the cloud holds, in the file's own heights.
    pub z0: f64,
    pub z1: f64,
    /// The grid cell the cloud is read at, in metres. Zero means every point of the band.
    pub cell: f64,
    /// The colour mode the cloud is drawn on ("rgb", "intensity", "elevation", "class",
    /// "deviation", "single:#rrggbb").
    pub color: String,
    /// The colour scale the intensity is read on ("grey", "bluered", "viridis", "turbo", "magma",
    /// "diverging").
    pub spectrum: String,
    /// The coordinate system the file says (the EPSG code, or the WKT).
    pub crs: Option<String>,
    /// Where the cloud sits in the drawing: the world point that is the drawing's (0, 0, 0).
    pub offset: [f64; 3],
}

impl PointCloud {
    /// A reference to the file at `path`, called `name`.
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self { name: name.into(), path: path.into(), color: "rgb".into(), spectrum: "bluered".into(), ..Self::default() }
    }

    /// Whether there is anything to load: a reference without a file is nothing.
    pub fn is_empty(&self) -> bool {
        self.path.trim().is_empty()
    }
}

/// The name of the header variable that carries the clouds' description.
pub const CLOUD_HEADER_VAR: &str = "RENCAD_CLOUDS";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_without_a_file_is_empty() {
        let mut c = PointCloud::new("Ap 1", "D:/scan/Ap 1.las");
        assert!(!c.is_empty());
        c.path = "  ".into();
        assert!(c.is_empty(), "there is nothing to load");
    }

    #[test]
    fn the_defaults_are_the_ones_a_report_reads() {
        let c = PointCloud::new("Ap 1", "D:/scan/Ap 1.las");
        assert_eq!(c.color, "rgb");
        assert_eq!(c.spectrum, "bluered");
        assert_eq!(c.offset, [0.0; 3]);
    }
}
