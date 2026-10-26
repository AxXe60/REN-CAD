//! A band kept for the canvas: decimation levels and colour modes.
//!
//! [`crate::slice`] streams the file once and returns every point in the band. For display that
//! is too many and too fat: a 12-million-point scan at 32 bytes is 384 MB and no GPU wants a
//! 12-million-quad instance buffer. So the band is turned into a pyramid of grid-decimated levels
//! (one point per cell, cells doubling), stored as 16-byte vertices relative to the cloud's
//! origin, and the canvas picks the level whose spacing is about two screen pixels. Zoomed out it
//! draws thousands; zoomed in, more — never more than the screen can show.

use std::collections::HashMap;
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
    /// The two endpoints the user named for the elevation ramp, in metres. `None` is Auto: the
    /// band's own 2%–98% window. A scan whose interesting wall sits in a tenth of its height reads
    /// as one flat colour on the auto window, and this is how the user pulls it apart.
    pub z_window: Option<(f64, f64)>,
    /// The same for the intensity ramp, on the 0..255 scale the vertices and the histogram use.
    pub i_window: Option<(u8, u8)>,
    pub bounds: Bounds3,
    pub visible: bool,
    pub color: CloudColor,
    /// Display multipliers for the scanner's RGB, both bounded to 0.25..=4 by LASCOLOR and Auto.
    pub rgb_exposure: f32,
    pub rgb_contrast: f32,
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
    /// How many times the class bytes were rewritten in place. The classifiers change `level.classes`
    /// without moving a point, and the drawing caches a plan by the cloud's state: without this
    /// counter a re-classification keeps the colours of the one before it.
    pub class_revision: u64,
    /// The colour scale the intensity and the elevation bands are read on.
    pub spectrum: crate::spectrum::Spectrum,
    /// Whether the cloud belongs to the drawing it was opened in: an attached cloud is written
    /// into the file and comes back with it.
    pub attached: bool,
    /// The drawing the cloud belongs to (the index of the session's document).
    pub doc: usize,
    /// Where the cloud sits in the drawing: the world point that is the drawing's (0, 0, 0). The
    /// cloud is drawn relative to it, so a scan loaded from another computer lands in the same
    /// place in the drawing.
    pub offset: [f64; 3],
    /// The band's heights in 64 bins, so the palette can draw the scan's profile and the user can
    /// see where the floor and the ceiling are.
    pub zhisto: [u32; 64],
    /// The band's returns in 64 bins of the 0..255 scale, so the same palette can show where the
    /// bright walls sit and how wide the window the ramp reads is.
    pub ihisto: [u32; 64],
    /// Where the file says it is (the EPSG code and the WKT), when it says anything.
    pub crs: Option<crate::Crs>,
    /// Finest first.
    pub levels: Vec<CloudLevel>,
    /// Points the band held before the `maxPoints` cap.
    pub full_points: usize,
    /// The points the file holds, band or no band: the header's own count.
    pub file_points: usize,
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
        mut slice: Slice,
        max_points: usize,
        color: CloudColor,
    ) -> Self {
        crate::normalize_rgb(&mut slice.points);
        let b = slice.bounds;
        let origin = [b.min[0], b.min[1]];
        let (zmin, zmax) = (b.min[2], b.max[2]);
        // The four colour and profile windows in one walk: the intensity range, both 2%-98%
        // window histograms and the height profile. The u8 stretch of the intensity comes after,
        // because the stretch is defined by the range this walk produces.
        let windows = band_windows(&slice.points, zmin, zmax);
        let (imin, imax) = if slice.points.is_empty() { (0, 65535) } else { (windows.imin, windows.imax) };
        let norm = Norm { lo: imin, span: u32::from(imax.saturating_sub(imin)) };
        // The 2%–98% window of the Z and of the intensity: the colour ramps use it, so a handful
        // of outliers (a spike through the ceiling, a hot return) cannot flatten the contrast of
        // the other 99% of the points.
        let (zp02, zp98) = bin_percentiles(&windows.zhist, zmin, zmax, slice.points.len());
        let (ip02, ip98) = intensity_percentiles(&windows.ihist, norm);
        // The returns in the 64 bins the palette draws: folded from the counts of the same walk,
        // so the bars are the numbers the percentiles above were read from.
        let ihisto = fold_to_64(&windows.ihist, norm);
        // No `decimate` (or `0`) means the source points themselves: the finest level keeps every
        // point of the band, up to the memory cap. A cell is the drafting answer for huge scans.
        let asked = decimate.filter(|c| *c > 0.0);
        let mut cell = asked.unwrap_or(0.0);
        let cap = if max_points == 0 { ALL_POINTS_LIMIT } else { max_points.max(10_000) };
        let (mut base, mut base_classes, mut band_idx) = grid(&slice.points, cell, origin, norm);
        // Fit the cap, then keep coarsening: 1.6x cells make the next level ~2.5x lighter.
        while base.len() > cap && cell < 1e4 {
            cell = if cell > 0.0 { cell * 1.5 } else { 0.002 };
            (base, base_classes, band_idx) = grid(&slice.points, cell, origin, norm);
        }
        let mut levels = vec![CloudLevel { cell, pts: base, classes: base_classes, clusters: Vec::new(), dev: Vec::new() }];
        // Each coarser level is decimated from the level below it, not from the band again: the
        // walk shrinks with every step. The cell doubles each step, because that is what makes the
        // two ways agree: every cube of the level below then sits inside one cube of the new
        // level, so the band's first point of a coarse cube has no earlier band point in its own
        // finer cube (an earlier one would sit in the same coarse cube and contradict its
        // firstness), and walking the level below keeps it first of its coarse cube exactly as
        // walking the band would. A cell that is not a whole multiple of the one below lets a fine
        // cube straddle a coarse boundary; the band's first point of the coarse cube is then a
        // point the level below may have dropped (an earlier point took its fine cube while
        // landing in the neighbouring coarse cube), and the levels drift apart.
        //
        // The pyramid goes on while the coarsest level is too heavy for a screen, and at least to
        // a third level, so a medium band still zooms out in steps; a band of one point has
        // nothing left to coarsen.
        while levels.len() < 16 && levels.last().is_some_and(|l| l.pts.len() > 20_000 || (levels.len() < 3 && l.pts.len() > 1)) {
            let last = match levels.last() {
                Some(l) => l,
                None => break,
            };
            let c = if last.cell <= 0.0 { 0.005 } else { last.cell * 2.0 };
            let (pts, classes, kept) = grid_below(&slice.points, &band_idx, &last.pts, &last.classes, c, origin);
            if pts.len() >= last.pts.len() {
                break;
            }
            levels.push(CloudLevel { cell: c, pts, classes, clusters: Vec::new(), dev: Vec::new() });
            band_idx = kept;
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
            z_window: None,
            i_window: None,
            bounds: b,
            visible: true,
            color,
            rgb_exposure: 1.0,
            rgb_contrast: 1.0,
            clip: None,
            ground_only: false,
            opacity: 1.0,
            dev_scale: 1.0,
            dev_stats: None,
            cluster_info: Vec::new(),
            cluster_sample: Vec::new(),
            class_revision: 0,
            spectrum: crate::spectrum::Spectrum::BlueRed,
            attached: false,
            doc: 0,
            offset: [0.0; 3],
            zhisto: windows.profile,
            ihisto,
            crs: None,
            levels,
            full_points: slice.in_band as usize,
            file_points: slice.total as usize,
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

    /// The level to draw at this zoom, keeping the level drawn last while it still reads well.
    ///
    /// [`level_for`] answers with the level nearest the ideal two-pixel spacing, so a zoom that
    /// hovers at the boundary between two levels flips between them from one frame to the next and
    /// the cloud changes density under the cursor. This keeps `last_cell` while its spacing on
    /// screen is between 1.25 and 3.2 pixels: a wobble around the boundary then costs nothing, and
    /// a real zoom still moves on at the step it always did.
    pub fn level_for_sticky(&self, px_per_m: f64, last_cell: Option<f64>) -> &CloudLevel {
        let kept = last_cell.and_then(|cell| {
            let spacing = if cell > 0.0 { cell } else { 0.0002 };
            let px = spacing * px_per_m;
            if px_per_m.is_finite() && px_per_m > 0.0 && (1.25..=3.2).contains(&px) {
                self.levels.iter().find(|l| l.cell.to_bits() == cell.to_bits())
            } else {
                None
            }
        });
        kept.unwrap_or_else(|| self.level_for(px_per_m))
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
        self.class_revision = self.class_revision.saturating_add(1);
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
        self.class_revision = self.class_revision.saturating_add(1);
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
        self.class_revision = self.class_revision.saturating_add(1);
    }

    /// Exposure and contrast chosen from the finest level's luminance percentiles.
    /// Empty clouds keep neutral shading, and so does a cloud whose colour has no spread: contrast
    /// has nothing to stretch there, and stretching it would push a flat field past white.
    pub fn auto_rgb(&self) -> (f32, f32) {
        let Some(level) = self.levels.first().filter(|level| !level.pts.is_empty()) else { return (1.0, 1.0) };
        let mut luminance = Vec::with_capacity(level.pts.len());
        let mut sum = 0.0f64;
        for v in &level.pts {
            let [r, g, b] = v.rgb.map(f32::from);
            let value = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
            luminance.push(value);
            sum += f64::from(value);
        }
        luminance.sort_by(f32::total_cmp);
        let last = luminance.len().saturating_sub(1);
        let at = |fraction: f32| luminance.get(((last as f32 * fraction).round() as usize).min(last)).copied().unwrap_or(0.5);
        let span = at(0.98) - at(0.02);
        // Under two hundredths of the scale there is no spread to gain, and maximum contrast on a
        // near-flat field moves every channel past the clamp: the picture goes white.
        let contrast = if span > 0.02 { (0.8 / span).clamp(0.25, 4.0) } else { 1.0 };
        let mean = (sum / luminance.len() as f64) as f32;
        let adjusted = ((mean - 0.5) * contrast + 0.5).clamp(0.05, 1.0);
        let exposure = (0.55 / adjusted).clamp(0.25, 4.0);
        (exposure, contrast)
    }

    /// The window the elevation ramp reads on: the endpoints the user named, or the band's own
    /// 2%–98% window when they named none.
    pub fn elevation_window(&self) -> (f64, f64) {
        self.z_window.unwrap_or((self.zp02, self.zp98))
    }

    /// The window the intensity ramp reads on, on the 0..255 scale the vertices and `ihisto` use.
    pub fn intensity_window(&self) -> (u8, u8) {
        self.i_window.unwrap_or((self.ip02, self.ip98))
    }

    /// The bin of `ihisto` a return of this 0..255 value is counted in — the bin the palette lights
    /// up when it names a value back to the user.
    pub fn bin_of_intensity(&self, value: u8) -> usize {
        bin_of_u8(value)
    }

    /// The colour of one vertex under the cloud's colour mode.
    ///
    /// `deviation` is the point's distance to the reference cloud, when the cloud was compared with
    /// one and the colour mode asks for the deviation scale. `class` is the point's classification,
    /// when the colour mode asks for the class colours.
    pub fn color_of(&self, v: &CloudVtx, deviation: Option<f32>, class: Option<u8>) -> [u8; 3] {
        match self.color {
            CloudColor::Rgb => v.rgb.map(|channel| {
                let value = f32::from(channel) / 255.0;
                ((((value - 0.5) * self.rgb_contrast + 0.5) * self.rgb_exposure).clamp(0.0, 1.0) * 255.0).round() as u8
            }),
            // A spectrum, not grey: the band's darkest return reads blue and its brightest red,
            // so two walls with slightly different returns stay distinguishable. The ramp runs over
            // the window the user left on Auto (the 2%–98% one, so a few hot returns cannot flatten
            // the rest) or the one they named.
            CloudColor::Intensity => {
                let (lo, hi) = self.intensity_window();
                let span = f32::from(hi) - f32::from(lo);
                let t = if span > 0.0 { (f32::from(v.intensity) - f32::from(lo)) / span } else { 0.5 };
                self.spectrum.apply(t.clamp(0.0, 1.0))
            }
            CloudColor::Single(c) => c,
            CloudColor::Deviation => match deviation {
                Some(d) => crate::compare::deviation_color(d, self.dev_scale),
                None => crate::compare::DEVIATION_NO_DATA,
            },
            CloudColor::Class => crate::classify::class_color(class.unwrap_or(crate::classify::CLASS_UNCLASSIFIED)),
            CloudColor::Elevation => {
                let (lo, hi) = self.elevation_window();
                let span = hi - lo;
                let t = if span > 0.0 { (f64::from(v.z) - lo) / span } else { 0.5 };
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
        // One grid over the reference, for every level: the reference does not change from one
        // level of the tested cloud to the next.
        let grid = crate::grid::Grid::new(reference_points, cell);
        for level in &mut self.levels {
            let tested: Vec<[f64; 3]> =
                level.pts.iter().map(|v| [f64::from(v.x) + self.origin[0], f64::from(v.y) + self.origin[1], f64::from(v.z)]).collect();
            level.dev = crate::compare::distances_with_grid(&tested, reference_points, &grid);
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

/// The four colour and profile windows of the band, read in one walk of the points: the
/// intensity range, the 2%-98% window histograms of the Z and of the intensity (in the 16-bit
/// scale the u8 stretch comes afterwards, from the same counts), and the 64-bin height profile
/// the palette draws.
struct BandWindows {
    imin: u16,
    imax: u16,
    /// The Z counts over `[zmin, zmax]` in `Z_BINS` bins: the percentiles read them.
    zhist: Vec<u32>,
    /// The intensity counts in the 16-bit scale: the u8 counts come from these after the walk,
    /// because the band's range (which the stretch needs) is only known once it is over.
    ihist: Vec<u32>,
    /// The same heights in 64 bins, so the palette can draw the scan's profile: the floor and
    /// the ceiling show up as two wide humps, a wall as a thin one.
    profile: [u32; 64],
}

/// The Z bins of the percentile histogram: 65536 keeps a half-metre wall resolvable even when one
/// outlier stretches the range over a kilometre.
const Z_BINS: usize = 65_536;
/// The bins of the intensity histogram, in the 0..255 scale the vertices use.
const INTENSITY_BINS: usize = 256;

/// The 2nd and 98th percentile of the band's Z and of its intensity, the intensity range and the
/// 64-bin height profile, all from one walk of the points (no sorting of four million points, no
/// four walks of them).
fn band_windows(points: &[crate::Pt], zmin: f64, zmax: f64) -> BandWindows {
    let mut out = BandWindows { imin: u16::MAX, imax: 0, zhist: vec![0u32; Z_BINS], ihist: vec![0u32; Z_BINS], profile: [0u32; 64] };
    // One guard serves both Z histograms: a band that lives in one height has no window, and a
    // non-finite bound stretches the range without end.
    let span = zmax - zmin;
    let bin_z = span > 0.0 && span.is_finite();
    for p in points {
        out.imin = out.imin.min(p.intensity);
        out.imax = out.imax.max(p.intensity);
        out.ihist[usize::from(p.intensity)] += 1;
        if bin_z {
            let i = (((p.z - zmin) / span) * (Z_BINS - 1) as f64).round();
            let i = (i as usize).min(Z_BINS - 1);
            out.zhist[i] = out.zhist[i].saturating_add(1);
            let j = (((p.z - zmin) / span) * 63.0).round().clamp(0.0, 63.0) as usize;
            out.profile[j] = out.profile[j].saturating_add(1);
        }
    }
    out
}

/// The 2%–98% percentile pairs of a histogram over `[lo, hi]`, as values in the same range.
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

/// The same window for the intensity, in the 0..255 scale the vertices store: a hot return
/// cannot stretch the ramp and flatten the wall behind it.
///
/// The counts arrive from the walk in the 16-bit scale, and the stretch that folds them into the
/// 0..255 bins needs the band's range, which only the walk produces: so the fold happens after
/// it, bin by bin, over the same numbers the per-point stretch would have histogrammed.
fn intensity_percentiles(raw: &[u32], norm: Norm) -> (u8, u8) {
    let total: usize = raw.iter().map(|c| *c as usize).sum();
    let mut hist = vec![0u32; INTENSITY_BINS];
    for (v, n) in raw.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let b = usize::from(norm.u8(v as u16)).min(INTENSITY_BINS - 1);
        hist[b] = hist[b].saturating_add(*n);
    }
    let (lo, hi) = bin_percentiles(&hist, 0.0, 255.0, total);
    (lo as u8, hi as u8)
}

/// The 64-bin return histogram, folded from the same walk's 65 536 counts: a 16-bit value first
/// takes the 0..255 stretch the vertices carry, then the bin of that scale. The counts of a value
/// therefore land where the ramp reads it, and every point of the band lands in exactly one bin.
fn fold_to_64(raw: &[u32], norm: Norm) -> [u32; 64] {
    let mut out = [0u32; 64];
    for (v, n) in raw.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let b = bin_of_u8(norm.u8(v as u16));
        out[b] = out[b].saturating_add(*n);
    }
    out
}

/// The bin of the 0..255 return scale a vertex's own intensity falls in: four values per bin.
fn bin_of_u8(v: u8) -> usize {
    (usize::from(v) * 64 / INTENSITY_BINS).min(63)
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

/// One grid cell's display accumulator. Its representative remains the first real point that
/// entered the cell; only RGB and intensity average the cell's points.
#[derive(Clone, Copy)]
struct CellAcc {
    out: usize,
    count: u64,
    rgb: [u64; 3],
    intensity: u64,
}

/// Average an accumulated u8 attribute, rounded half up. Saturating arithmetic keeps hostile
/// point counts from overflowing the sum.
fn average_u8(sum: u64, count: u64) -> u8 {
    if count == 0 {
        return 0;
    }
    let rounded = sum.saturating_add(count / 2);
    rounded.checked_div(count).unwrap_or(255).min(255) as u8
}

/// Add one cell point's display attributes to its accumulator.
fn add_cell_point(acc: &mut CellAcc, rgb: [u8; 3], intensity: u8) {
    acc.count = acc.count.saturating_add(1);
    for (total, channel) in acc.rgb.iter_mut().zip(rgb) {
        *total = total.saturating_add(u64::from(channel));
    }
    acc.intensity = acc.intensity.saturating_add(u64::from(intensity));
}

/// Write each cell's averaged display attributes onto its real representative.
fn finish_cells(out: &mut [CloudVtx], accs: &[CellAcc]) {
    for acc in accs {
        if let Some(vertex) = out.get_mut(acc.out) {
            vertex.rgb = acc.rgb.map(|sum| average_u8(sum, acc.count));
            vertex.intensity = average_u8(acc.intensity, acc.count);
        }
    }
}

/// One point per `cell`-metre cube. The first point in file order supplies the real position and
/// class; every point in the cell contributes its display RGB and intensity to that representative.
/// `cell` 0 keeps every point.
///
/// The cube is three-dimensional on purpose: an XY-only grid keeps one point per column, so a
/// vertical wall loses every point but one and the walls disappear from the 3D view.
///
/// The classification of the point that was kept travels with it, and so does the point's place
/// in the band: the coarser levels are aggregated from the levels below them and need the band
/// coordinates of the kept points to compute the same cells the band would give.
fn grid(points: &[crate::Pt], cell: f64, origin: [f64; 2], norm: Norm) -> (Vec<CloudVtx>, Vec<u8>, Vec<usize>) {
    let held = points.len().min(4_000_000);
    let mut out = Vec::with_capacity(held);
    let mut classes = Vec::with_capacity(held);
    let mut indices = Vec::with_capacity(held);
    let mut vtx = |p: &crate::Pt| CloudVtx {
        x: (p.x - origin[0]) as f32,
        y: (p.y - origin[1]) as f32,
        z: p.z as f32,
        rgb: p.rgb.map(|rgb| rgb.map(|channel| channel as u8)).unwrap_or([220, 220, 220]),
        intensity: norm.u8(p.intensity),
    };
    if cell <= 0.0 || !cell.is_finite() {
        indices.extend(0..points.len());
        out.extend(points.iter().map(&mut vtx));
        classes.extend(points.iter().map(|p| p.class));
        return (out, classes, indices);
    }
    let inv = 1.0 / cell;
    // The map holds one key per cell kept, so it grows with the aggregation result rather than the
    // points it walks: a 4-million-point band on a 1 cm grid rarely keeps a million cells.
    let mut cells: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut accs: Vec<CellAcc> = Vec::with_capacity(held);
    for (at, p) in points.iter().enumerate() {
        let key = band_cell(p, origin, inv);
        let first = vtx(p);
        if let Some(&acc_at) = cells.get(&key) {
            if let Some(acc) = accs.get_mut(acc_at) {
                add_cell_point(acc, first.rgb, first.intensity);
            }
            continue;
        }
        cells.insert(key, accs.len());
        accs.push(CellAcc { out: out.len(), count: 1, rgb: first.rgb.map(u64::from), intensity: u64::from(first.intensity) });
        out.push(first);
        classes.push(p.class);
        indices.push(at);
    }
    finish_cells(&mut out, &accs);
    (out, classes, indices)
}

/// The `cell`-metre cube a band point falls in. The keys are computed from the band coordinates,
/// so a point gets the same cube whichever level is asking.
fn band_cell(p: &crate::Pt, origin: [f64; 2], inv: f64) -> (i64, i64, i64) {
    (((p.x - origin[0]) * inv).floor() as i64, ((p.y - origin[1]) * inv).floor() as i64, (p.z * inv).floor() as i64)
}

/// One coarser level, aggregated from the level above it: walk the level's representatives in
/// their band order, keep the first real position of every `cell`-metre cube, and average the
/// display attributes of the representatives that fall in it.
///
/// The keys come from the band coordinates the level's points came from (the indices walk the
/// band), so the cubes are the same the band itself would give. The cell must be a whole multiple
/// of the cell below (the caller doubles it every step: a fine cube then never straddles a coarse
/// boundary), so the band's first point of a coarse cube also survives the level below: any
/// earlier band point in its own fine cube would sit in the same coarse cube and contradict its
/// firstness. Walking that level in band order then keeps the first of its points per coarse
/// cube, which is the first of the band's points in it, exactly what decimating the band again
/// gives.
///
/// The classification the level above carries travels with the representative, and the
/// representative's place in the band travels too, so the next step can do the same.
fn grid_below(
    band: &[crate::Pt],
    indices: &[usize],
    pts: &[CloudVtx],
    classes: &[u8],
    cell: f64,
    origin: [f64; 2],
) -> (Vec<CloudVtx>, Vec<u8>, Vec<usize>) {
    let held = (indices.len() / 2).max(64);
    let mut out = Vec::with_capacity(held);
    let mut out_classes = Vec::with_capacity(held);
    let mut kept = Vec::with_capacity(held);
    let inv = 1.0 / cell;
    // The map holds one key per cell kept, so it grows with the aggregation result rather than the
    // level it walks (see `grid`).
    let mut cells: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut accs: Vec<CellAcc> = Vec::with_capacity(held);
    for (i, &at) in indices.iter().enumerate() {
        let (Some(point), Some(vertex)) = (band.get(at), pts.get(i)) else { continue };
        let key = band_cell(point, origin, inv);
        if let Some(&acc_at) = cells.get(&key) {
            if let Some(acc) = accs.get_mut(acc_at) {
                add_cell_point(acc, vertex.rgb, vertex.intensity);
            }
            continue;
        }
        cells.insert(key, accs.len());
        accs.push(CellAcc { out: out.len(), count: 1, rgb: vertex.rgb.map(u64::from), intensity: u64::from(vertex.intensity) });
        out.push(*vertex);
        if let Some(class) = classes.get(i) {
            out_classes.push(*class);
        }
        kept.push(at);
    }
    finish_cells(&mut out, &accs);
    (out, out_classes, kept)
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
    fn aggregates_cell_attributes_onto_first_real_point_deterministically() {
        let points = vec![
            crate::Pt { x: 0.1, y: 0.2, z: 0.3, rgb: Some([10, 20, 30]), intensity: 0, class: 2 },
            crate::Pt { x: 0.4, y: 0.5, z: 0.6, rgb: Some([30, 40, 50]), intensity: 200, class: 9 },
        ];
        let build = || Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, Some(1.0), slice_of(points.clone()), 1000, CloudColor::Rgb);
        let first = build();
        let second = build();

        assert_eq!(first.levels[0].pts.len(), 1, "both source points share one grid cell");
        let p = &first.levels[0].pts[0];
        assert_eq!((p.x, p.y, p.z), (0.0, 0.0, 0.3), "the representative keeps the first real point's coordinates");
        assert_eq!(p.rgb, [20, 30, 40], "RGB is averaged over both points in the cell");
        assert_eq!(p.intensity, 128, "normalized intensities 0 and 255 average to 128");
        assert_eq!(first.levels[0].classes, [2], "the first point's class is retained");

        let signature = |cloud: &Cloud| {
            cloud
                .levels
                .iter()
                .map(|l| (l.cell, l.pts.iter().map(|p| (p.x, p.y, p.z, p.rgb, p.intensity)).collect::<Vec<_>>(), l.classes.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(signature(&first), signature(&second), "repeated builds are deterministic");
    }

    #[test]
    fn coarse_levels_aggregate_immediately_finer_cells() {
        let points = vec![
            crate::Pt { x: 0.1, y: 0.1, z: 0.5, rgb: Some([10, 20, 30]), intensity: 0, class: 2 },
            crate::Pt { x: 0.2, y: 0.1, z: 0.5, rgb: Some([30, 40, 50]), intensity: 200, class: 9 },
            crate::Pt { x: 1.1, y: 0.1, z: 0.5, rgb: Some([70, 80, 90]), intensity: 100, class: 4 },
        ];
        let cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, Some(1.0), slice_of(points), 1000, CloudColor::Rgb);

        assert_eq!(cloud.levels[0].pts.len(), 2, "the 1 m level keeps two real representatives");
        assert_eq!(cloud.levels[0].pts[0].rgb, [20, 30, 40]);
        assert_eq!(cloud.levels[0].pts[0].intensity, 128);
        assert_eq!(cloud.levels[0].pts[1].rgb, [70, 80, 90]);
        assert_eq!(cloud.levels[0].pts[1].intensity, 128);

        assert_eq!(cloud.levels[1].pts.len(), 1, "both fine cells collapse into the 2 m cell");
        let coarse = &cloud.levels[1].pts[0];
        assert_eq!((coarse.x, coarse.y, coarse.z), (0.0, 0.0, 0.5), "the coarse representative is a real source point");
        assert_eq!(coarse.rgb, [45, 55, 65], "coarse RGB averages the two fine representatives");
        assert_eq!(coarse.intensity, 128, "coarse intensity averages the fine representatives");
        assert_eq!(cloud.levels[1].classes, [2], "the coarse class is the first representative's class");
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

    /// The pyramid is built from the level below, not by decimating the band again: the kept
    /// points must still be the ones the band itself would give at the same cells, whatever the
    /// shape or the values of the point cloud.
    #[test]
    fn the_pyramid_from_the_level_below_holds_the_same_points() {
        // A 3 x 2 m floor on a 1 cm grid, the shape the lighter-cells test uses, so the pyramid
        // really builds several steps.
        let band: Vec<_> = (0..300).flat_map(|i| (0..200).map(move |j| pt(f64::from(i) * 0.01, f64::from(j) * 0.01, 1.0))).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.01), slice_of(band.clone()), 100_000, CloudColor::Rgb);
        let norm = Norm { lo: c.imin, span: u32::from(c.imax.saturating_sub(c.imin)) };
        assert!(c.levels.len() >= 3, "the band builds several levels: {}", c.levels.len());
        for level in &c.levels {
            let (reference, reference_classes, _) = grid(&band, level.cell, c.origin, norm);
            let sig = |pts: &[CloudVtx], classes: &[u8]| -> Vec<(f32, f32, f32, [u8; 3], u8, u8)> {
                let mut v: Vec<_> = pts.iter().zip(classes).map(|(p, cl)| (p.x, p.y, p.z, p.rgb, p.intensity, *cl)).collect();
                // Floats have no total order; the band's coordinates are positive, so the bit
                // patterns sort like the values they hold.
                v.sort_unstable_by_key(|p| (p.0.to_bits(), p.1.to_bits(), p.2.to_bits(), p.3, p.4, p.5));
                v
            };
            assert_eq!(
                sig(&level.pts, &level.classes),
                sig(&reference, &reference_classes),
                "the level on a {:.5} m grid is decimated again, not picked up",
                level.cell
            );
        }
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
    fn a_zoom_hovering_at_a_boundary_keeps_its_level() {
        let pts: Vec<_> = (0..4000).map(|i| pt(f64::from(i % 100) * 0.01, f64::from(i / 100) * 0.01, 1.0)).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.01), slice_of(pts), 100_000, CloudColor::Rgb);
        let fine = c.levels[0].cell;
        let spacing = if fine > 0.0 { fine } else { 0.0002 };
        // 1.41 pixels is the spacing at which the nearest-level choice sits exactly between the
        // fine level and the one above it: the point where a wobble would start flipping.
        let hover = 1.41 / spacing;
        assert_eq!(
            c.level_for_sticky(hover, Some(fine)).cell.to_bits(),
            fine.to_bits(),
            "a zoom that hovers at the boundary must keep the level under the cursor"
        );
        // A real zoom out still moves on.
        assert_ne!(c.level_for_sticky(2.0, Some(fine)).cell.to_bits(), fine.to_bits(), "a real zoom must still change level");
        // With nothing drawn yet, the answer is the plain nearest level.
        assert_eq!(c.level_for_sticky(hover, None).cell.to_bits(), c.level_for(hover).cell.to_bits());
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

    /// The class colours are drawn from `level.classes`, and the classifiers rewrite those bytes
    /// in place without moving a point. The drawing caches a plan by the cloud's state, so a
    /// rewrite has to change that state — otherwise the user re-classifies a scan and goes on
    /// seeing the colours of the classification before it.
    #[test]
    fn every_classifier_that_rewrites_the_classes_moves_the_revision() {
        let points: Vec<_> = (0..40).map(|i| pt(f64::from(i % 8) * 0.2, f64::from(i / 8) * 0.2, f64::from(i % 3) * 0.5)).collect();
        let mut c = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(points), 100_000, CloudColor::Class);
        let before = c.class_revision;
        c.classify_ground(&crate::ground::GroundOptions::default());
        assert_ne!(c.class_revision, before, "the ground filter rewrote the classes");
        let before = c.class_revision;
        c.classify(&crate::classify::ClassifyOptions::default());
        assert_ne!(c.class_revision, before, "the classifier rewrote the classes");
        let before = c.class_revision;
        c.apply_clusters();
        assert_ne!(c.class_revision, before, "naming the groups rewrites the classes");
    }

    #[test]
    fn rgb_shading_is_neutral_by_default() {
        let cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(vec![pt(0.0, 0.0, 0.0)]), 1000, CloudColor::Rgb);
        assert_eq!(cloud.rgb_exposure, 1.0);
        assert_eq!(cloud.rgb_contrast, 1.0);
        assert_eq!(cloud.color_of(&cloud.levels[0].pts[0], None, None), [10, 20, 30]);
    }

    #[test]
    fn rgb_exposure_doubles_channels_and_clamps_at_white() {
        let mut cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(vec![pt(0.0, 0.0, 0.0)]), 1000, CloudColor::Rgb);
        cloud.rgb_exposure = 2.0;
        let vertex = CloudVtx { x: 0.0, y: 0.0, z: 0.0, rgb: [10, 128, 200], intensity: 0 };
        assert_eq!(cloud.color_of(&vertex, None, None), [20, 255, 255]);
    }

    #[test]
    fn rgb_contrast_pivots_around_middle_grey() {
        let mut cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(vec![pt(0.0, 0.0, 0.0)]), 1000, CloudColor::Rgb);
        cloud.rgb_contrast = 2.0;
        let vertex = CloudVtx { x: 0.0, y: 0.0, z: 0.0, rgb: [32, 128, 224], intensity: 0 };
        assert_eq!(cloud.color_of(&vertex, None, None), [0, 129, 255]);
    }

    /// A file that carries no colour gets the placeholder grey on every point, so the luminance
    /// window has no width at all. Stretching a flat field cannot reveal anything, and maximum
    /// contrast on it pushes every channel past white: Auto is meant to make a photo readable, not
    /// to burn it out.
    #[test]
    fn auto_rgb_does_not_burn_out_a_cloud_with_no_colour_spread() {
        let points: Vec<_> = (0..40).map(|i| crate::Pt { x: f64::from(i) * 0.01, y: 0.0, z: 0.0, rgb: None, intensity: 0, class: 1 }).collect();
        let mut cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(points), 1000, CloudColor::Rgb);
        assert!(cloud.levels[0].pts.iter().all(|v| v.rgb == [220, 220, 220]), "the placeholder a colourless file leaves");
        let (exposure, contrast) = cloud.auto_rgb();
        cloud.rgb_exposure = exposure;
        cloud.rgb_contrast = contrast;
        let colours: Vec<[u8; 3]> = cloud.levels[0].pts.iter().map(|v| cloud.color_of(v, None, None)).collect();
        assert!(colours.iter().all(|c| *c != [255, 255, 255]), "Auto leaves a flat field off white: {:?}", colours[0]);
        assert_eq!(contrast, 1.0, "there is no spread to stretch");
    }

    #[test]
    fn auto_rgb_makes_a_dark_low_variance_cloud_more_readable() {
        let points: Vec<_> = (0..101)
            .map(|i| crate::Pt { rgb: Some([115 + (i % 11), 115 + (i % 11), 115 + (i % 11)]), ..pt(f64::from(i) * 0.01, 0.0, 0.0) })
            .collect();
        let mut cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(points), 1000, CloudColor::Rgb);
        let before: Vec<u32> = cloud.levels[0].pts.iter().map(|v| u32::from(v.rgb[0])).collect();
        let (exposure, contrast) = cloud.auto_rgb();
        cloud.rgb_exposure = exposure;
        cloud.rgb_contrast = contrast;
        let after: Vec<u32> = cloud.levels[0].pts.iter().map(|v| u32::from(cloud.color_of(v, None, None)[0])).collect();
        assert!((0.25..=4.0).contains(&exposure) && exposure > 1.0, "bounded exposure: {exposure}");
        assert!((0.25..=4.0).contains(&contrast) && contrast > 1.0, "bounded contrast: {contrast}");
        assert!(after.iter().sum::<u32>() > before.iter().sum::<u32>(), "Auto lifts the dark values: {before:?} -> {after:?}");
        assert!(after.iter().min() < before.iter().min() && after.iter().max() > before.iter().max(), "Auto expands the narrow range");
    }

    #[test]
    fn mixed_scale_rgb_uses_one_set_wide_decision() {
        let mut points = vec![
            crate::Pt { rgb: Some([65535, 128, 257]), ..pt(0.0, 0.0, 0.0) },
            crate::Pt { rgb: Some([257, 257, 0]), ..pt(1.0, 0.0, 0.0) },
            crate::Pt { rgb: Some([128, 128, 128]), ..pt(2.0, 0.0, 0.0) },
        ];
        crate::normalize_rgb(&mut points);
        assert_eq!(points[0].rgb, Some([255, 0, 1]));
        assert_eq!(points[1].rgb, Some([1, 1, 0]));
        assert_eq!(points[2].rgb, Some([0, 0, 0]));
    }

    #[test]
    fn mixed_scale_cloud_rgb_matches_set_wide_conversion() {
        let points = vec![
            crate::Pt { rgb: Some([65535, 128, 257]), ..pt(0.0, 0.0, 0.0) },
            crate::Pt { rgb: Some([257, 257, 0]), ..pt(1.0, 0.0, 0.0) },
            crate::Pt { rgb: Some([128, 128, 128]), ..pt(2.0, 0.0, 0.0) },
        ];
        let cloud = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(points), 1000, CloudColor::Rgb);
        let colors: Vec<[u8; 3]> = cloud.levels[0].pts.iter().map(|p| p.rgb).collect();
        assert_eq!(colors, vec![[255, 0, 1], [1, 1, 0], [0, 0, 0]]);
    }

    #[test]
    fn eight_bit_only_rgb_is_copied_exactly() {
        let mut points = vec![crate::Pt { rgb: Some([255, 255, 0]), ..pt(0.0, 0.0, 0.0) }];
        crate::normalize_rgb(&mut points);
        assert_eq!(points[0].rgb, Some([255, 255, 0]));
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

    /// A scan whose interesting wall sits in one tenth of its height reads as one flat colour on
    /// the auto window: the user names the two endpoints of the ramp instead, and the ramp is the
    /// window they named. `None` is Auto — the band's own 2%-98% window.
    #[test]
    fn a_manual_elevation_window_overrides_the_auto_one() {
        let pts: Vec<_> = (0..101).map(|i| pt(f64::from(i % 10) * 0.01, f64::from(i / 10) * 0.01, f64::from(i) * 0.01)).collect();
        let mut c = Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, Some(0.01), slice_of(pts), 1_000_000, CloudColor::Elevation);
        assert_eq!(c.elevation_window(), (c.zp02, c.zp98), "with no manual endpoints the ramp reads the auto window");
        // Ten points on a wall 1 m high, in a band that spans ten times that: the wall is the one
        // thing worth reading, so the ramp is pulled onto it.
        c.z_window = Some((1.0, 2.0));
        assert_eq!(c.elevation_window(), (1.0, 2.0), "the manual endpoints are the ramp");
        let grey = |t: f32| {
            let s = crate::spectrum::Spectrum::Grey;
            s.apply(t)
        };
        c.spectrum = crate::spectrum::Spectrum::Grey;
        let colour_at = |z: f64| c.color_of(&CloudVtx { x: 0.0, y: 0.0, z: z as f32, rgb: [0, 0, 0], intensity: 0 }, None, None);
        assert_eq!(colour_at(1.0), grey(0.0), "the window's floor is the ramp's first colour");
        assert_eq!(colour_at(1.5), grey(0.5), "and its middle the ramp's middle");
        assert_eq!(colour_at(2.0), grey(1.0), "its ceiling the ramp's last");
        // The auto window put the first colour near the bottom of the whole band, not at 1 m.
        c.z_window = None;
        assert_eq!(c.elevation_window(), (c.zp02, c.zp98), "clearing them returns to Auto");
    }

    /// The same lever for the returns, on the 0..255 scale the vertices and the histogram use.
    #[test]
    fn a_manual_intensity_window_overrides_the_auto_one() {
        let pts: Vec<_> = (0..101)
            .map(|i| crate::Pt { x: f64::from(i % 10) * 0.01, y: f64::from(i / 10) * 0.01, z: 0.0, rgb: None, intensity: (i as u16) * 100, class: 1 })
            .collect();
        let mut c = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, Some(0.01), slice_of(pts), 1_000_000, CloudColor::Intensity);
        assert_eq!(c.intensity_window(), (c.ip02, c.ip98), "with no manual endpoints the ramp reads the auto window");
        // The band's returns run to 10 000, which the vertices stretch over the whole 0..255 scale;
        // the wall the user cares about sits between two of them.
        c.i_window = Some((51, 203));
        assert_eq!(c.intensity_window(), (51, 203), "the manual endpoints are the ramp");
        c.spectrum = crate::spectrum::Spectrum::Grey;
        let grey = |t: f32| crate::spectrum::Spectrum::Grey.apply(t);
        let colour_at = |i: u8| c.color_of(&CloudVtx { x: 0.0, y: 0.0, z: 0.0, rgb: [0, 0, 0], intensity: i }, None, None);
        assert_eq!(colour_at(51), grey(0.0), "the window's floor is the ramp's first colour");
        assert_eq!(colour_at(127), grey(0.5), "its middle is (51 + 203) / 2");
        assert_eq!(colour_at(203), grey(1.0), "and its ceiling the last");
        assert_eq!(colour_at(255), grey(1.0), "a hotter return is clamped, not given a colour past the ramp");
        assert_eq!(colour_at(0), grey(0.0), "and a darker one the first colour");
        c.i_window = None;
        assert_eq!(c.intensity_window(), (c.ip02, c.ip98), "clearing them returns to Auto");
    }

    /// The bins the palette draws are the band's own points, not the level the screen happens to
    /// show: a histogram that counted only the uploaded points would change shape as the user
    /// zooms, and read as though the scan held fewer bright returns than it does.
    #[test]
    fn the_intensity_histogram_counts_every_band_point() {
        let pts: Vec<_> = (0..400)
            .map(|i| crate::Pt { x: f64::from(i % 20) * 0.01, y: f64::from(i / 20) * 0.01, z: 0.0, rgb: None, intensity: (i as u16) * 10, class: 1 })
            .collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, Some(0.05), slice_of(pts), 1_000_000, CloudColor::Intensity);
        let uploaded = c.points();
        assert!(uploaded < 400, "the grid really did thin the band: {uploaded} of 400");
        let total: u64 = c.ihisto.iter().map(|b| u64::from(*b)).sum();
        assert_eq!(total, 400, "every band point lands in exactly one of the 64 bins");
    }

    /// A point is counted in the bin its own stretched value falls in, so the two bars of the
    /// histogram's ends are the band's darkest and brightest returns.
    #[test]
    fn every_level_point_is_binned_where_the_ramp_reads_it() {
        let pts: Vec<_> =
            (0..20).map(|i| crate::Pt { x: f64::from(i) * 0.01, y: 0.0, z: 0.0, rgb: None, intensity: (i as u16) * 3000, class: 1 }).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice_of(pts), 1_000_000, CloudColor::Intensity);
        for v in &c.levels[0].pts {
            let bin = c.bin_of_intensity(v.intensity);
            assert!(bin < 64, "bin {bin} of a u8 {i}", i = v.intensity);
            assert!(c.ihisto[bin] > 0, "the point's own bin carries it");
        }
        assert_eq!(c.bin_of_intensity(0), 0, "the darkest return is the histogram's first bin");
        assert_eq!(c.bin_of_intensity(255), 63, "and the brightest its last");
    }

    /// A band of one height has no Z window to bin over, but its returns still have to be drawn:
    /// the intensity histogram is a 0..255 scale, so it never depends on the band's height.
    #[test]
    fn a_zero_height_band_keeps_its_intensity_counts() {
        let pts: Vec<_> =
            (0..50).map(|i| crate::Pt { x: f64::from(i) * 0.01, y: 0.0, z: 1.0, rgb: None, intensity: (i as u16) * 100, class: 1 }).collect();
        let c = Cloud::from_slice("t".into(), String::new(), 1.0, 1.0001, None, slice_of(pts), 1_000_000, CloudColor::Intensity);
        assert_eq!(c.zmin, c.zmax, "a flat band: no height to bin over");
        assert_eq!(c.zhisto.iter().sum::<u32>(), 0, "the height profile is honestly empty");
        assert_eq!(c.ihisto.iter().sum::<u32>(), 50, "the returns are still counted");
    }
}
