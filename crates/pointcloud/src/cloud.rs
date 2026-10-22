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
    /// The distance to a reference cloud, on a blue-to-red scale.
    Deviation,
    /// The classification of every point: the ground brown, the vegetation green, the walls red.
    Class,
}

impl CloudColor {
    /// Parse the command spelling: `rgb`, `elevation`, `intensity`, `single:#rrggbb`.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_ascii_lowercase();
        match t.as_str() {
            "rgb" | "truecolor" | "true-colour" => Some(Self::Rgb),
            "elevation" | "z" | "height" => Some(Self::Elevation),
            "intensity" | "grey" | "gray" => Some(Self::Intensity),
            "single" | "flat" => Some(Self::Single([200, 200, 200])),
            "deviation" | "dev" | "compare" => Some(Self::Deviation),
            "class" | "classes" | "classification" => Some(Self::Class),
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
            Self::Deviation => "deviation",
            Self::Class => "class",
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
    /// Intensity normalised to the band, `0` = the band's darkest, `255` = its brightest. Stored
    /// after that stretch because scanners write anything from 8-bit values in a 16-bit field to
    /// a narrow dark band; `value / 257` would flatten the first case to one colour.
    pub intensity: u8,
}

/// A crop box in world metres: points outside it are not drawn, so a ceiling or the four walls
/// can be cut away without re-reading the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Clip {
    pub fn contains(&self, x: f64, y: f64, z: f64) -> bool {
        x >= self.min[0] && x <= self.max[0] && y >= self.min[1] && y <= self.max[1] && z >= self.min[2] && z <= self.max[2]
    }
}

/// One decimation level: points on a grid of `cell` metres (`0` = the source points).
#[derive(Clone, Debug)]
pub struct CloudLevel {
    pub cell: f64,
    pub pts: Vec<CloudVtx>,
    /// The LAS classification of every point, parallel to `pts` (2 = ground, 1 = unclassified).
    /// The 16-byte vertex does not carry it: the GPU never needs the class.
    pub classes: Vec<u8>,
    /// The group of every point, parallel to `pts`: the number of the group, from 0.
    pub clusters: Vec<u8>,
    /// The distance to a reference cloud, in metres, parallel to `pts`; `NaN` where the reference
    /// had no point nearby.
    pub dev: Vec<f32>,
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
    /// The 2nd and 98th percentiles of the band's Z: the elevation ramp uses them, so a few
    /// isolated outliers do not stretch the ramp and flatten the contrast of the other 99%.
    pub zp02: f64,
    pub zp98: f64,
    /// The band's intensity range, so the ramp uses what the scanner recorded instead of the
    /// nominal 0–65535 (a typical indoor slice sits in a narrow dark band and would look black).
    pub imin: u16,
    pub imax: u16,
    /// The same 2%–98% window for the intensity, in the 0..255 scale the vertices use.
    pub ip02: u8,
    pub ip98: u8,
    pub bounds: Bounds3,
    pub visible: bool,
    pub color: CloudColor,
    /// Crop box: `None` draws every point of the band.
    pub clip: Option<Clip>,
    /// Draw only the points the ground filter found.
    pub ground_only: bool,
    /// How opaque the cloud is drawn (0.05..=1.0). Below 1 the 3D view thins the points by a
    /// deterministic mask, so the drawing under the cloud stays readable.
    pub opacity: f32,
    /// The scale of the deviation colours, in metres (the 95th percentile of the comparison).
    pub dev_scale: f32,
    /// The statistics of the last comparison, when the cloud was compared with a reference.
    pub dev_stats: Option<crate::compare::DeviationStats>,
    /// The groups of the last clustering: their centres, their counts and the class each one was
    /// given.
    pub cluster_info: Vec<crate::cluster::Cluster>,
    /// The points the groups were read on: the nearest of them gives a point its group.
    pub cluster_sample: Vec<[f64; 3]>,
    /// The colour scale the intensity and the elevation bands are read on.
    pub spectrum: crate::spectrum::Spectrum,
    /// Whether the cloud belongs to the drawing it was opened in: an attached cloud is written
    /// into the file and comes back with it.
    pub attached: bool,
    /// The drawing the cloud belongs to (the index of the session's document).
    pub doc: usize,
    /// The band's heights in 64 bins, so the palette can draw the scan's profile and the user can
    /// see where the floor and the ceiling are.
    pub zhisto: [u32; 64],
    /// Where the file says it is (the EPSG code and the WKT), when it says anything.
    pub crs: Option<crate::Crs>,
    /// Finest first.
    pub levels: Vec<CloudLevel>,
    /// Points the band held before the `maxPoints` cap.
    pub full_points: usize,
}

/// Points per cloud by default: a 6-million-point band is 96 MB of 16-byte vertices and still
/// ten times more than a 4K screen can show at any zoom.
pub const DEFAULT_MAX_POINTS: usize = 6_000_000;

/// The most points `maxPoints: 0` ("every point of the band") will keep: 20 million is 320 MB of
/// vertices, the point where a workstation scan stops being comfortable.
pub const ALL_POINTS_LIMIT: usize = 20_000_000;

/// Files up to this many points are read whole when no `decimate` is asked. Bigger ones are
/// thinned to a 5 mm cube while streaming: 3.2 GB of `Pt` for a 100-million-point scan would
/// exhaust most machines before the display cap could help.
pub const SLICE_ALL_LIMIT: u64 = 8_000_000;

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
        let (imin, imax) = slice.points.iter().fold((u16::MAX, 0u16), |(lo, hi), p| (lo.min(p.intensity), hi.max(p.intensity)));
        let (imin, imax) = if slice.points.is_empty() { (0, 65535) } else { (imin, imax) };
        let norm = Norm { lo: imin, span: u32::from(imax.saturating_sub(imin)) };
        // The 2%–98% window of the Z and of the intensity: the colour ramps use it, so a handful
        // of outliers (a spike through the ceiling, a hot return) cannot flatten the contrast of
        // the other 99% of the points.
        let (zp02, zp98) = z_percentiles(&slice.points, zmin, zmax);
        let (ip02, ip98) = intensity_percentiles(&slice.points, norm);
        // No `decimate` (or `0`) means the source points themselves: the finest level keeps every
        // point of the band, up to the memory cap. A cell is the drafting answer for huge scans.
        let asked = decimate.filter(|c| *c > 0.0);
        let mut cell = asked.unwrap_or(0.0);
        let cap = if max_points == 0 { ALL_POINTS_LIMIT } else { max_points.max(10_000) };
        let (mut base, mut base_classes) = grid(&slice.points, cell, origin, norm);
        // Fit the cap, then keep coarsening: 1.6x cells make the next level ~2.5x lighter.
        while base.len() > cap && cell < 1e4 {
            cell = if cell > 0.0 { cell * 1.5 } else { 0.002 };
            (base, base_classes) = grid(&slice.points, cell, origin, norm);
        }
        let mut levels = vec![CloudLevel { cell, pts: base, classes: base_classes, clusters: Vec::new(), dev: Vec::new() }];
        while levels.last().is_some_and(|l| l.pts.len() > 20_000) && levels.len() < 16 {
            let c = if levels.last().is_some_and(|l| l.cell <= 0.0) { 0.005 } else { levels.last().map_or(0.01, |l| l.cell) * 1.6 };
            let (pts, classes) = grid(&slice.points, c, origin, norm);
            if pts.len() >= levels.last().map_or(0, |l| l.pts.len()) {
                break;
            }
            levels.push(CloudLevel { cell: c, pts, classes, clusters: Vec::new(), dev: Vec::new() });
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
            zp02,
            zp98,
            imin,
            imax,
            ip02,
            ip98,
            bounds: b,
            visible: true,
            color,
            clip: None,
            ground_only: false,
            opacity: 1.0,
            dev_scale: 1.0,
            dev_stats: None,
            cluster_info: Vec::new(),
            cluster_sample: Vec::new(),
            spectrum: crate::spectrum::Spectrum::BlueRed,
            attached: false,
            doc: 0,
            zhisto: heights(&slice.points, zmin, zmax),
            crs: None,
            levels,
            full_points: slice.in_band as usize,
        }
    }

    /// Read `path`, cut the band and build the levels.
    ///
    /// A file larger than [`SLICE_ALL_LIMIT`] points is thinned to a 5 mm cube while streaming, so
    /// the slice cannot hold a 100-million-point scan as 3.2 GB of `Pt`; the cap loop coarsens it
    /// further if needed. Smaller files are read whole, which is what "all points" means.
    pub fn open(path: &Path, name: String, z0: f64, z1: f64, decimate: Option<f64>, max_points: usize, color: CloudColor) -> Result<Self> {
        let info = crate::read_info(path)?;
        let asked = decimate.filter(|c| *c > 0.0);
        let eff = match asked {
            Some(c) => Some(c),
            None if info.points > SLICE_ALL_LIMIT => Some(0.005),
            None => None,
        };
        let cap = if max_points == 0 { ALL_POINTS_LIMIT } else { max_points.max(10_000) };
        // The read never holds more than the cap: the cell grows while the file streams.
        let slice = crate::slice_capped(path, z0, z1, eff, cap)?;
        Ok(Self::from_slice(name, path.display().to_string(), z0, z1, eff, slice, max_points, color).with_crs(info.crs))
    }

    /// Attach the coordinate reference system the file named.
    pub fn with_crs(mut self, crs: Option<crate::Crs>) -> Self {
        self.crs = crs;
        self
    }

    /// How opaque the cloud is drawn, clamped to what the display can show (5%..=100%).
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = if opacity.is_finite() { opacity.clamp(0.05, 1.0) } else { 1.0 };
    }

    pub fn points(&self) -> usize {
        self.levels.first().map_or(0, |l| l.pts.len())
    }

    /// The level to draw at `px_per_m` screen pixels per metre: the one whose grid spacing is
    /// nearest two pixels. The source-point level (`cell` 0) counts as sub-millimetre, so it
    /// appears only when the view is zoomed in far enough to resolve it.
    pub fn level_for(&self, px_per_m: f64) -> &CloudLevel {
        let want = if px_per_m.is_finite() && px_per_m > 0.0 { 2.0 / px_per_m } else { f64::INFINITY };
        let mut best = &self.levels[0];
        let mut best_d = f64::INFINITY;
        for l in &self.levels {
            let spacing = if l.cell > 0.0 { l.cell } else { 0.0002 };
            let d = if want > 0.0 { (spacing / want).ln().abs() } else { 0.0 };
            if d < best_d {
                best_d = d;
                best = l;
            }
        }
        best
    }

    /// Find the ground under the cloud. Every level is classified on its own, so the answer is as
    /// sharp as the level it belongs to: the fine levels see the furniture, the coarse ones the
    /// shape of the ground. The counts are the finest level's, the one the user draws.
    pub fn classify_ground(&mut self, options: &crate::ground::GroundOptions) -> crate::ground::GroundResult {
        let mut finest = crate::ground::GroundResult::default();
        for (i, level) in self.levels.iter_mut().enumerate() {
            let (classes, result) = crate::ground::classify_with(&level.pts, options, |v| {
                [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]
            });
            level.classes = classes;
            if i == 0 {
                finest = result;
            }
        }
        finest
    }

    /// The number of points the ground filter found in the finest level.
    pub fn ground_points(&self) -> usize {
        self.levels.first().map_or(0, |l| l.classes.iter().filter(|c| **c == crate::ground::CLASS_GROUND).count())
    }

    /// Whether the cloud has been classified at all: the ground filter's word, or nothing.
    pub fn has_classes(&self) -> bool {
        self.levels.iter().any(|l| l.classes.iter().any(|c| *c != crate::classify::CLASS_UNCLASSIFIED))
    }

    /// Classify the cloud: the ground, the vegetation, the walls, the noise.
    ///
    /// The ground filter's word is the base: the heights are measured from the ground it found.
    pub fn classify(&mut self, options: &crate::classify::ClassifyOptions) -> crate::classify::ClassifyResult {
        let mut finest = crate::classify::ClassifyResult::default();
        for (i, level) in self.levels.iter_mut().enumerate() {
            let points: Vec<[f64; 3]> =
                level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect();
            let (classes, result) = crate::classify::classify_level(&points, &level.classes, options);
            level.classes = classes;
            if i == 0 {
                finest = result;
            }
        }
        finest
    }

    /// Group the cloud's points by what they are: the height above the ground, the intensity, the
    /// roughness of the surface and how much it stands up.
    ///
    /// The groups come back with a class proposed for each one; the user names them afterwards.
    pub fn cluster(&mut self, k: usize, options: &crate::classify::ClassifyOptions) -> Vec<crate::cluster::Cluster> {
        let Some(level) = self.levels.first() else { return Vec::new() };
        let points: Vec<[f64; 3]> =
            level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect();
        let intensities: Vec<u8> = level.pts.iter().map(|v| v.intensity).collect();
        let sample = crate::classify::sample_level(&points, &level.classes, options);
        let features = crate::cluster::features_of(&sample, &intensities);
        let (labels, clusters) = crate::cluster::clusters_of(&features, k);
        // The group of every point: the group of its nearest sampled point.
        let grid = crate::grid::Grid::new(&sample.points, options.radius.max(options.cell * 0.5));
        let mut near = Vec::new();
        for (i, level) in self.levels.iter_mut().enumerate() {
            let level_points: Vec<[f64; 3]> =
                level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect();
            let mut out = vec![0u8; level_points.len()];
            for (n, p) in level_points.iter().enumerate() {
                grid.around(*p, &mut near);
                let mut best = f64::INFINITY;
                let mut best_at = 0usize;
                for &j in near.iter() {
                    let q = sample.points[j as usize];
                    let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2);
                    if d < best {
                        best = d;
                        best_at = j as usize;
                    }
                }
                if best.is_finite() {
                    out[n] = labels[best_at] as u8;
                }
            }
            let _ = i;
            level.clusters = out;
        }
        self.cluster_sample = sample.points;
        self.cluster_info = clusters;
        self.cluster_info.clone()
    }

    /// Give the groups their classes, after the user named one of them.
    pub fn apply_clusters(&mut self) {
        let classes: Vec<u8> = self.cluster_info.iter().map(|c| c.class).collect();
        for level in self.levels.iter_mut() {
            for (i, c) in level.classes.iter_mut().enumerate() {
                let label = level.clusters.get(i).copied().unwrap_or(0) as usize;
                *c = classes.get(label).copied().unwrap_or(crate::classify::CLASS_UNCLASSIFIED);
            }
        }
    }

    /// The colour of one vertex under the cloud's colour mode.
    ///
    /// `deviation` is the point's distance to the reference cloud, when the cloud was compared with
    /// one and the colour mode asks for the deviation scale. `class` is the point's classification,
    /// when the colour mode asks for the class colours.
    pub fn color_of(&self, v: &CloudVtx, deviation: Option<f32>, class: Option<u8>) -> [u8; 3] {
        match self.color {
            CloudColor::Rgb => v.rgb,
            // A spectrum, not grey: the band's darkest return reads blue and its brightest red,
            // so two walls with slightly different returns stay distinguishable. The ramp runs
            // over the 2%–98% window, so a few hot returns cannot flatten the rest.
            CloudColor::Intensity => {
                let span = f32::from(self.ip98) - f32::from(self.ip02);
                let t = if span > 0.0 { (f32::from(v.intensity) - f32::from(self.ip02)) / span } else { 0.5 };
                self.spectrum.apply(t.clamp(0.0, 1.0))
            }
            CloudColor::Single(c) => c,
            CloudColor::Deviation => match deviation {
                Some(d) => crate::compare::deviation_color(d, self.dev_scale),
                None => [120, 120, 120],
            },
            CloudColor::Class => crate::classify::class_color(class.unwrap_or(crate::classify::CLASS_UNCLASSIFIED)),
            CloudColor::Elevation => {
                let span = self.zp98 - self.zp02;
                let t = if span > 0.0 { (f64::from(v.z) - self.zp02) / span } else { 0.5 };
                self.spectrum.apply(t.clamp(0.0, 1.0) as f32)
            }
        }
    }

    /// Compare the cloud with a reference: every point gets its distance to the nearest point of
    /// the reference, and the statistics of those distances come back.
    ///
    /// The reference must be denser than this cloud and must cover it; the points that find no
    /// neighbour are counted, not given a distance.
    pub fn compare_with(&mut self, reference: &Cloud, wanted_cell: f64) -> crate::compare::DeviationStats {
        let points = reference.world_points();
        let cell = reference.search_cell(wanted_cell);
        self.compare_with_points(&points, cell)
    }

    /// The size of the search cell of a comparison with this cloud as the reference: four times
    /// the spacing of its points, so the neighbourhood that is looked at covers a misalignment of
    /// two spacings and the nearest point is still found.
    pub fn search_cell(&self, wanted_cell: f64) -> f64 {
        let mut cell = match self.levels.first() {
            Some(level) if level.cell > 0.0 => level.cell * 4.0,
            _ => 0.05,
        };
        // A cell asked for at the command line only widens the search.
        if wanted_cell > 0.0 && wanted_cell > cell {
            cell = wanted_cell;
        }
        cell
    }

    /// Compare the cloud with a reference given as points, in world coordinates.
    ///
    /// The points are taken as they are, so the caller does not have to copy the whole reference
    /// cloud: a hundred-million-point scan would not fit twice.
    pub fn compare_with_points(&mut self, reference_points: &[[f64; 3]], cell: f64) -> crate::compare::DeviationStats {
        let cell = if cell > 0.0 { cell } else { 0.05 };
        let mut stats = crate::compare::DeviationStats::default();
        for level in &mut self.levels {
            let tested: Vec<[f64; 3]> =
                level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect();
            level.dev = crate::compare::distances(&tested, reference_points, cell);
        }
        if let Some(finest) = self.levels.first() {
            stats = crate::compare::stats(&finest.dev);
        }
        // The colour scale reaches the 95th percentile, so a single spike does not wash the map out.
        self.dev_scale = if stats.p95 > 0.0 { stats.p95 as f32 } else { 1.0 };
        self.dev_stats = Some(stats);
        stats
    }

    /// The cloud's points in world coordinates, its finest level: the reference for a comparison.
    pub fn world_points(&self) -> Vec<[f64; 3]> {
        self.levels
            .first()
            .map(|level| level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect())
            .unwrap_or_default()
    }
}

/// The bins of the Z histogram: 65536 keeps a half-metre wall resolvable even when one outlier
/// stretches the range over a kilometre.
const Z_BINS: usize = 65_536;
/// The bins of the intensity histogram, in the 0..255 scale the vertices use.
const INTENSITY_BINS: usize = 256;

/// The 2nd and 98th percentile of a histogram over `[lo, hi]`, as values in the same range.
fn bin_percentiles(counts: &[u32], lo: f64, hi: f64, total: usize) -> (f64, f64) {
    if total == 0 || counts.len() < 2 {
        return (lo, hi);
    }
    let step = (hi - lo) / (counts.len() - 1) as f64;
    let low_at = (total as f64 * 0.02).ceil() as u64;
    let high_at = (total as f64 * 0.98).floor() as u64;
    let mut acc = 0u64;
    let mut low = None;
    let mut high = None;
    for (i, c) in counts.iter().enumerate() {
        acc += u64::from(*c);
        if low.is_none() && acc >= low_at {
            low = Some(lo + step * i as f64);
        }
        if acc >= high_at {
            high = Some(lo + step * i as f64);
            break;
        }
    }
    match (low, high) {
        (Some(l), Some(h)) if h > l => (l, h),
        // A band that lives in one bin has no window: the full range is the honest answer.
        _ => (lo, hi),
    }
}

/// The 2nd and 98th percentile of the band's Z, from a histogram (no sorting of four million
/// points).
fn z_percentiles(points: &[crate::Pt], zmin: f64, zmax: f64) -> (f64, f64) {
    if points.is_empty() || !zmin.is_finite() || !zmax.is_finite() {
        return (zmin, zmax);
    }
    let mut hist = vec![0u32; Z_BINS];
    let span = zmax - zmin;
    for p in points {
        let i = if span > 0.0 { (((p.z - zmin) / span) * (Z_BINS - 1) as f64).round() } else { 0.0 };
        let i = (i as usize).min(Z_BINS - 1);
        hist[i] = hist[i].saturating_add(1);
    }
    bin_percentiles(&hist, zmin, zmax, points.len())
}

/// The same window for the intensity, in the 0..255 scale the vertices store: a hot return
/// cannot stretch the ramp and flatten the wall behind it.
fn intensity_percentiles(points: &[crate::Pt], norm: Norm) -> (u8, u8) {
    if points.is_empty() {
        return (0, 255);
    }
    let mut hist = vec![0u32; INTENSITY_BINS];
    for p in points {
        let v = norm.u8(p.intensity) as usize;
        hist[v.min(INTENSITY_BINS - 1)] = hist[v.min(INTENSITY_BINS - 1)].saturating_add(1);
    }
    let (lo, hi) = bin_percentiles(&hist, 0.0, 255.0, points.len());
    (lo as u8, hi as u8)
}

/// Stretches 16-bit intensities to the band: the darkest becomes 0, the brightest 255.
#[derive(Clone, Copy)]
struct Norm {
    lo: u16,
    span: u32,
}

impl Norm {
    fn u8(self, v: u16) -> u8 {
        let scaled = u32::from(v.saturating_sub(self.lo)) * 255 + self.span / 2;
        // A band with one intensity value (`span` 0) sits mid-ramp: visible, no fake contrast.
        scaled.checked_div(self.span).map_or(128, |t| t.min(255) as u8)
    }
}

/// The band's heights in 64 bins, so the palette can draw the scan's profile: the floor and the
/// ceiling show up as two wide humps, a wall as a thin one.
fn heights(points: &[crate::Pt], zmin: f64, zmax: f64) -> [u32; 64] {
    let mut bins = [0u32; 64];
    let span = zmax - zmin;
    if span <= 0.0 || !span.is_finite() {
        return bins;
    }
    for p in points {
        let i = (((p.z - zmin) / span) * 63.0).round().clamp(0.0, 63.0) as usize;
        bins[i] = bins[i].saturating_add(1);
    }
    bins
}

/// One point per `cell`-metre cube, keeping the first in file order; `cell` 0 keeps every point.
///
/// The cube is three-dimensional on purpose: an XY-only grid keeps one point per column, so a
/// vertical wall loses every point but one and the walls disappear from the 3D view.
///
/// The classification of the point that was kept travels with it.
fn grid(points: &[crate::Pt], cell: f64, origin: [f64; 2], norm: Norm) -> (Vec<CloudVtx>, Vec<u8>) {
    let mut out = Vec::with_capacity(points.len().min(4_000_000));
    let mut classes = Vec::with_capacity(points.len().min(4_000_000));
    let mut vtx = |p: &crate::Pt| CloudVtx {
        x: (p.x - origin[0]) as f32,
        y: (p.y - origin[1]) as f32,
        z: p.z as f32,
        rgb: p.rgb.unwrap_or([220, 220, 220]),
        intensity: norm.u8(p.intensity),
    };
    if cell <= 0.0 || !cell.is_finite() {
        out.extend(points.iter().map(&mut vtx));
        classes.extend(points.iter().map(|p| p.class));
        return (out, classes);
    }
    let inv = 1.0 / cell;
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::with_capacity(points.len().min(4_000_000));
    for p in points {
        let key = (((p.x - origin[0]) * inv).floor() as i64, ((p.y - origin[1]) * inv).floor() as i64, (p.z * inv).floor() as i64);
        if !seen.insert(key) {
            continue;
        }
        out.push(vtx(p));
        classes.push(p.class);
    }
    (out, classes)
}

/// Blue → cyan → green → yellow → red, the drafting-friendly stand-in for a turbo ramp.
#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64, z: f64) -> crate::Pt {
        crate::Pt { x, y, z, rgb: Some([10, 20, 30]), intensity: 1000, class: 1 }
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
        let lo = c.color_of(&c.levels[0].pts[0], None, None);
        let hi = c.color_of(&c.levels[0].pts[1], None, None);
        assert_ne!(lo, hi, "the ramp spans the band");
        let mut s = c.clone();
        s.color = CloudColor::Single([1, 2, 3]);
        assert_eq!(s.color_of(&s.levels[0].pts[0], None, None), [1, 2, 3]);
        assert_eq!(CloudColor::parse("#00ff00"), Some(CloudColor::Single([0, 255, 0])));
        assert_eq!(CloudColor::parse("rgb"), Some(CloudColor::Rgb));
    }

    #[test]
    fn intensity_spans_the_band_as_a_spectrum() {
        let mk = |x: f64, i: u16| crate::Pt { x, y: 0.0, z: 1.0, rgb: None, intensity: i, class: 1 };
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
        let lo = c.color_of(&c.levels[0].pts[0], None, None);
        let hi = c.color_of(&c.levels[0].pts[1], None, None);
        assert_ne!(lo, hi, "the ramp spans the band");
        assert!(lo[2] > lo[0], "the darkest return reads blue: {lo:?}");
        assert!(hi[0] > hi[2], "the brightest reads red: {hi:?}");
    }

    /// Scanners often write 8-bit intensities into the 16-bit field (`0..=255`). Dividing by 257
    /// flattened every one of them to 0, so the whole cloud took the span-zero colour.
    #[test]
    fn eight_bit_intensities_keep_their_contrast() {
        let mk = |x: f64, i: u16| crate::Pt { x, y: 0.0, z: 1.0, rgb: None, intensity: i, class: 1 };
        let c = Cloud::from_slice(
            "t".into(),
            String::new(),
            0.0,
            2.0,
            Some(0.01),
            slice_of(vec![mk(0.0, 0), mk(1.0, 29), mk(2.0, 255)]),
            1000,
            CloudColor::Intensity,
        );
        let i: Vec<u8> = c.levels[0].pts.iter().map(|p| p.intensity).collect();
        assert_eq!(i, [0, 29, 255], "the band's range is stretched, not truncated");
        assert_eq!(c.imin, 0);
        assert_eq!(c.imax, 255);
    }

    #[test]
    fn no_decimation_keeps_every_band_point() {
        let pts: Vec<_> = (0..500).map(|i| pt(f64::from(i) * 0.001, 0.0, 1.0)).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, None, slice_of(pts), 100_000, CloudColor::Rgb);
        assert_eq!(c.levels[0].cell, 0.0, "the finest level is the source points");
        assert_eq!(c.points(), 500);
    }

    /// An XY-only grid keeps one point per column, which empties a vertical wall; the grid is a
    /// cube so height survives.
    #[test]
    fn the_grid_keeps_a_wall_vertical() {
        let pts = vec![pt(1.0, 1.0, 1.0), pt(1.0, 1.0, 1.5), pt(1.0, 1.0, 2.0)];
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 3.0, Some(0.01), slice_of(pts), 1000, CloudColor::Rgb);
        assert_eq!(c.points(), 3);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(Cloud::open(Path::new("no-such-file.las"), "x".into(), 0.0, 1.0, None, 1000, CloudColor::Rgb).is_err());
    }

    /// A spike through the ceiling must not stretch the ramp: the wall keeps its gradient.
    #[test]
    fn outliers_do_not_flatten_the_elevation_ramp() {
        let mut pts: Vec<crate::Pt> = (0..100).map(|i| pt(f64::from(i) * 0.01, 0.0, 2.0 + f64::from(i) * 0.005)).collect();
        pts.push(pt(0.0, 0.0, 1000.0));
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2000.0, Some(0.001), slice_of(pts), 100_000, CloudColor::Elevation);
        assert!((c.zp02 - 2.0).abs() < 0.05, "the window starts at the wall: {}", c.zp02);
        assert!((c.zp98 - 2.5).abs() < 0.05, "and ends at its top: {}", c.zp98);
        let colors: Vec<[u8; 3]> = c.levels[0].pts.iter().map(|p| c.color_of(p, None, None)).collect();
        assert_ne!(colors[0], colors[99], "the wall still shows its gradient");
        assert_eq!(colors[100], colors[99], "the spike takes the top colour");
    }

    /// The same for the intensity: one hot return cannot flatten the wall behind it.
    #[test]
    fn outliers_do_not_flatten_the_intensity_ramp() {
        let mk = |x: f64, i: u16| crate::Pt { x, y: 0.0, z: 1.0, rgb: None, intensity: i, class: 1 };
        let mut pts: Vec<crate::Pt> = (0..100).map(|i| mk(f64::from(i) * 0.01, 1000 + (i as u16) * 5)).collect();
        pts.push(mk(5.0, 60000));
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.001), slice_of(pts), 100_000, CloudColor::Intensity);
        let colors: Vec<[u8; 3]> = c.levels[0].pts.iter().map(|p| c.color_of(p, None, None)).collect();
        assert_ne!(colors[0], colors[99], "the wall keeps its gradient");
        assert_eq!(colors[100], colors[99], "the hot return takes the top colour");
    }

    /// Without an outlier the window is the band minus its 2% tails, so a clean scan keeps its
    /// whole gradient.
    #[test]
    fn a_clean_band_uses_its_whole_range() {
        let pts: Vec<crate::Pt> = (0..100).map(|i| pt(f64::from(i) * 0.01, 0.0, 1.0 + f64::from(i) * 0.01)).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 3.0, Some(0.001), slice_of(pts), 100_000, CloudColor::Elevation);
        assert!((c.zp02 - c.zmin).abs() < 0.02, "the window starts at the floor: {} vs {}", c.zp02, c.zmin);
        assert!((c.zp98 - (c.zmax - 0.02 * (c.zmax - c.zmin))).abs() < 0.02, "and ends at the 98th percentile: {} vs {}", c.zp98, c.zmax);
    }

    /// A band with one value has no window: the ramp sits in the middle instead of pretending.
    #[test]
    fn the_intensity_is_read_on_the_chosen_scale() {
        let mut c = Cloud::from_slice(
            "t".into(),
            String::new(),
            0.0,
            1.0,
            Some(0.01),
            slice_of(vec![pt(0.0, 0.0, 0.5), pt(1.0, 0.0, 0.5)]),
            100_000,
            CloudColor::Intensity,
        );
        let default = c.color_of(&c.levels[0].pts[0], None, None);
        c.spectrum = crate::spectrum::Spectrum::Grey;
        let as_grey = c.color_of(&c.levels[0].pts[0], None, None);
        c.spectrum = crate::spectrum::Spectrum::Viridis;
        let as_viridis = c.color_of(&c.levels[0].pts[0], None, None);
        assert_ne!(as_grey, as_viridis, "the scale changes the colour");
        assert_eq!(as_grey[0], as_grey[1], "the grey scale has no hue");
        assert_eq!(as_grey[1], as_grey[2]);
        assert_ne!(default, as_viridis, "the default scale is not the viridis one");
    }

    #[test]
    fn a_flat_band_does_not_panic() {
        let pts: Vec<crate::Pt> = (0..10).map(|i| pt(f64::from(i) * 0.01, 0.0, 1.0)).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.001), slice_of(pts), 1000, CloudColor::Elevation);
        let color = c.color_of(&c.levels[0].pts[0], None, None);
        assert!(color.iter().all(|v| *v < 255), "the mid ramp: {color:?}");
    }
}
