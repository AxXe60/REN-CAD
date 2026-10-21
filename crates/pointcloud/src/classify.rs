//! Classifying a cloud: what is the ground, the vegetation, the walls, the noise.
//!
//! The LAS files of a survey often come without classification, or with a classification the
//! scanner guessed. The rules here are the ones the practice uses, and every one of them can be
//! explained in a report:
//!
//! * **the height above the ground** separates the vegetation in three bands — the low one under
//!   half a metre, the middle one under two, the high one under five; above five metres a scan of
//!   a yard or a courtyard is a building;
//! * **the shape of the surface** separates the walls from the clutter: a surface that stands up
//!   (its normal is horizontal) above the vegetation is a facade, a flat one is a roof or a floor;
//! * **the roughness** separates the leaves from the hard surfaces, and the lonely points from
//!   the real ones.
//!
//! The features are read on a sample of the points — a few tens of thousands — and given to the
//! whole level through the nearest neighbour, so a four-million-point scan classifies in seconds.

use std::collections::HashMap;

use crate::grid::Grid;

/// The LAS classification codes the fork writes (the ASPRS standard).
pub const CLASS_UNCLASSIFIED: u8 = 1;
pub const CLASS_GROUND: u8 = 2;
pub const CLASS_LOW_VEG: u8 = 3;
pub const CLASS_MED_VEG: u8 = 4;
pub const CLASS_HIGH_VEG: u8 = 5;
pub const CLASS_BUILDING: u8 = 6;
pub const CLASS_LOW_POINT: u8 = 7;
pub const CLASS_MODEL_KEY: u8 = 8;
pub const CLASS_WATER: u8 = 9;
pub const CLASS_ROAD: u8 = 11;
pub const CLASS_WIRE: u8 = 12;
pub const CLASS_NOISE: u8 = 18;

/// The name of a class, the way a report writes it.
pub fn class_name(code: u8) -> &'static str {
    match code {
        CLASS_UNCLASSIFIED => "unclassified",
        CLASS_GROUND => "ground",
        CLASS_LOW_VEG => "low vegetation",
        CLASS_MED_VEG => "medium vegetation",
        CLASS_HIGH_VEG => "high vegetation",
        CLASS_BUILDING => "building",
        CLASS_LOW_POINT => "low point",
        CLASS_MODEL_KEY => "mass point",
        CLASS_WATER => "water",
        CLASS_ROAD => "road surface",
        CLASS_WIRE => "wire",
        CLASS_NOISE => "noise",
        _ => "other",
    }
}

/// The class a name stands for.
pub fn class_from_name(name: &str) -> Option<u8> {
    let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    match key.as_str() {
        "unclassified" | "none" => Some(CLASS_UNCLASSIFIED),
        "ground" | "terrain" | "earth" => Some(CLASS_GROUND),
        "lowvegetation" | "grass" | "lowveg" => Some(CLASS_LOW_VEG),
        "mediumvegetation" | "mediumveg" | "bush" | "shrub" => Some(CLASS_MED_VEG),
        "highvegetation" | "highveg" | "tree" | "trees" => Some(CLASS_HIGH_VEG),
        "building" | "structure" | "wall" | "walls" | "facade" => Some(CLASS_BUILDING),
        "lowpoint" | "lowpoints" => Some(CLASS_LOW_POINT),
        "masspoint" | "modelkeypoint" | "keypoint" => Some(CLASS_MODEL_KEY),
        "water" => Some(CLASS_WATER),
        "roadsurface" | "road" => Some(CLASS_ROAD),
        "wire" | "cable" => Some(CLASS_WIRE),
        "noise" | "highnoise" => Some(CLASS_NOISE),
        _ => None,
    }
}

/// Every class the classifier can write, in the order a report lists them.
pub const ALL_CLASSES: [u8; 12] = [
    CLASS_UNCLASSIFIED,
    CLASS_GROUND,
    CLASS_LOW_VEG,
    CLASS_MED_VEG,
    CLASS_HIGH_VEG,
    CLASS_BUILDING,
    CLASS_LOW_POINT,
    CLASS_MODEL_KEY,
    CLASS_WATER,
    CLASS_ROAD,
    CLASS_WIRE,
    CLASS_NOISE,
];

/// What the classifier looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassifyMode {
    /// The height bands and the shape: the full pipeline.
    Auto,
    /// The height above the ground only.
    Height,
    /// The shape of the surface only: the walls and the flat surfaces.
    Shape,
}

impl ClassifyMode {
    /// The mode a name stands for.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "auto" | "full" | "both" => Some(ClassifyMode::Auto),
            "height" | "heightaboveground" | "hag" => Some(ClassifyMode::Height),
            "shape" | "geometry" | "normals" => Some(ClassifyMode::Shape),
            _ => None,
        }
    }

    /// The name of the mode, the way the command writes it.
    pub fn name(self) -> &'static str {
        match self {
            ClassifyMode::Auto => "auto",
            ClassifyMode::Height => "height",
            ClassifyMode::Shape => "shape",
        }
    }
}

/// The colour of a class, the way the LAS viewers paint them: the ground brown, the vegetation
/// green, the buildings red, the noise violet.
pub fn class_color(code: u8) -> [u8; 3] {
    match code {
        CLASS_GROUND => [160, 120, 80],
        CLASS_LOW_VEG => [150, 220, 120],
        CLASS_MED_VEG => [80, 180, 70],
        CLASS_HIGH_VEG => [30, 110, 40],
        CLASS_BUILDING => [220, 60, 50],
        CLASS_LOW_POINT => [60, 90, 220],
        CLASS_MODEL_KEY => [220, 80, 220],
        CLASS_WATER => [60, 200, 220],
        CLASS_ROAD => [110, 110, 110],
        CLASS_WIRE => [240, 230, 60],
        CLASS_NOISE => [140, 80, 200],
        _ => [170, 170, 170],
    }
}

/// How the classification is read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassifyOptions {
    /// The size of the cells the ground surface is built on, in metres.
    pub cell: f64,
    /// The radius of the neighbourhood the shape is read from, in metres.
    pub radius: f64,
    /// The most points read on one level: the features are computed on a sample of this size.
    pub sample: usize,
    /// The bands of the vegetation, in metres: the low one ends here, the middle one, the high one.
    pub bands: [f64; 3],
    /// What the classifier looks at.
    pub mode: ClassifyMode,
}

impl Default for ClassifyOptions {
    fn default() -> Self {
        // Half-metre cells, a 30 cm neighbourhood, a sample of 60k points, and the bands of the
        // practice: 50 cm, 2 m and 5 m above the ground.
        Self { cell: 0.5, radius: 0.3, sample: 60_000, bands: [0.5, 2.0, 5.0], mode: ClassifyMode::Auto }
    }
}

/// What the classifier found.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClassifyResult {
    /// The points that were read.
    pub points: usize,
    /// The count per class, indexed by the class code.
    pub counts: [usize; 19],
}

impl ClassifyResult {
    /// The report line: the classes that were found, with their counts.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for code in ALL_CLASSES {
            let n = self.counts[code as usize];
            if n > 0 {
                parts.push(format!("{} {}", n, class_name(code)));
            }
        }
        if parts.is_empty() {
            return "nothing classified".to_string();
        }
        parts.join(", ")
    }
}

/// The shape of the surface around a point, read from its neighbours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shape {
    /// How much the surface tilts up: 0 is a flat surface, 1 is a wall.
    pub verticality: f32,
    /// How far the neighbours are from the plane of the surface, in metres: 0 is a plane, more is
    /// a bush.
    pub roughness: f32,
    /// How much the heights around the point differ, in metres.
    pub spread: f32,
    /// How many neighbours the point had.
    pub neighbours: usize,
}

/// The normal of the surface around a point: the direction the neighbourhood is thinnest in.
///
/// The covariance of the neighbours is diagonalised with the Jacobi rotation — a few rounds of
/// turning the matrix until the off-diagonal terms vanish — and the smallest eigenvalue's
/// eigenvector is the normal. A flat floor's normal points up; a wall's points sideways.
// The matrix is written on both triangles of the symmetry, which the range loops spell out.
#[allow(clippy::needless_range_loop)]
fn normal_of(cov: [[f64; 3]; 3]) -> [f64; 3] {
    let mut a = cov;
    // The eigenvectors, as the columns of a matrix that starts as the identity.
    let mut v = [[0.0f64; 3]; 3];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..32 {
        // The biggest off-diagonal term: the pair to turn.
        let (mut p, mut q, mut biggest) = (0usize, 1usize, 0.0f64);
        for i in 0..3 {
            for j in (i + 1)..3 {
                if a[i][j].abs() > biggest.abs() {
                    biggest = a[i][j];
                    p = i;
                    q = j;
                }
            }
        }
        if biggest.abs() < 1e-12 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * biggest);
        let t = if theta >= 0.0 { 1.0 / (theta + (1.0 + theta * theta).sqrt()) } else { -1.0 / (-theta + (1.0 + theta * theta).sqrt()) };
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;
        for k in 0..3 {
            if k != p && k != q {
                let akp = a[k][p];
                let akq = a[k][q];
                a[k][p] = c * akp - s * akq;
                a[p][k] = a[k][p];
                a[k][q] = s * akp + c * akq;
                a[q][k] = a[k][q];
            }
        }
        let app = a[p][p];
        let aqq = a[q][q];
        a[p][p] = c * c * app - 2.0 * s * c * biggest + s * s * aqq;
        a[q][q] = s * s * app + 2.0 * s * c * biggest + c * c * aqq;
        a[p][q] = 0.0;
        a[q][p] = 0.0;
        for k in 0..3 {
            let vkp = v[k][p];
            let vkq = v[k][q];
            v[k][p] = c * vkp - s * vkq;
            v[k][q] = s * vkp + c * vkq;
        }
    }
    // The eigenvector of the smallest eigenvalue.
    let mut best = 0usize;
    for k in 1..3 {
        if a[k][k] < a[best][best] {
            best = k;
        }
    }
    [v[0][best], v[1][best], v[2][best]]
}

/// The surface around a point: the tilt and the roughness.
///
/// The normal comes from the smallest direction of the neighbourhood's covariance — the inverse
/// power iteration, a few rounds, which is enough for the surfaces of a scan.
pub fn shape_of(neighbours: &[[f64; 3]], centre: [f64; 3]) -> Shape {
    let mut out = Shape::default();
    if neighbours.len() < 3 {
        out.neighbours = neighbours.len();
        return out;
    }
    out.neighbours = neighbours.len();
    // The covariance of the neighbourhood.
    let mut cov = [[0.0f64; 3]; 3];
    let mut mean = [0.0f64; 3];
    for p in neighbours {
        for (k, m) in mean.iter_mut().enumerate() {
            *m += p[k];
        }
    }
    for m in mean.iter_mut() {
        *m /= neighbours.len() as f64;
    }
    for p in neighbours {
        let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        for (a, row) in cov.iter_mut().enumerate() {
            for (b, value) in row.iter_mut().enumerate() {
                *value += d[a] * d[b];
            }
        }
    }
    // The spread of the heights: a wall's neighbours are at different heights, so this is not the
    // roughness — it is how much the surface climbs around the point.
    let mut var = 0.0;
    for p in neighbours {
        let d = p[2] - centre[2];
        var += d * d;
    }
    out.spread = (var / neighbours.len() as f64).sqrt() as f32;
    // The tilt: the normal of a flat surface points up (|nz| = 1), a wall's points sideways.
    let n = normal_of(cov);
    out.verticality = (1.0 - n[2].abs() as f32).clamp(0.0, 1.0);
    // The roughness: how far the neighbours are from the plane of the surface. The raw spread of
    // the heights would call a wall rough — its neighbours are at different heights — so the
    // distance is taken from the plane, not from the height.
    let mut plane_var = 0.0;
    for p in neighbours {
        let d = (p[0] - mean[0]) * n[0] + (p[1] - mean[1]) * n[1] + (p[2] - mean[2]) * n[2];
        plane_var += d * d;
    }
    out.roughness = (plane_var / neighbours.len() as f64).sqrt() as f32;
    out
}

/// The height above the ground of every point, in metres.
///
/// The ground height of a cell is the median of the points the ground filter found in it; a cell
/// without ground takes the nearest cell that has some.
pub fn heights_above_ground(points: &[[f64; 3]], ground: &[u8], cell: f64) -> Vec<f32> {
    let cell = if cell > 0.0 && cell.is_finite() { cell } else { 0.5 };
    let inv = 1.0 / cell;
    let mut surface: HashMap<(i64, i64), Vec<f64>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        if ground.get(i).copied() != Some(CLASS_GROUND) {
            continue;
        }
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        surface.entry(key).or_default().push(p[2]);
    }
    // The ground height of a cell: the median of its ground points.
    let mut heights: HashMap<(i64, i64), f64> = HashMap::with_capacity(surface.len());
    for (key, zs) in surface {
        let mut zs = zs;
        zs.sort_by(f64::total_cmp);
        let h = zs[zs.len() / 2];
        heights.insert(key, h);
    }
    // The fallback for a cell without ground: the lowest cell of the neighbourhood.
    let fallback = heights.values().copied().fold(f64::INFINITY, f64::min);
    let fallback = if fallback.is_finite() { fallback } else { 0.0 };
    let mut out = Vec::with_capacity(points.len());
    for p in points {
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        let mut ground_z = None;
        for dx in -2i64..=2 {
            for dy in -2i64..=2 {
                if let Some(h) = heights.get(&(key.0 + dx, key.1 + dy)) {
                    ground_z = Some(*h);
                    break;
                }
            }
            if ground_z.is_some() {
                break;
            }
        }
        out.push((p[2] - ground_z.unwrap_or(fallback)) as f32);
    }
    out
}

/// The class a point gets from its height and its shape.
///
/// The rules, in the order they are read:
///
/// * under 20 cm above the ground: the ground itself;
/// * a surface that stands up above the vegetation: a facade, so a building;
/// * by the height bands: the low, the medium and the high vegetation, and above the high band a
///   building;
/// * a point with no neighbours at all: noise.
pub fn class_of(height: f32, shape: &Shape, ground_class: u8, options: &ClassifyOptions) -> u8 {
    if ground_class == CLASS_NOISE {
        return CLASS_NOISE;
    }
    if ground_class == CLASS_GROUND {
        return CLASS_GROUND;
    }
    if shape.neighbours == 0 {
        return CLASS_NOISE;
    }
    let h = f64::from(height);
    // A smooth surface above the vegetation is a structure: a facade that stands up, a ceiling or
    // a roof that lies flat. The roughness is what tells them from the trees, whose leaves are
    // never smooth.
    if options.mode != ClassifyMode::Height
        && h > options.bands[1]
        && shape.roughness < 0.08
        && (shape.verticality > 0.65 || shape.verticality < 0.35)
    {
        return CLASS_BUILDING;
    }
    if h < 0.2 {
        return CLASS_GROUND;
    }
    if options.mode == ClassifyMode::Shape {
        // Only the shape was asked for: the walls are named, the rest stays unclassified.
        return CLASS_UNCLASSIFIED;
    }
    if h < options.bands[0] {
        return CLASS_LOW_VEG;
    }
    if h < options.bands[1] {
        return CLASS_MED_VEG;
    }
    if h < options.bands[2] {
        return CLASS_HIGH_VEG;
    }
    CLASS_BUILDING
}

/// A sample of a level and the features read on it.
///
/// The features of a scan are too many to read on every point: the sample is a few tens of
/// thousands, spread over the level, and the rest of the level takes the word of its nearest
/// sampled point.
pub struct Sample {
    /// The points of the level the sample was taken from.
    pub indices: Vec<usize>,
    /// The sampled points.
    pub points: Vec<[f64; 3]>,
    /// The height above the ground of every sampled point.
    pub heights: Vec<f32>,
    /// The shape of the surface around every sampled point.
    pub shapes: Vec<Shape>,
}

impl Sample {
    /// The number of sampled points.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether the sample is empty.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Read the features of a sample of a level: the height above the ground and the shape.
pub fn sample_level(points: &[[f64; 3]], ground: &[u8], options: &ClassifyOptions) -> Sample {
    let heights = heights_above_ground(points, ground, options.cell);
    let step = (points.len() / options.sample.max(1)).max(1);
    let indices: Vec<usize> = (0..points.len()).step_by(step).collect();
    let sampled: Vec<[f64; 3]> = indices.iter().map(|&i| points[i]).collect();
    let grid = Grid::new(&sampled, options.radius.max(options.cell * 0.5));
    let mut near = Vec::new();
    let mut shapes = Vec::with_capacity(sampled.len());
    for p in sampled.iter() {
        grid.around(*p, &mut near);
        let r2 = options.radius * options.radius;
        let mut neighbours: Vec<[f64; 3]> = Vec::with_capacity(near.len());
        for &j in near.iter() {
            let q = sampled[j as usize];
            let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2);
            if d <= r2 {
                neighbours.push(q);
            }
        }
        shapes.push(shape_of(&neighbours, *p));
    }
    let sample_heights = indices.iter().map(|&i| heights[i]).collect();
    Sample { indices, points: sampled, heights: sample_heights, shapes }
}

/// Classify the points of one level.
///
/// `ground` holds the class the ground filter gave every point; the classifier reads the ground
/// from it and rewrites the rest.
pub fn classify_level(points: &[[f64; 3]], ground: &[u8], options: &ClassifyOptions) -> (Vec<u8>, ClassifyResult) {
    let mut result = ClassifyResult { points: points.len(), counts: [0; 19] };
    if points.is_empty() {
        return (Vec::new(), result);
    }
    let sample = sample_level(points, ground, options);
    let mut sample_classes = Vec::with_capacity(sample.len());
    for (n, &i) in sample.indices.iter().enumerate() {
        let class = class_of(sample.heights[n], &sample.shapes[n], ground.get(i).copied().unwrap_or(CLASS_UNCLASSIFIED), options);
        sample_classes.push(class);
    }
    // The whole level takes the class of its nearest sampled point.
    let class_grid = Grid::new(&sample.points, options.radius.max(options.cell * 0.5));
    let mut out = vec![CLASS_UNCLASSIFIED; points.len()];
    let mut near = Vec::new();
    for (i, p) in points.iter().enumerate() {
        class_grid.around(*p, &mut near);
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
            out[i] = sample_classes[best_at];
        } else {
            // No sample nearby: the point keeps the ground filter's word.
            out[i] = ground.get(i).copied().unwrap_or(CLASS_UNCLASSIFIED);
        }
    }
    for c in out.iter() {
        result.counts[*c as usize] += 1;
    }
    (out, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_names_are_read_the_way_they_are_written() {
        assert_eq!(class_from_name("Building"), Some(CLASS_BUILDING));
        assert_eq!(class_from_name("high vegetation"), Some(CLASS_HIGH_VEG));
        assert_eq!(class_from_name("facade"), Some(CLASS_BUILDING));
        assert_eq!(class_from_name("nope"), None);
        assert_eq!(class_name(CLASS_BUILDING), "building");
    }

    #[test]
    fn a_flat_floor_is_a_ground_and_a_wall_is_a_building() {
        // A floor: a patch of points at the same height.
        let floor: Vec<[f64; 3]> = (0..10).flat_map(|i| (0..10).map(move |j| [i as f64 * 0.1, j as f64 * 0.1, 0.0])).collect();
        let shape = shape_of(&floor, [0.5, 0.5, 0.0]);
        assert!(shape.verticality < 0.2, "a floor does not stand up: {shape:?}");
        assert!(shape.roughness < 0.01, "a floor is smooth: {shape:?}");
        // A wall: a patch stood up, spread in X and Z with the Y fixed.
        let wall: Vec<[f64; 3]> = (0..10).flat_map(|i| (0..10).map(move |k| [i as f64 * 0.1, 0.0, k as f64 * 0.1])).collect();
        let shape = shape_of(&wall, [0.5, 0.0, 0.5]);
        assert!(shape.verticality > 0.6, "a wall stands up: {shape:?}");
    }

    #[test]
    fn the_vegetation_bands_follow_the_height() {
        let options = ClassifyOptions::default();
        // A rough surface: the leaves of a tree, never a plane.
        let rough = Shape { verticality: 0.5, roughness: 0.3, spread: 0.3, neighbours: 10 };
        assert_eq!(class_of(0.1, &rough, CLASS_UNCLASSIFIED, &options), CLASS_GROUND);
        assert_eq!(class_of(0.3, &rough, CLASS_UNCLASSIFIED, &options), CLASS_LOW_VEG);
        assert_eq!(class_of(1.0, &rough, CLASS_UNCLASSIFIED, &options), CLASS_MED_VEG);
        assert_eq!(class_of(3.0, &rough, CLASS_UNCLASSIFIED, &options), CLASS_HIGH_VEG);
        assert_eq!(class_of(6.0, &rough, CLASS_UNCLASSIFIED, &options), CLASS_BUILDING);
    }

    #[test]
    fn a_smooth_surface_above_the_vegetation_is_a_structure() {
        let options = ClassifyOptions::default();
        // A flat surface: a ceiling.
        let flat = Shape { verticality: 0.1, roughness: 0.01, spread: 0.01, neighbours: 10 };
        assert_eq!(class_of(3.0, &flat, CLASS_UNCLASSIFIED, &options), CLASS_BUILDING);
        // A wall: a facade.
        let wall = Shape { verticality: 0.9, roughness: 0.01, spread: 0.01, neighbours: 10 };
        assert_eq!(class_of(3.0, &wall, CLASS_UNCLASSIFIED, &options), CLASS_BUILDING);
        // A low wall is a trunk, not a facade.
        assert_eq!(class_of(0.4, &wall, CLASS_UNCLASSIFIED, &options), CLASS_LOW_VEG);
        // A rough surface above the vegetation stays vegetation: the leaves of a tree.
        let rough = Shape { verticality: 0.5, roughness: 0.3, spread: 0.3, neighbours: 10 };
        assert_eq!(class_of(3.0, &rough, CLASS_UNCLASSIFIED, &options), CLASS_HIGH_VEG);
    }

    #[test]
    fn a_lonely_point_is_noise() {
        let options = ClassifyOptions::default();
        let alone = Shape::default();
        assert_eq!(class_of(1.0, &alone, CLASS_UNCLASSIFIED, &options), CLASS_NOISE);
    }

    #[test]
    fn a_level_is_classified_and_counted() {
        // A floor with a wall standing on it.
        let mut points: Vec<[f64; 3]> = Vec::new();
        for i in 0..50 {
            for j in 0..50 {
                points.push([i as f64 * 0.1, j as f64 * 0.1, 0.0]);
            }
        }
        for i in 0..50 {
            for k in 0..20 {
                points.push([i as f64 * 0.1, 0.0, 2.5 + k as f64 * 0.1]);
            }
        }
        let ground: Vec<u8> = points.iter().map(|p| if p[2] < 0.05 { CLASS_GROUND } else { CLASS_UNCLASSIFIED }).collect();
        let options = ClassifyOptions { sample: 2000, ..ClassifyOptions::default() };
        let (classes, result) = classify_level(&points, &ground, &options);
        assert_eq!(classes.len(), points.len());
        assert_eq!(result.points, points.len());
        assert!(result.counts[CLASS_GROUND as usize] > 0, "the floor is the ground");
        assert!(result.counts[CLASS_BUILDING as usize] > 0, "the wall is a building: {result:?}");
        assert!(result.summary().contains("ground"), "{}", result.summary());
    }
}
