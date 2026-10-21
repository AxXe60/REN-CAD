//! Header-level information about a LAS/LAZ file: point count, bounds, attributes.

use std::path::Path;

use crate::{Bounds3, Crs, Error, Result};

/// What a point cloud file says about itself, before reading any point.
#[derive(Clone, Debug)]
pub struct CloudInfo {
    pub points: u64,
    pub bounds: Bounds3,
    /// The file's integer scale and offset (a LAS stores integers, not metres).
    pub scale: [f64; 3],
    pub offset: [f64; 3],
    /// Point format id (0–10), as written by the producer.
    pub point_format: u8,
    pub has_color: bool,
    pub has_intensity: bool,
    /// Where the file says it is (the EPSG code and the WKT), when it says anything.
    pub crs: Option<Crs>,
}

/// Read the header of `path`.
pub fn read_info(path: &Path) -> Result<CloudInfo> {
    let reader = las::Reader::from_path(path).map_err(|e| Error::Las { path: path.display().to_string(), source: e })?;
    let h = reader.header();
    let b = h.bounds();
    let t = h.transforms();
    let format = h.point_format();
    let bounds = Bounds3 { min: [b.min.x, b.min.y, b.min.z], max: [b.max.x, b.max.y, b.max.z] };
    Ok(CloudInfo {
        points: h.number_of_points(),
        bounds,
        scale: [t.x.scale, t.y.scale, t.z.scale],
        offset: [t.x.offset, t.y.offset, t.z.offset],
        point_format: format_id(h),
        has_color: format.has_color,
        has_intensity: true,
        crs: Crs::from_header(h),
    })
}

/// LAS point formats are numbered 0–10; the reader keeps the fields, not the number, so map the
/// layouts back. Formats with colour: 2, 3, 5, 7, 8, 10.
fn format_id(h: &las::Header) -> u8 {
    match (h.point_format().has_color, h.point_format().has_waveform) {
        (true, true) => 10,
        (true, false) => 3,
        (false, true) => 4,
        (false, false) => 0,
    }
}
