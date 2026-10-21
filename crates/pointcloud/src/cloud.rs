//! A band kept for the canvas: decimation levels and colour modes.
//!
//! [`crate::slice`] streams the file once and returns every point in the band. For display that
//! is too many and too fat: a 12-million-point scan at 32 bytes is 384 MB and no GPU wants a
//! 12-million-quad instance buffer. So the band is turned into a pyramid of grid-decimated levels
//! (one point per cell, cells doubling), stored as 16-byte vertices relative to the cloud's
//! origin, and the canvas picks the level whose spacing is about two screen pixels. Zoomed out it
//! draws thousands; zoomed in, more — never more than the screen can show.

use std::collections::HashSet;
use std::path::Path;

use crate::{Bounds3, Result, Slice};

/// How the points are coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudColor {
    /// The file's RGB.
    Rgb,
    /// A ramp from the band's lowest point to its highest.
    Elevation,
    /// Greyscale from the return intensity.
    Intensity,
    /// One colour for every point.
    Single([u8; 3]),
}

impl CloudColor {
    /// Parse the command spelling: `rgb`, `elevation`, `intensity`, `single:#rrggbb`.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_ascii_lowercase();
        match t.as_str() {
            "rgb" | "truecolor" | "true-colour" => Some(Self::Rgb),
            "elevation" | "z" | "height" => Some(Self::Elevation),
            "intensity" | "grey" | "gray" => Some(Self::Intensity),
            _ => {
                let hex = t.strip_prefix("single:").or_else(|| t.strip_prefix('#'))?;
                parse_hex(hex).map(Self::Single)
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Rgb => "rgb",
            Self::Elevation => "elevation",
            Self::Intensity => "intensity",
            Self::Single(_) => "single",
        }
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// One display vertex: 16 bytes, relative to [`Cloud::origin`].
#[derive(Clone, Copy, Debug)]
pub struct CloudVtx {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rgb: [u8; 3],
    pub intensity: u8,
}

/// One decimation level: points on a grid of `cell` metres (`0` = the source points).
#[derive(Clone, Debug)]
pub struct CloudLevel {
    pub cell: f64,
    pub pts: Vec<CloudVtx>,
}

/// A band of points kept in the session and drawn in the canvas (the `LASOPEN` overlay).
#[derive(Clone, Debug)]
pub struct Cloud {
    pub name: String,
    pub path: String,
    /// The band, kept so `LASDECIMATE` and `LASCOLOR` can rebuild without the caller repeating it.
    pub z0: f64,
    pub z1: f64,
    pub decimate: Option<f64>,
    /// World XY subtracted from every vertex.
    pub origin: [f64; 2],
    pub zmin: f64,
    pub zmax: f64,
    /// The band's intensity range, so the greyscale uses what the scanner recorded instead of the
    /// nominal 0–65535 (a typical indoor slice sits in a narrow dark band and would look black).
    pub imin: u8,
    pub imax: u8,
    pub bounds: Bounds3,
    pub visible: bool,
    pub color: CloudColor,
    /// Finest first.
    pub levels: Vec<CloudLevel>,
    /// Points the band held before the `maxPoints` cap.
    pub full_points: usize,
}

/// Points per cloud by default: a 6-million-point band is 96 MB of 16-byte vertices and still
/// ten times more than a 4K screen can show at any zoom.
pub const DEFAULT_MAX_POINTS: usize = 6_000_000;

impl Cloud {
    /// Build the levels of an already-sliced band.
    pub fn from_slice(
        name: String,
        path: String,
        z0: f64,
        z1: f64,
        decimate: Option<f64>,
        slice: Slice,
        max_points: usize,
        color: CloudColor,
    ) -> Self {
        let b = slice.bounds;
        let origin = [b.min[0], b.min[1]];
        let (zmin, zmax) = (b.min[2], b.max[2]);
        let (imin, imax) = slice.points.iter().fold((u8::MAX, 0u8), |(lo, hi), p| {
            let v = (p.intensity / 257).min(255) as u8;
            (lo.min(v), hi.max(v))
        });
        let (imin, imax) = if slice.points.is_empty() { (0, 255) } else { (imin, imax) };
        let mut cell = decimate.filter(|c| *c > 0.0).unwrap_or(0.002).max(1e-4);
        let cap = max_points.max(10_000);
        let mut base = grid(&slice.points, cell, origin);
        // Fit the cap, then keep halving the density: 1.6x cells make the next level ~2.5x lighter.
        while base.len() > cap && cell < 1e4 {
            cell *= 1.5;
            base = grid(&slice.points, cell, origin);
        }
        let mut levels = vec![CloudLevel { cell, pts: base }];
        while levels.last().is_some_and(|l| l.pts.len() > 20_000) && levels.len() < 16 {
            let c = levels.last().map_or(0.01, |l| l.cell) * 1.6;
            let pts = grid(&slice.points, c, origin);
            if pts.len() >= levels.last().map_or(0, |l| l.pts.len()) {
                break;
            }
            levels.push(CloudLevel { cell: c, pts });
        }
        Self {
            name,
            path,
            z0,
            z1,
            decimate,
            origin,
            zmin,
            zmax,
            imin,
            imax,
            bounds: b,
            visible: true,
            color,
            levels,
            full_points: slice.in_band as usize,
        }
    }

    /// Read `path`, cut the band and build the levels.
    pub fn open(path: &Path, name: String, z0: f64, z1: f64, decimate: Option<f64>, max_points: usize, color: CloudColor) -> Result<Self> {
        let slice = crate::slice(path, z0, z1, decimate)?;
        Ok(Self::from_slice(name, path.display().to_string(), z0, z1, decimate, slice, max_points, color))
    }

    pub fn points(&self) -> usize {
        self.levels.first().map_or(0, |l| l.pts.len())
    }

    /// The level to draw at `px_per_m` screen pixels per metre: the one whose grid spacing is
    /// nearest two pixels.
    pub fn level_for(&self, px_per_m: f64) -> &CloudLevel {
        let want = if px_per_m.is_finite() && px_per_m > 0.0 { 2.0 / px_per_m } else { f64::INFINITY };
        let mut best = &self.levels[0];
        let mut best_d = f64::INFINITY;
        for l in &self.levels {
            let d = if l.cell > 0.0 && want > 0.0 { (l.cell / want).ln().abs() } else { 0.0 };
            if d < best_d {
                best_d = d;
                best = l;
            }
        }
        best
    }

    /// The colour of one vertex under the cloud's colour mode.
    pub fn color_of(&self, v: &CloudVtx) -> [u8; 3] {
        match self.color {
            CloudColor::Rgb => v.rgb,
            CloudColor::Intensity => {
                // Normalised to the band, and never darker than a mid grey: indoor returns are
                // dim, and black points on a dark canvas are invisible.
                let span = u32::from(self.imax.saturating_sub(self.imin));
                let t = if span > 0 { f32::from(v.intensity.saturating_sub(self.imin)) / span as f32 } else { 0.6 };
                let g = (40.0 + 215.0 * t.clamp(0.0, 1.0)).round() as u8;
                [g, g, g]
            }
            CloudColor::Single(c) => c,
            CloudColor::Elevation => {
                let span = self.zmax - self.zmin;
                let t = if span > 0.0 { (f64::from(v.z) - self.zmin) / span } else { 0.5 };
                ramp(t.clamp(0.0, 1.0) as f32)
            }
        }
    }
}

/// One point per `cell`-metre XY square, keeping the first in file order.
fn grid(points: &[crate::Pt], cell: f64, origin: [f64; 2]) -> Vec<CloudVtx> {
    let inv = 1.0 / cell;
    let mut seen: HashSet<(i64, i64)> = HashSet::with_capacity(points.len().min(4_000_000));
    let mut out = Vec::with_capacity(points.len().min(4_000_000));
    for p in points {
        let key = (((p.x - origin[0]) * inv).floor() as i64, ((p.y - origin[1]) * inv).floor() as i64);
        if !seen.insert(key) {
            continue;
        }
        out.push(CloudVtx {
            x: (p.x - origin[0]) as f32,
            y: (p.y - origin[1]) as f32,
            z: p.z as f32,
            rgb: p.rgb.unwrap_or([220, 220, 220]),
            intensity: (p.intensity / 257).min(255) as u8,
        });
    }
    out
}

/// Blue → cyan → green → yellow → red, the drafting-friendly stand-in for a turbo ramp.
fn ramp(t: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 5] = [[0.16, 0.20, 0.55], [0.15, 0.60, 0.85], [0.20, 0.80, 0.45], [0.95, 0.85, 0.25], [0.90, 0.20, 0.15]];
    let x = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    let f = x - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    let mix = |k: usize| ((a[k] * (1.0 - f) + b[k] * f) * 255.0).round().clamp(0.0, 255.0) as u8;
    [mix(0), mix(1), mix(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64, z: f64) -> crate::Pt {
        crate::Pt { x, y, z, rgb: Some([10, 20, 30]), intensity: 1000 }
    }

    fn slice_of(points: Vec<crate::Pt>) -> Slice {
        let mut b = Bounds3::EMPTY;
        for p in &points {
            b.add(p.x, p.y, p.z);
        }
        let n = points.len() as u64;
        Slice { points, total: n, in_band: n, bounds: b }
    }

    #[test]
    fn levels_get_lighter_and_cells_wider() {
        // A 3 x 2 m floor on a 1 cm grid: 60k points, so the pyramid has several steps.
        let pts: Vec<_> = (0..300).flat_map(|i| (0..200).map(move |j| pt(f64::from(i) * 0.01, f64::from(j) * 0.01, 1.0))).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.01), slice_of(pts), 100_000, CloudColor::Rgb);
        assert!(c.levels.len() >= 3, "a pyramid for zooming out: {}", c.levels.len());
        for w in c.levels.windows(2) {
            assert!(w[1].cell > w[0].cell);
            assert!(w[1].pts.len() <= w[0].pts.len());
        }
        assert!(c.points() <= 100_000, "the max-points cap is honoured");
        assert!(c.levels.last().is_some_and(|l| l.pts.len() <= 20_000), "the coarsest level fits a screen");
    }

    #[test]
    fn level_for_picks_finer_as_you_zoom_in() {
        let pts: Vec<_> = (0..2000).map(|i| pt(f64::from(i % 50) * 0.01, f64::from(i / 50) * 0.01, 1.0)).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.01), slice_of(pts), 100_000, CloudColor::Rgb);
        let far = c.level_for(2.0).cell;
        let near = c.level_for(200.0).cell;
        assert!(near <= far);
    }

    #[test]
    fn colour_modes() {
        let c = Cloud::from_slice(
            "t".into(),
            String::new(),
            1.0,
            3.0,
            Some(0.01),
            slice_of(vec![pt(0.0, 0.0, 1.0), pt(1.0, 1.0, 3.0)]),
            100_000,
            CloudColor::Elevation,
        );
        let lo = c.color_of(&c.levels[0].pts[0]);
        let hi = c.color_of(&c.levels[0].pts[1]);
        assert_ne!(lo, hi, "the ramp spans the band");
        let mut s = c.clone();
        s.color = CloudColor::Single([1, 2, 3]);
        assert_eq!(s.color_of(&s.levels[0].pts[0]), [1, 2, 3]);
        assert_eq!(CloudColor::parse("#00ff00"), Some(CloudColor::Single([0, 255, 0])));
        assert_eq!(CloudColor::parse("rgb"), Some(CloudColor::Rgb));
    }

    #[test]
    fn intensity_spans_the_band_not_the_sensor_range() {
        let mk = |x: f64, i: u16| crate::Pt { x, y: 0.0, z: 1.0, rgb: None, intensity: i };
        let c = Cloud::from_slice(
            "t".into(),
            String::new(),
            0.0,
            2.0,
            Some(0.01),
            slice_of(vec![mk(0.0, 100), mk(1.0, 900)]),
            1000,
            CloudColor::Intensity,
        );
        let lo = c.color_of(&c.levels[0].pts[0])[0];
        let hi = c.color_of(&c.levels[0].pts[1])[0];
        assert!(lo >= 40, "dim returns stay visible: {lo}");
        assert!(hi > 200, "the band's brightest is near white: {hi}");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(Cloud::open(Path::new("no-such-file.las"), "x".into(), 0.0, 1.0, None, 1000, CloudColor::Rgb).is_err());
    }
}
