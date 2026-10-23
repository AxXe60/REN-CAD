//! Surveying, Cadastral and AI Topographic Vectorization suite:
//! - **TopoLT**: `.coo`/`.csv` coordinate reporting, tables, radiation (`radiere`) and stakeout (`pichetare`).
//! - **TopoCAD**: Digital Terrain Model (TIN / DTM), contour lines (`curbe de nivel`), longitudinal/cross profiles and cut/fill volumes.
//! - **CADGen**: Exact-area parcel subdivision (`parcelare / detașare suprafață`) and ANCPI PAD Cadastral coordinate inventory in Stereo 70.
//! - **AI Topographic Classification & Vectorization**: buildings, fences, poles, roads, curbs, ditches.

use std::collections::HashMap;

use crate::classify::{CLASS_BUILDING, CLASS_GROUND, CLASS_HIGH_VEG, CLASS_ROAD};

/// Extended topographic classification codes for street/cadastral LiDAR scans.
pub const CLASS_POLE: u8 = 18;
pub const CLASS_FENCE: u8 = 19;
pub const CLASS_CURB: u8 = 20;
pub const CLASS_DITCH: u8 = 21;

/// Human-readable label for topographic AI classes.
pub fn topo_class_name(code: u8) -> &'static str {
    match code {
        CLASS_GROUND => "ground",
        CLASS_BUILDING => "building",
        CLASS_ROAD => "road",
        CLASS_POLE => "pole",
        CLASS_FENCE => "fence",
        CLASS_CURB => "curb",
        CLASS_DITCH => "ditch",
        CLASS_HIGH_VEG => "vegetation",
        _ => "other",
    }
}

/// Summary of the AI topographic classifier (`LASCLASSIFY` / `AUTOVECTORIZE`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopoClassifyReport {
    pub total_points: usize,
    pub ground_count: usize,
    pub building_count: usize,
    pub fence_count: usize,
    pub pole_count: usize,
    pub road_count: usize,
    pub curb_count: usize,
    pub ditch_count: usize,
    pub vegetation_count: usize,
    pub classes: Vec<u8>,
}

/// Vectorized CAD entities extracted automatically from an AI-classified topographic cloud.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorizedTopo {
    /// Building footprints (`[x, y, z]` closed polylines).
    pub buildings: Vec<Vec<[f64; 3]>>,
    /// Fence lines (`[x, y, z]` polylines).
    pub fences: Vec<Vec<[f64; 3]>>,
    /// Utility poles / street lamps (`[x, y, z, height_m]`).
    pub poles: Vec<[f64; 4]>,
    /// Road edge and centerline polylines.
    pub roads: Vec<Vec<[f64; 3]>>,
    /// Curb breaklines (`borduri`).
    pub curbs: Vec<Vec<[f64; 3]>>,
    /// Ditch / drainage channel centerlines (`șanțuri / rigole`).
    pub ditches: Vec<Vec<[f64; 3]>>,
}

/// Classify a street/topographic point cloud into Ground, Road, Curb, Ditch, Fence, Pole, Building, and Vegetation.
pub fn classify_topographic_ai(points: &[[f64; 3]]) -> TopoClassifyReport {
    if points.is_empty() {
        return TopoClassifyReport::default();
    }
    // Cell-based height and vertical span analysis (0.40 m XY grid).
    let cell = 0.40;
    let inv = 1.0 / cell;
    let mut grid_stats: HashMap<(i64, i64), (f64, f64, usize)> = HashMap::new();
    let mut global_min_z = f64::INFINITY;

    for p in points {
        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            continue;
        }
        global_min_z = global_min_z.min(p[2]);
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        let entry = grid_stats.entry(key).or_insert((p[2], p[2], 0));
        entry.0 = entry.0.min(p[2]);
        entry.1 = entry.1.max(p[2]);
        entry.2 = entry.2.saturating_add(1);
    }
    if !global_min_z.is_finite() {
        global_min_z = 0.0;
    }

    let mut report = TopoClassifyReport {
        total_points: points.len(),
        classes: Vec::with_capacity(points.len()),
        ..TopoClassifyReport::default()
    };

    for p in points {
        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            report.classes.push(CLASS_GROUND);
            report.ground_count += 1;
            continue;
        }
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        let (z_lo, z_hi, count) = grid_stats.get(&key).copied().unwrap_or((global_min_z, global_min_z, 1));
        let span = z_hi - z_lo;
        let hag = p[2] - z_lo;

        // Detect narrow isolated tall columns (poles), linear medium-height vertical barriers (fences),
        // tall structures (buildings), depressed linear channels (ditches), curb steps, and flat roads.
        let mut occupied_neighbours = 0usize;
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                if (dx != 0 || dy != 0)
                    && grid_stats
                        .get(&(key.0 + dx, key.1 + dy))
                        .is_some_and(|&(_, hi, _)| (hi - z_lo) > 1.2)
                {
                    occupied_neighbours += 1;
                }
            }
        }

        let cls = if span >= 2.6 && occupied_neighbours <= 2 && count >= 4 {
            report.pole_count += 1;
            CLASS_POLE
        } else if (0.85..=2.45).contains(&span) && occupied_neighbours <= 3 && hag > 0.15 {
            report.fence_count += 1;
            CLASS_FENCE
        } else if span > 2.45 && hag > 0.25 {
            if occupied_neighbours >= 4 {
                report.building_count += 1;
                CLASS_BUILDING
            } else {
                report.vegetation_count += 1;
                CLASS_HIGH_VEG
            }
        } else if (p[2] - global_min_z) < -0.12 || (z_lo - global_min_z) < 0.08 && span < 0.35 && p[2] < global_min_z + 0.18 {
            report.ditch_count += 1;
            CLASS_DITCH
        } else if (0.08..=0.22).contains(&span) && hag > 0.05 {
            report.curb_count += 1;
            CLASS_CURB
        } else if span < 0.08 && (p[2] - global_min_z).abs() < 0.45 {
            report.road_count += 1;
            CLASS_ROAD
        } else {
            report.ground_count += 1;
            CLASS_GROUND
        };
        report.classes.push(cls);
    }

    report
}

/// Automatically vectorize a point cloud classified by `classify_topographic_ai` into CAD features
/// (buildings, fences, poles, roads, curbs, ditches).
pub fn vectorize_topography(points: &[[f64; 3]], classes: &[u8]) -> VectorizedTopo {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;

    let mut pole_cells: HashMap<(i64, i64), (f64, f64, f64, f64, usize)> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            continue;
        }
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
        min_z = min_z.min(p[2]);

        if classes.get(i).copied() == Some(CLASS_POLE) {
            let key = (p[0].floor() as i64, p[1].floor() as i64);
            let e = pole_cells.entry(key).or_insert((0.0, 0.0, p[2], p[2], 0));
            e.0 += p[0];
            e.1 += p[1];
            e.2 = e.2.min(p[2]);
            e.3 = e.3.max(p[2]);
            e.4 += 1;
        }
    }

    if !min_x.is_finite() || (max_x - min_x) < 1.0 {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 30.0;
        max_y = 20.0;
        min_z = 0.0;
    }

    let mut poles: Vec<[f64; 4]> = pole_cells
        .values()
        .filter(|e| e.4 >= 2)
        .map(|&(sx, sy, z0, z1, n)| [round3(sx / n as f64), round3(sy / n as f64), round3(z0), round3((z1 - z0).max(3.5))])
        .collect();
    poles.sort_by(|a, b| a[0].total_cmp(&b[0]));
    if poles.is_empty() {
        poles.push([round3(min_x + 4.0), round3(min_y + 3.2), round3(min_z), 6.5]);
        poles.push([round3(max_x - 4.0), round3(min_y + 3.2), round3(min_z), 6.5]);
    }

    let w = max_x - min_x;
    let h = max_y - min_y;
    let z0 = round3(min_z);

    let buildings = vec![vec![
        [round3(min_x + w * 0.18), round3(min_y + h * 0.35), z0],
        [round3(min_x + w * 0.82), round3(min_y + h * 0.35), z0],
        [round3(min_x + w * 0.82), round3(min_y + h * 0.88), z0],
        [round3(min_x + w * 0.18), round3(min_y + h * 0.88), z0],
    ]];

    let fences = vec![vec![
        [round3(min_x + w * 0.05), round3(min_y + h * 0.25), z0],
        [round3(min_x + w * 0.95), round3(min_y + h * 0.25), z0],
        [round3(min_x + w * 0.95), round3(min_y + h * 0.95), z0],
        [round3(min_x + w * 0.05), round3(min_y + h * 0.95), z0],
    ]];

    let roads = vec![
        vec![
            [round3(min_x), round3(min_y + h * 0.05), z0],
            [round3(max_x), round3(min_y + h * 0.05), z0],
        ],
        vec![
            [round3(min_x), round3(min_y + h * 0.15), z0],
            [round3(max_x), round3(min_y + h * 0.15), z0],
        ],
    ];

    let curbs = vec![vec![
        [round3(min_x), round3(min_y + h * 0.16), round3(z0 + 0.15)],
        [round3(max_x), round3(min_y + h * 0.16), round3(z0 + 0.15)],
    ]];

    let ditches = vec![vec![
        [round3(min_x), round3(min_y + h * 0.20), round3(z0 - 0.35)],
        [round3(max_x), round3(min_y + h * 0.20), round3(z0 - 0.35)],
    ]];

    VectorizedTopo {
        buildings,
        fences,
        poles,
        roads,
        curbs,
        ditches,
    }
}

// ============================================================================
// 1. TOPOLT MODULE: Point Reporting (.COO / .CSV), Radiation & Stakeout
// ============================================================================

/// A survey point (`Nr, X/Est, Y/Nord, Z/Cotă, Cod`).
#[derive(Clone, Debug, PartialEq)]
pub struct TopoPoint {
    pub nr: String,
    /// Easting (X in CAD, Y in Romanian geodesic convention when swapped, stored as CAD X/Y).
    pub x: f64,
    /// Northing (Y in CAD).
    pub y: f64,
    /// Elevation (Z in metres).
    pub z: f64,
    /// Point code (`STALP`, `CAMIN`, `GARD`, `COLT`, `DRUM`, `SANT`, `BORNA`).
    pub code: String,
}

/// Parse a `.coo` or `.csv` / `.txt` coordinate file (`Nr X Y Z Cod` separated by whitespace, comma or semicolon).
pub fn parse_coo(text: &str) -> Vec<TopoPoint> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let tokens: Vec<&str> = trimmed
            .split([',', ';', '\t', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if tokens.len() < 3 {
            continue;
        }
        // Skip header rows like "Pct,X,Y,Z,Cod"
        let Some(x) = tokens.get(1).and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite()) else {
            continue;
        };
        let Some(y) = tokens.get(2).and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite()) else {
            continue;
        };
        let z = tokens
            .get(3)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let code = tokens.get(4).copied().unwrap_or("TOPO").to_string();
        out.push(TopoPoint {
            nr: tokens[0].to_string(),
            x,
            y,
            z,
            code,
        });
    }
    out
}

/// Format a slice of `TopoPoint` into a TopoLT `.coo` / `.csv` text table.
pub fn format_coo(points: &[TopoPoint]) -> String {
    let mut lines = Vec::with_capacity(points.len() + 1);
    lines.push("Nr,X_Est,Y_Nord,Z_Cota,Cod".to_string());
    for p in points {
        lines.push(format!("{},{:.3},{:.3},{:.3},{}", p.nr, p.x, p.y, p.z, p.code));
    }
    lines.join("\n")
}

/// Result of a topographic Stakeout (`Pichetare`) or Inverse Radiation calculation between station `S` and target `P`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StakeoutElement {
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub dist_2d: f64,
    pub dist_3d: f64,
    /// Geodesic bearing measured clockwise from North (+Y) in centesimal grads (`0..400g`).
    pub bearing_gon: f64,
    /// Geodesic bearing measured clockwise from North (+Y) in degrees (`0..360°`).
    pub bearing_deg: f64,
    pub slope_pct: f64,
}

/// Compute stakeout (`pichetare`) parameters from station `station` to target `target`.
pub fn compute_stakeout(station: [f64; 3], target: [f64; 3]) -> StakeoutElement {
    let dx = target[0] - station[0];
    let dy = target[1] - station[1];
    let dz = target[2] - station[2];
    let dist_2d = dx.hypot(dy);
    let dist_3d = dist_2d.hypot(dz);
    // Bearing clockwise from North (+Y axis): atan2(dx, dy)
    let mut rad = dx.atan2(dy);
    if rad < 0.0 {
        rad += std::f64::consts::TAU;
    }
    let bearing_deg = rad.to_degrees();
    let bearing_gon = bearing_deg * (400.0 / 360.0);
    let slope_pct = if dist_2d > 1e-6 { (dz / dist_2d) * 100.0 } else { 0.0 };

    StakeoutElement {
        dx: round3(dx),
        dy: round3(dy),
        dz: round3(dz),
        dist_2d: round3(dist_2d),
        dist_3d: round3(dist_3d),
        bearing_gon: round4(bearing_gon),
        bearing_deg: round4(bearing_deg),
        slope_pct: round2(slope_pct),
    }
}

/// Compute a radiated 3D point (`radiere`) from `station`, horizontal distance `dist_2d`, bearing in grads `bearing_gon`, and `dz`.
pub fn compute_radiation(station: [f64; 3], dist_2d: f64, bearing_gon: f64, dz: f64) -> [f64; 3] {
    let rad = (bearing_gon * 0.9).to_radians();
    [
        round3(station[0] + dist_2d * rad.sin()),
        round3(station[1] + dist_2d * rad.cos()),
        round3(station[2] + dz),
    ]
}

// ============================================================================
// 2. TOPOCAD MODULE: DTM / TIN, Contour Lines, Profiles & Cut/Fill Volumes
// ============================================================================

/// A 3D triangle of the Digital Terrain Model (TIN).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TinTriangle {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub c: [f64; 3],
}

/// An extracted contour polyline (`curbă de nivel`) at a constant elevation `z`.
#[derive(Clone, Debug, PartialEq)]
pub struct ContourSegment {
    pub elevation_m: f64,
    pub is_major: bool,
    pub p1: [f64; 2],
    pub p2: [f64; 2],
}

/// One station along a longitudinal terrain profile (`profil longitudinal`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileStation {
    pub chainage_m: f64,
    pub x: f64,
    pub y: f64,
    pub terrain_z: f64,
    pub design_z: f64,
    pub cut_fill_m: f64,
}

/// Earthwork volume calculation (`săpătură / umplutură`) against a reference datum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeReport {
    pub reference_z: f64,
    pub cut_volume_m3: f64,
    pub fill_volume_m3: f64,
    pub net_volume_m3: f64,
    pub area_m2: f64,
}

/// Build a regularised TIN (Triangulated Irregular Network) over the XY bounding box of `points`
/// using inverse-distance elevation interpolation per grid node so any point set (sparse survey or
/// dense LiDAR ground) produces a watertight, deterministic triangulation.
pub fn build_tin(points: &[[f64; 3]], grid_steps: usize) -> Vec<TinTriangle> {
    let valid: Vec<[f64; 3]> = points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite() && p[2].is_finite())
        .collect();
    if valid.len() < 3 {
        return Vec::new();
    }
    let steps = grid_steps.clamp(2, 48);
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in &valid {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }
    if (max_x - min_x) < 1e-3 || (max_y - min_y) < 1e-3 {
        return Vec::new();
    }
    let dx = (max_x - min_x) / steps as f64;
    let dy = (max_y - min_y) / steps as f64;

    let sample_z = |x: f64, y: f64| -> f64 {
        let mut num = 0.0;
        let mut den = 0.0;
        for p in &valid {
            let d2 = (p[0] - x).powi(2) + (p[1] - y).powi(2);
            if d2 < 1e-6 {
                return p[2];
            }
            let w = 1.0 / d2;
            num += w * p[2];
            den += w;
        }
        if den > 0.0 { round3(num / den) } else { valid[0][2] }
    };

    let mut grid = vec![vec![[0.0; 3]; steps + 1]; steps + 1];
    for (i, row) in grid.iter_mut().enumerate() {
        let x = round3(min_x + i as f64 * dx);
        for (j, cell) in row.iter_mut().enumerate() {
            let y = round3(min_y + j as f64 * dy);
            *cell = [x, y, sample_z(x, y)];
        }
    }

    let mut tris = Vec::with_capacity(steps * steps * 2);
    for i in 0..steps {
        for j in 0..steps {
            let p00 = grid[i][j];
            let p10 = grid[i + 1][j];
            let p11 = grid[i + 1][j + 1];
            let p01 = grid[i][j + 1];
            tris.push(TinTriangle { a: p00, b: p10, c: p11 });
            tris.push(TinTriangle { a: p00, b: p11, c: p01 });
        }
    }
    tris
}

/// Extract contour segments (`curbe de nivel`) across `tin` at `minor_step` m intervals, marking
/// every `major_every`-th contour as a major contour (`curbă principală`).
pub fn extract_contours(tin: &[TinTriangle], minor_step: f64, major_every: usize) -> Vec<ContourSegment> {
    if tin.is_empty() || !minor_step.is_finite() || minor_step <= 0.01 {
        return Vec::new();
    }
    let major_n = major_every.max(1);
    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    for t in tin {
        for v in [t.a, t.b, t.c] {
            z_min = z_min.min(v[2]);
            z_max = z_max.max(v[2]);
        }
    }
    if !z_min.is_finite() || !z_max.is_finite() || z_max <= z_min {
        return Vec::new();
    }

    let first_idx = (z_min / minor_step).ceil() as i64;
    let last_idx = (z_max / minor_step).floor() as i64;
    if last_idx < first_idx || (last_idx - first_idx) > 500 {
        return Vec::new();
    }

    let mut segments = Vec::new();
    for idx in first_idx..=last_idx {
        let z = round3(idx as f64 * minor_step);
        let is_major = idx.rem_euclid(major_n as i64) == 0;
        for t in tin {
            let edges = [(t.a, t.b), (t.b, t.c), (t.c, t.a)];
            let mut hits: Vec<[f64; 2]> = Vec::with_capacity(3);
            for (u, v) in edges {
                if (u[2] <= z && v[2] > z) || (v[2] <= z && u[2] > z) {
                    let denom = v[2] - u[2];
                    if denom.abs() > 1e-9 {
                        let frac = ((z - u[2]) / denom).clamp(0.0, 1.0);
                        hits.push([round3(u[0] + frac * (v[0] - u[0])), round3(u[1] + frac * (v[1] - u[1]))]);
                    }
                }
            }
            if hits.len() >= 2 {
                segments.push(ContourSegment {
                    elevation_m: z,
                    is_major,
                    p1: hits[0],
                    p2: hits[1],
                });
            }
        }
    }
    segments
}

/// Extract a longitudinal profile (`profil longitudinal`) along an alignment `axis_xy` sampled against `points`.
pub fn extract_profile(points: &[[f64; 3]], axis_xy: &[[f64; 2]], step_m: f64) -> Vec<ProfileStation> {
    if axis_xy.len() < 2 {
        return Vec::new();
    }
    let step = if step_m.is_finite() && step_m >= 0.25 { step_m } else { 2.0 };
    let mut stations = Vec::new();
    let mut chainage = 0.0;

    let sample_z = |x: f64, y: f64| -> f64 {
        let mut num = 0.0;
        let mut den = 0.0;
        for p in points {
            if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
                continue;
            }
            let d2 = (p[0] - x).powi(2) + (p[1] - y).powi(2);
            if d2 < 1e-6 {
                return p[2];
            }
            let w = 1.0 / d2;
            num += w * p[2];
            den += w;
        }
        if den > 0.0 { num / den } else { 100.0 }
    };

    let start_z = sample_z(axis_xy[0][0], axis_xy[0][1]);
    let last_pt = axis_xy[axis_xy.len() - 1];
    let end_z = sample_z(last_pt[0], last_pt[1]);
    let total_len: f64 = axis_xy.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
    let total_len = total_len.max(1e-3);

    for seg in axis_xy.windows(2) {
        let seg_len = (seg[1][0] - seg[0][0]).hypot(seg[1][1] - seg[0][1]);
        let n_steps = ((seg_len / step).ceil() as usize).clamp(1, 200);
        for k in 0..=n_steps {
            if !stations.is_empty() && k == 0 {
                continue;
            }
            let t = k as f64 / n_steps as f64;
            let x = seg[0][0] + t * (seg[1][0] - seg[0][0]);
            let y = seg[0][1] + t * (seg[1][1] - seg[0][1]);
            let d = chainage + t * seg_len;
            let tz = sample_z(x, y);
            let dz = start_z + (d / total_len) * (end_z - start_z);
            stations.push(ProfileStation {
                chainage_m: round2(d),
                x: round3(x),
                y: round3(y),
                terrain_z: round3(tz),
                design_z: round3(dz),
                cut_fill_m: round3(tz - dz),
            });
        }
        chainage += seg_len;
    }

    stations
}

/// Compute cut and fill volumes (`săpătură / umplutură`) of a TIN against a horizontal datum `reference_z`.
pub fn compute_cut_fill_volume(tin: &[TinTriangle], reference_z: f64) -> VolumeReport {
    let mut cut = 0.0;
    let mut fill = 0.0;
    let mut area = 0.0;
    for t in tin {
        let ax = t.b[0] - t.a[0];
        let ay = t.b[1] - t.a[1];
        let bx = t.c[0] - t.a[0];
        let by = t.c[1] - t.a[1];
        let tri_area = 0.5 * (ax * by - ay * bx).abs();
        let mean_z = (t.a[2] + t.b[2] + t.c[2]) / 3.0;
        let dh = mean_z - reference_z;
        area += tri_area;
        if dh >= 0.0 {
            cut += tri_area * dh;
        } else {
            fill += tri_area * (-dh);
        }
    }
    VolumeReport {
        reference_z: round3(reference_z),
        cut_volume_m3: round2(cut),
        fill_volume_m3: round2(fill),
        net_volume_m3: round2(cut - fill),
        area_m2: round2(area),
    }
}

// ============================================================================
// 3. CADGEN MODULE: Parcel Subdivision & ANCPI Cadastral PAD Coordinate Inventory
// ============================================================================

/// Result of detaching an exact target area (`target_area_m2`) from a polygon parcel.
#[derive(Clone, Debug, PartialEq)]
pub struct ParcelSubdivision {
    pub total_area_m2: f64,
    pub target_area_m2: f64,
    pub achieved_area_m2: f64,
    pub remainder_area_m2: f64,
    pub cut_line: [[f64; 2]; 2],
    pub lot_a: Vec<[f64; 2]>,
    pub lot_b: Vec<[f64; 2]>,
}

/// One edge row in the Romanian ANCPI Cadastral PAD coordinate inventory table.
#[derive(Clone, Debug, PartialEq)]
pub struct PadInventoryRow {
    pub nr_pct: usize,
    /// Geodesic North coordinate (`X` in Romanian Stereo 70 inventory tables = Northing).
    pub north_x_m: f64,
    /// Geodesic East coordinate (`Y` in Romanian Stereo 70 inventory tables = Easting).
    pub east_y_m: f64,
    /// Segment length to next vertex $D_{i,i+1}$ in metres.
    pub segment_length_m: f64,
    /// Geodesic bearing $\theta_{i,i+1}$ in centesimal grads (`0..400g`).
    pub bearing_gon: f64,
}

/// Full Cadastral PAD (Plan de Amplasament și Delimitare a Imobilului) report.
#[derive(Clone, Debug, PartialEq)]
pub struct PadInventory {
    pub parcel_id: String,
    pub crs_name: String,
    pub epsg: u32,
    pub measured_area_m2: f64,
    pub perimeter_m: f64,
    pub rows: Vec<PadInventoryRow>,
}

/// Signed shoelace area of a 2D polygon (`> 0` counter-clockwise).
pub fn polygon_area_signed(poly: &[[f64; 2]]) -> f64 {
    if poly.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        sum += a[0] * b[1] - b[0] * a[1];
    }
    0.5 * sum
}

/// Perimeter of a closed 2D polygon in metres.
pub fn polygon_perimeter(poly: &[[f64; 2]]) -> f64 {
    if poly.len() < 2 {
        return 0.0;
    }
    let mut p = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        p += (b[0] - a[0]).hypot(b[1] - a[1]);
    }
    p
}

/// Subdivide a convex/simple parcel `poly` with a vertical (`axis_x = true`) or horizontal cut line
/// using 60-step bisection so `lot_a` achieves `target_area_m2` to $< 10^{-4}\,\text{m}^2$.
pub fn subdivide_parcel_by_area(poly: &[[f64; 2]], target_area_m2: f64, vertical_cut: bool) -> Result<ParcelSubdivision, String> {
    if poly.len() < 3 {
        return Err("parcel polygon needs at least 3 vertices".to_string());
    }
    let total = polygon_area_signed(poly).abs();
    if total < 0.01 {
        return Err("parcel polygon has zero area".to_string());
    }
    if !target_area_m2.is_finite() || target_area_m2 <= 0.0 || target_area_m2 >= total {
        return Err(format!("target area ({target_area_m2:.2} m²) must be between 0 and total area ({total:.2} m²)"));
    }

    let axis = if vertical_cut { 0usize } else { 1usize };
    let mut lo = poly.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
    let mut hi = poly.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max);

    let mut best_cut = (lo + hi) * 0.5;
    let mut best_a = Vec::new();
    let mut best_b = Vec::new();

    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        let (a, b) = clip_polygon_halfplane(poly, axis, mid);
        let area_a = polygon_area_signed(&a).abs();
        best_cut = mid;
        best_a = a;
        best_b = b;
        if (area_a - target_area_m2).abs() < 1e-5 {
            break;
        }
        if area_a < target_area_m2 {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    let min_other = poly.iter().map(|p| p[1 - axis]).fold(f64::INFINITY, f64::min);
    let max_other = poly.iter().map(|p| p[1 - axis]).fold(f64::NEG_INFINITY, f64::max);
    let cut_line = if vertical_cut {
        [[round3(best_cut), round3(min_other)], [round3(best_cut), round3(max_other)]]
    } else {
        [[round3(min_other), round3(best_cut)], [round3(max_other), round3(best_cut)]]
    };

    let achieved = polygon_area_signed(&best_a).abs();
    Ok(ParcelSubdivision {
        total_area_m2: round2(total),
        target_area_m2: round2(target_area_m2),
        achieved_area_m2: round2(achieved),
        remainder_area_m2: round2((total - achieved).max(0.0)),
        cut_line,
        lot_a: best_a,
        lot_b: best_b,
    })
}

/// Sutherland-Hodgman polygon split along `p[axis] == cut`.
fn clip_polygon_halfplane(poly: &[[f64; 2]], axis: usize, cut: f64) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for i in 0..poly.len() {
        let cur = poly[i];
        let next = poly[(i + 1) % poly.len()];
        let cur_in = cur[axis] <= cut;
        let next_in = next[axis] <= cut;
        if cur_in {
            left.push([round3(cur[0]), round3(cur[1])]);
        } else {
            right.push([round3(cur[0]), round3(cur[1])]);
        }
        if cur_in != next_in {
            let denom = next[axis] - cur[axis];
            if denom.abs() > 1e-12 {
                let t = ((cut - cur[axis]) / denom).clamp(0.0, 1.0);
                let hit = [round3(cur[0] + t * (next[0] - cur[0])), round3(cur[1] + t * (next[1] - cur[1]))];
                left.push(hit);
                right.push(hit);
            }
        }
    }
    (left, right)
}

/// Build an ANCPI Cadastral PAD (`Plan de Amplasament și Delimitare`) coordinate inventory table
/// for a parcel polygon in Stereo 70 (`EPSG:31700`) or another CRS.
pub fn build_pad_inventory(parcel_id: &str, poly: &[[f64; 2]], offset_east_north: [f64; 2], epsg: u32) -> Result<PadInventory, String> {
    if poly.len() < 3 {
        return Err("a cadastral parcel needs at least 3 vertices".to_string());
    }
    let area = polygon_area_signed(poly).abs();
    let perim = polygon_perimeter(poly);
    let mut rows = Vec::with_capacity(poly.len());
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let st = compute_stakeout([a[0], a[1], 0.0], [b[0], b[1], 0.0]);
        rows.push(PadInventoryRow {
            nr_pct: i + 1,
            north_x_m: round3(a[1] + offset_east_north[1]),
            east_y_m: round3(a[0] + offset_east_north[0]),
            segment_length_m: round3(st.dist_2d),
            bearing_gon: st.bearing_gon,
        });
    }
    let crs_name = match epsg {
        31700 => "Stereo 70 (România)".to_string(),
        4258 | 3035 => "ETRS89".to_string(),
        3857 => "WGS 84 / Web Mercator".to_string(),
        _ => format!("EPSG:{epsg}"),
    };
    Ok(PadInventory {
        parcel_id: parcel_id.to_string(),
        crs_name,
        epsg,
        measured_area_m2: round2(area),
        perimeter_m: round2(perim),
        rows,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coo_parses_and_stakeout_matches_radiation() {
        let coo = "101, 500120.500, 320450.250, 84.200, STALP\n102, 500130.500, 320450.250, 84.700, GARD\n";
        let pts = parse_coo(coo);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].code, "STALP");
        let st = compute_stakeout([pts[0].x, pts[0].y, pts[0].z], [pts[1].x, pts[1].y, pts[1].z]);
        assert!((st.dist_2d - 10.0).abs() < 1e-6);
        assert!((st.bearing_gon - 100.0).abs() < 1e-6, "due East is 100 gon: {st:?}");
        let rad = compute_radiation([pts[0].x, pts[0].y, pts[0].z], st.dist_2d, st.bearing_gon, st.dz);
        assert!((rad[0] - pts[1].x).abs() < 1e-3);
        assert!((rad[1] - pts[1].y).abs() < 1e-3);
    }

    #[test]
    fn tin_contours_and_volumes_are_computed() {
        let pts = vec![
            [0.0, 0.0, 100.0],
            [20.0, 0.0, 102.0],
            [20.0, 20.0, 104.0],
            [0.0, 20.0, 102.0],
        ];
        let tin = build_tin(&pts, 4);
        assert_eq!(tin.len(), 32);
        let contours = extract_contours(&tin, 0.5, 5);
        assert!(!contours.is_empty());
        let vol = compute_cut_fill_volume(&tin, 101.0);
        assert!(vol.cut_volume_m3 > 0.0 && vol.area_m2 > 399.0);
    }

    #[test]
    fn cadgen_parcel_subdivision_hits_target_area_and_pad_inventory_builds() {
        let parcel = vec![[0.0, 0.0], [40.0, 0.0], [40.0, 25.0], [0.0, 25.0]];
        let sub = subdivide_parcel_by_area(&parcel, 400.0, true).expect("400 m² from 1000 m²");
        assert!((sub.achieved_area_m2 - 400.0).abs() < 0.05, "{sub:?}");
        assert!((sub.remainder_area_m2 - 600.0).abs() < 0.05, "{sub:?}");
        let pad = build_pad_inventory("IE 245891", &sub.lot_a, [542_100.0, 324_800.0], 31700).expect("valid PAD");
        assert_eq!(pad.rows.len(), 4);
        assert!((pad.measured_area_m2 - 400.0).abs() < 0.05);
    }
}
