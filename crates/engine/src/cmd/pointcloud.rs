//! Point cloud commands: LASINFO, LASEXPORT and the LASOPEN canvas overlay.
//!
//! M1 was stateless: read a LAS/LAZ file, cut a Z band, write a DXF. M2 keeps the band in the
//! session ([`Session::clouds`]) so the canvas draws it under the drawing and the user traces
//! over it without a second file.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::View;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("lasinfo", "Point Cloud Information", |_, p| lasinfo(p)).params("{path: string}").enabled(always).noundo(),
        CommandSpec::new("lasexport", "Export Point Cloud Slice", lasexport)
            .params("{path: string, out: string, z0?: number, z1?: number, decimate?: number, layer?: string}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasopen", "Open Point Cloud", lasopen)
            .params("{path: string, z0?: number, z1?: number, decimate?: number, maxPoints?: int, color?: string, name?: string}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lascolor", "Colour Point Cloud", lascolor).params("{mode: string, index?: int}").enabled(always).noundo(),
        CommandSpec::new("lasdecimate", "Decimate Point Cloud", lasdecimate)
            .params("{cell: number, index?: int} (0 = every point of the band)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasclip", "Crop Point Cloud", lasclip)
            .params("{min: [x,y,z], max: [x,y,z], index?: int} | {reset: true}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasvisible", "Show Point Cloud", lasvisible).params("{visible?: bool, index?: int}").enabled(always).noundo(),
        CommandSpec::new("lasground", "Find the Ground", lasground)
            .params("{cell?: number, tolerance?: number, bump?: number, sigma?: number, show?: bool, index?: int}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasclass", "Classify Point Cloud", lasclass)
            .params("{mode?: string, cell?: number, radius?: number, sample?: int, index?: int}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lascluster", "Cluster the Cloud", lascluster)
            .params("{k?: int, cluster?: int, class?: string, index?: int}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasorigin", "Set the Site Origin", lasorigin)
            .params("{x?: number, y?: number, z?: number, reset?: bool}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lascrs", "Set the Project CRS", lascrs).params("{epsg?: int, name?: string, reset?: bool}").enabled(always).noundo(),
        CommandSpec::new("lasgeo", "Tie the Project to the Map", lasgeo).params("{lat: number, lon: number}").enabled(always).noundo(),
        CommandSpec::new("lascmp", "Compare Two Clouds", lascmp).params("{index?: int, reference?: int, cell?: number}").enabled(always).noundo(),
        CommandSpec::new("lasopacity", "Set the Cloud Opacity", lasopacity).params("{opacity?: number, index?: int}").enabled(always).noundo(),
        CommandSpec::new("lpectrum", "Choose the Intensity Scale", lpectrum).params("{spectrum?: string, index?: int}").enabled(always).noundo(),
        CommandSpec::new("lasframe", "Frame Point Cloud", lasframe).params("{index?: int}").enabled(always).noundo(),
        CommandSpec::new("lasclose", "Close Point Cloud", lasclose).params("{index?: int, all?: bool}").enabled(always).noundo(),
        CommandSpec::new("lasclouds", "List Point Clouds", |s, _| lasclouds(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dline", "3D Line", draw3d).params("{points: [[x,y,z], ...], layer?: string, closed?: bool}").enabled(always).noundo(),
        CommandSpec::new("3dclear", "Clear 3D Sketch", |s, _| sketch_clear(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dundo", "Undo 3D Stroke", |s, _| sketch_undo(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dlist", "List 3D Sketch", |s, _| sketch_list(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dexport", "Export 3D Sketch", sketch_export).params("{path: string, layer?: string}").enabled(always).noundo(),
        CommandSpec::new("3dsnap", "3D Snap Mode", snap3d).params("{mode: \"smooth\"|\"exact\"}").enabled(always).noundo(),
        CommandSpec::new("3dmeasure", "Measure 3D", |_, p| measure3d(p)).params("{p1: [x,y,z], p2: [x,y,z]}").enabled(always).noundo(),
    ]
}

/// The most vertices one 3D stroke may hold: drawn by hand it never comes close, and a script that
/// asks for millions should hear about it instead of allocating them.
const MAX_STROKE_VERTICES: usize = 100_000;

fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or_else(|| EngineError::Other(format!("`{key}` is required")))
}

/// Whether the name is a point cloud, which OPEN hands to LASOPEN instead of reading it as a
/// drawing: people type `OPEN scan.las` and expect the cloud, not "unsupported file type".
pub(super) fn is_cloud_path(name: &str) -> bool {
    matches!(Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("las" | "laz"))
}

fn number(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn oops(e: cadcraft_pointcloud::Error) -> EngineError {
    EngineError::Other(e.to_string())
}

fn lasinfo(p: &Value) -> Result<Value> {
    let path = PathBuf::from(text(p, "path")?);
    let info = cadcraft_pointcloud::read_info(&path).map_err(oops)?;
    Ok(json!({
        "path": path.display().to_string(),
        "points": info.points,
        "bounds": { "min": info.bounds.min, "max": info.bounds.max },
        "scale": info.scale,
        "offset": info.offset,
        "pointFormat": info.point_format,
        "hasColor": info.has_color,
        "hasIntensity": info.has_intensity,
        "crs": crs_json(&info.crs),
    }))
}

/// The CRS as the reports show it: the EPSG code, the name and whether it is geographic.
fn crs_json(crs: &Option<cadcraft_pointcloud::Crs>) -> Value {
    match crs {
        Some(c) => json!({
            "epsg": c.epsg,
            "name": c.name(),
            "geographic": c.is_geographic(),
            "wkt": c.wkt.is_some(),
            "falseEasting": c.false_easting,
            "falseNorthing": c.false_northing,
        }),
        None => Value::Null,
    }
}

fn lasexport(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(text(p, "path")?);
    let out = PathBuf::from(text(p, "out")?);
    let info = cadcraft_pointcloud::read_info(&path).map_err(oops)?;
    let z0 = number(p, "z0").unwrap_or(info.bounds.min[2]);
    let z1 = number(p, "z1").unwrap_or(info.bounds.max[2]);
    let decimate = number(p, "decimate").filter(|v| *v > 0.0);
    let layer = p.get("layer").and_then(Value::as_str).unwrap_or("PC-SLICE").to_string();

    // The export reads the whole band: the memory stays bounded by the all-points limit, and the
    // cell grows if the band holds more than that.
    let slice = cadcraft_pointcloud::slice_capped(&path, z0, z1, decimate, cadcraft_pointcloud::ALL_POINTS_LIMIT).map_err(oops)?;
    let written = cadcraft_pointcloud::write_points_dxf(&out, &slice.points, &layer).map_err(oops)?;
    // The CRS of the scan goes next to the drawing as a `.prj` sidecar (the convention every
    // GIS tool reads), so the exported slice keeps its place on the globe. The file's own system
    // wins; the project's (LASCRS) is the fallback for the files that name none.
    let file_crs = cadcraft_pointcloud::read_info(&path).map_err(oops)?.crs;
    let crs = file_crs.clone().or_else(|| s.crs.clone());
    let prj = crs
        .as_ref()
        .and_then(|c| {
            c.prj().map(str::to_string).or_else(|| c.epsg.and_then(|code| cadcraft_pointcloud::Crs::wkt_for(code, c.false_easting, c.false_northing)))
        })
        .map(|wkt| {
            let sidecar = out.with_extension("prj");
            if let Err(e) = std::fs::write(&sidecar, wkt) {
                log::warn!("could not write the .prj sidecar: {e}");
            }
            sidecar.display().to_string()
        });
    Ok(json!({
        "message": format!("{} of {} points in the band written to {}", written, slice.in_band, out.display()),
        "path": out.display().to_string(),
        "layer": layer,
        "points": written,
        "inBand": slice.in_band,
        "filePoints": slice.total,
        "bounds": { "min": slice.bounds.min, "max": slice.bounds.max },
        "crs": crs_json(&crs),
        "prj": prj,
    }))
}

/// Keep a band in the session for the canvas. Without `decimate` the finest level holds every
/// point of the band (up to `maxPoints`); `maxPoints: 0` means the whole band. `color` picks the
/// shading.
pub(super) fn lasopen(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(text(p, "path")?);
    let info = cadcraft_pointcloud::read_info(&path).map_err(oops)?;
    let z0 = number(p, "z0").unwrap_or(info.bounds.min[2]);
    let z1 = number(p, "z1").unwrap_or(info.bounds.max[2]);
    let decimate = number(p, "decimate").filter(|v| *v > 0.0);
    let max_points = match p.get("maxPoints").and_then(Value::as_u64) {
        None => cadcraft_pointcloud::DEFAULT_MAX_POINTS,
        Some(0) => 0,
        Some(v) => (v as usize).clamp(10_000, cadcraft_pointcloud::ALL_POINTS_LIMIT),
    };
    let color = match p.get("color").and_then(Value::as_str) {
        Some(text) => cadcraft_pointcloud::CloudColor::parse(text).ok_or_else(|| EngineError::Other(format!("unknown colour mode `{text}`")))?,
        None => cadcraft_pointcloud::CloudColor::Rgb,
    };
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("Point cloud").to_string());
    let cloud = cadcraft_pointcloud::Cloud::open(&path, name.clone(), z0, z1, decimate, max_points, color).map_err(oops)?;
    let b = cloud.bounds;
    let count = cloud.points();
    let levels = cloud.levels.len();
    let full = cloud.full_points;
    let cell = cloud.levels.first().map_or(0.0, |l| l.cell);
    let crs = cloud.crs.clone();
    // First cloud in an empty drawing: frame it, so LASOPEN is all it takes to see the scan.
    if s.doc().is_ok_and(|d| d.model.is_empty()) {
        let w = b.max[0] - b.min[0];
        let h = b.max[1] - b.min[1];
        let center = Vec2::new((b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0);
        if let Ok(st) = s.state_mut() {
            st.set_view(View { center, height: w.max(h).max(1.0) * 1.2 });
        }
    }
    s.clouds.push(cloud);
    Ok(json!({
        "message": format!(
            "{name}: {count} points{} from {full} in the band ({levels} levels)",
            if cell > 0.0 { format!(" on a {cell} m grid") } else { " (all band points)".into() }
        ),
        "name": name,
        "points": count,
        "levels": levels,
        "cell": cell,
        "inBand": full,
        "filePoints": info.points,
        "color": color.name(),
        "bounds": { "min": b.min, "max": b.max },
        "crs": crs_json(&crs),
        "open": s.clouds.len(),
    }))
}

fn lasclose(s: &mut Session, p: &Value) -> Result<Value> {
    let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
    if all || s.clouds.is_empty() {
        let n = s.clouds.len();
        s.clouds.clear();
        return Ok(json!({ "message": format!("{n} point clouds closed"), "closed": n, "open": 0 }));
    }
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(s.clouds.len() - 1);
    if index >= s.clouds.len() {
        return Err(EngineError::Other(format!("no point cloud {index}; {} open", s.clouds.len())));
    }
    let closed = s.clouds.remove(index);
    Ok(json!({
        "message": format!("{} closed", closed.name),
        "name": closed.name,
        "closed": 1,
        "open": s.clouds.len(),
    }))
}

fn lasclouds(s: &mut Session) -> Result<Value> {
    let list: Vec<Value> = s
        .clouds
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "index": i,
                "name": c.name,
                "path": c.path,
                "points": c.points(),
                "levels": c.levels.len(),
                "inBand": c.full_points,
                "color": c.color.name(),
                "visible": c.visible,
                "ground": c.ground_points(),
                "groundOnly": c.ground_only,
                "crs": crs_json(&c.crs),
                "bounds": { "min": c.bounds.min, "max": c.bounds.max },
            })
        })
        .collect();
    Ok(json!({ "count": list.len(), "clouds": list }))
}

/// The cloud a command means: `index`, or the last one opened.
fn cloud_index(s: &Session, p: &Value) -> Result<usize> {
    let i = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(s.clouds.len().saturating_sub(1));
    if i >= s.clouds.len() {
        return Err(EngineError::Other(format!("no point cloud {i}; {} open", s.clouds.len())));
    }
    Ok(i)
}

fn lascolor(s: &mut Session, p: &Value) -> Result<Value> {
    let text = text(p, "mode")?;
    let mode = cadcraft_pointcloud::CloudColor::parse(text)
        .ok_or_else(|| EngineError::Other(format!("unknown colour mode `{text}`: rgb, elevation, intensity, single:#rrggbb")))?;
    let i = cloud_index(s, p)?;
    let name = {
        let c = s.clouds.get_mut(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
        c.color = mode;
        c.name.clone()
    };
    Ok(json!({
        "message": format!("{name}: colour {}{}", mode.name(), match mode {
            cadcraft_pointcloud::CloudColor::Single([r, g, b]) => format!(" #{r:02x}{g:02x}{b:02x}"),
            _ => String::new(),
        }),
        "index": i,
        "color": mode.name(),
    }))
}

/// Choose the colour scale the intensity (and the elevation) band is read on.
///
/// The scales are drawn in code — grey, the thermal blue-to-red, viridis, turbo, magma, the
/// diverging scale — and the user picks the one the report is read on.
fn lpectrum(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let wanted = text(p, "spectrum").ok();
    let spectrum = match wanted {
        Some(text) => cadcraft_pointcloud::Spectrum::parse(text)
            .ok_or_else(|| EngineError::Other(format!("unknown scale `{text}`: grey, bluered, viridis, turbo, magma, diverging")))?,
        None => {
            // Without a name, the next scale of the list: the palette's button walks through them.
            let current = s.clouds.get(i).map(|c| c.spectrum).unwrap_or(cadcraft_pointcloud::Spectrum::BlueRed);
            let at = cadcraft_pointcloud::Spectrum::ALL.iter().position(|s| *s == current).unwrap_or(0);
            cadcraft_pointcloud::Spectrum::ALL[(at + 1) % cadcraft_pointcloud::Spectrum::ALL.len()]
        }
    };
    let name = {
        let c = s.clouds.get_mut(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
        c.spectrum = spectrum;
        c.name.clone()
    };
    Ok(json!({
        "message": format!("{name}: the intensity is read on the {} scale", spectrum.label()),
        "index": i,
        "spectrum": spectrum.name(),
    }))
}

/// Re-read the file with a new grid cell, keeping the band, the name, the colour and the crop.
/// `cell: 0` drops the grid and keeps every point of the band (`maxPoints: 0` semantics).
fn lasdecimate(s: &mut Session, p: &Value) -> Result<Value> {
    let cell = number(p, "cell")
        .filter(|c| *c >= 0.0)
        .ok_or_else(|| EngineError::Other("`cell` must be 0 (every point) or a positive number of metres".into()))?;
    let i = cloud_index(s, p)?;
    let (path, name, z0, z1, color, clip) = {
        let c = s.clouds.get(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
        (PathBuf::from(&c.path), c.name.clone(), c.z0, c.z1, c.color, c.clip)
    };
    let decimate = (cell > 0.0).then_some(cell);
    let max_points = if decimate.is_some() { cadcraft_pointcloud::DEFAULT_MAX_POINTS } else { 0 };
    let mut cloud = cadcraft_pointcloud::Cloud::open(&path, name.clone(), z0, z1, decimate, max_points, color).map_err(oops)?;
    cloud.clip = clip;
    let points = cloud.points();
    let levels = cloud.levels.len();
    let shown = cloud.levels.first().map_or(0.0, |l| l.cell);
    s.clouds[i] = cloud;
    Ok(json!({
        "message": if shown > 0.0 {
            format!("{name}: {points} points on a {shown} m grid ({levels} levels)")
        } else {
            format!("{name}: all {points} band points ({levels} levels)")
        },
        "index": i,
        "cell": shown,
        "points": points,
        "levels": levels,
    }))
}

fn xyz(p: &Value, key: &str) -> Result<[f64; 3]> {
    let a = p.get(key).and_then(Value::as_array).filter(|a| a.len() == 3).ok_or_else(|| EngineError::Other(format!("`{key}` must be [x, y, z]")))?;
    let mut v = [0.0; 3];
    for (i, s) in a.iter().enumerate() {
        v[i] = s.as_f64().filter(|f| f.is_finite()).ok_or_else(|| EngineError::Other(format!("`{key}` must be [x, y, z]")))?;
    }
    Ok(v)
}

/// Crop the cloud to a box without re-reading the file, so the ceiling (`max` Z) or the walls
/// (`min`/`max` X and Y) can be cut away and put back while the scan stays loaded.
fn lasclip(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let reset = p.get("reset").and_then(Value::as_bool).unwrap_or(false);
    let clip = if reset {
        None
    } else {
        let min = xyz(p, "min")?;
        let max = xyz(p, "max")?;
        if (0..3).any(|k| max[k] < min[k]) {
            return Err(EngineError::Other("`max` must not be below `min` on any axis".into()));
        }
        Some(cadcraft_pointcloud::Clip { min, max })
    };
    let name = {
        let c = s.clouds.get_mut(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
        c.clip = clip;
        c.name.clone()
    };
    Ok(json!({
        "message": if clip.is_some() { format!("{name}: cropped to the box") } else { format!("{name}: crop off") },
        "index": i,
        "clip": clip.map(|c| json!({ "min": c.min, "max": c.max })),
    }))
}

/// Find the ground under a cloud, or show only the ground that was found.
///
/// The filter is the morphological one: the lowest point of every cell seeds a surface, a seed far
/// above its neighbours is dropped, the surface of every cell is a plane through its 3 × 3
/// neighbourhood, and a point on that plane is the ground. `show` switches the drawing between
/// everything and the ground alone.
fn lasground(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let defaults = cadcraft_pointcloud::GroundOptions::default();
    let options = cadcraft_pointcloud::GroundOptions {
        cell: number(p, "cell").filter(|v| *v > 0.0).unwrap_or(defaults.cell),
        tolerance: number(p, "tolerance").filter(|v| *v >= 0.0).unwrap_or(defaults.tolerance),
        bump: number(p, "bump").filter(|v| *v >= 0.0).unwrap_or(defaults.bump),
        sigma: number(p, "sigma").filter(|v| *v >= 0.0).unwrap_or(defaults.sigma),
    };
    let show = p.get("show").and_then(Value::as_bool);
    let name = s.clouds[i].name.clone();
    let result = s.clouds[i].classify_ground(&options);
    if let Some(show) = show {
        s.clouds[i].ground_only = show;
    }
    let only = s.clouds[i].ground_only;
    Ok(json!({
        "message": format!(
            "{name}: {} ground points, {} noise, {} unclassified{}",
            result.ground,
            result.noise,
            result.unclassified,
            if only { " (showing the ground only)" } else { "" }
        ),
        "index": i,
        "name": name,
        "ground": result.ground,
        "noise": result.noise,
        "unclassified": result.unclassified,
        "groundOnly": only,
        "cell": options.cell,
        "tolerance": options.tolerance,
    }))
}

/// Classify the cloud: the ground, the vegetation, the walls, the noise.
///
/// The heights are measured from the ground the filter found, so the ground filter runs first if
/// it has not run yet. The report says how many points each class holds.
fn lasclass(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let defaults = cadcraft_pointcloud::ClassifyOptions::default();
    let mode = match text(p, "mode").ok() {
        Some(t) => {
            cadcraft_pointcloud::ClassifyMode::parse(t).ok_or_else(|| EngineError::Other(format!("unknown mode `{t}`: auto, height, shape")))?
        }
        None => cadcraft_pointcloud::ClassifyMode::Auto,
    };
    let options = cadcraft_pointcloud::ClassifyOptions {
        cell: number(p, "cell").filter(|v| *v > 0.0).unwrap_or(defaults.cell),
        radius: number(p, "radius").filter(|v| *v > 0.0).unwrap_or(defaults.radius),
        sample: p.get("sample").and_then(Value::as_u64).map(|v| v.clamp(1000, 2_000_000) as usize).unwrap_or(defaults.sample),
        bands: defaults.bands,
        mode,
    };
    let name = s.clouds[i].name.clone();
    // The ground filter's word is the base of the classification.
    if !s.clouds[i].has_classes() {
        s.clouds[i].classify_ground(&cadcraft_pointcloud::GroundOptions::default());
    }
    let result = s.clouds[i].classify(&options);
    let mut counts = serde_json::Map::new();
    for code in cadcraft_pointcloud::ALL_CLASSES {
        let n = result.counts[code as usize];
        if n > 0 {
            counts.insert(cadcraft_pointcloud::class_name(code).to_string(), json!(n));
        }
    }
    Ok(json!({
        "message": format!("{name} ({}): {}", mode.name(), result.summary()),
        "index": i,
        "name": name,
        "mode": mode.name(),
        "points": result.points,
        "classes": counts,
    }))
}

/// Group the cloud's points by what they are, and name the groups.
///
/// The points are described by four things — the height above the ground, the intensity of the
/// return, the roughness of the surface and how much it stands up — and the ones that look alike
/// are grouped together (the k-means, with a fixed seed, so the same scan always gives the same
/// groups). The groups come back with a class proposed for each one; `cluster` and `class` let
/// the user name a group and the classes are given again.
fn lascluster(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let name = s.clouds[i].name.clone();
    let mut counts = serde_json::Map::new();
    // The user names one of the groups: the classes are given again, without recomputing.
    if let (Some(cluster), Some(class)) = (p.get("cluster").and_then(Value::as_u64), text(p, "class").ok()) {
        let cluster = cluster as usize;
        let class = cadcraft_pointcloud::class_from_name(class).ok_or_else(|| EngineError::Other(format!("unknown class `{class}`")))?;
        if cluster >= s.clouds[i].cluster_info.len() {
            return Err(EngineError::Other(format!("no group {cluster}: the last run found {} groups", s.clouds[i].cluster_info.len())));
        }
        s.clouds[i].cluster_info[cluster].class = class;
        s.clouds[i].apply_clusters();
        for c in cadcraft_pointcloud::ALL_CLASSES {
            let n = s.clouds[i].levels.first().map_or(0, |l| l.classes.iter().filter(|k| **k == c).count());
            if n > 0 {
                counts.insert(cadcraft_pointcloud::class_name(c).to_string(), json!(n));
            }
        }
        return Ok(json!({
            "message": format!("{name}: group {cluster} is now {}", cadcraft_pointcloud::class_name(class)),
            "index": i,
            "cluster": cluster,
            "class": cadcraft_pointcloud::class_name(class),
            "classes": counts,
        }));
    }
    // The groups themselves.
    let k = p.get("k").and_then(Value::as_u64).map(|v| v.clamp(2, 16) as usize).unwrap_or(5);
    let options = cadcraft_pointcloud::ClassifyOptions::default();
    if !s.clouds[i].has_classes() {
        s.clouds[i].classify_ground(&cadcraft_pointcloud::GroundOptions::default());
    }
    let clusters = s.clouds[i].cluster(k, &options);
    s.clouds[i].apply_clusters();
    for c in cadcraft_pointcloud::ALL_CLASSES {
        let n = s.clouds[i].levels.first().map_or(0, |l| l.classes.iter().filter(|k| **k == c).count());
        if n > 0 {
            counts.insert(cadcraft_pointcloud::class_name(c).to_string(), json!(n));
        }
    }
    let groups: Vec<Value> = clusters
        .iter()
        .map(|c| {
            let mut centre = serde_json::Map::new();
            for (k, value) in c.centre.iter().enumerate() {
                centre.insert(cadcraft_pointcloud::feature_name(k).to_string(), json!(value));
            }
            json!({
                "index": c.index,
                "count": c.count,
                "class": cadcraft_pointcloud::class_name(c.class),
                "centre": centre,
                "summary": c.summary(),
            })
        })
        .collect();
    Ok(json!({
        "message": format!("{name}: {} groups, named by the rules — rename one with `cluster` and `class`", clusters.len()),
        "index": i,
        "k": clusters.len(),
        "groups": groups,
        "classes": counts,
    }))
}

/// The site frame: the world point that sits at the drawing's (0, 0, 0).
///
/// A registered survey sits hundreds of kilometres from the origin — a factory in Romania is at
/// E 498 162, N 290 290 — so the clouds and the drawing share a local frame, and this is the tie
/// between the two. Without `x`/`y`/`z` the origin is the middle of the first cloud, with the
/// ground at zero.
fn lasorigin(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("reset").and_then(Value::as_bool).unwrap_or(false) {
        s.local_origin = [0.0; 3];
        return Ok(json!({
            "message": "the site origin is back at the drawing's (0, 0, 0)",
            "origin": s.local_origin,
        }));
    }
    let given = number(p, "x").is_some() || number(p, "y").is_some() || number(p, "z").is_some();
    let origin = if given {
        [number(p, "x").unwrap_or(s.local_origin[0]), number(p, "y").unwrap_or(s.local_origin[1]), number(p, "z").unwrap_or(s.local_origin[2])]
    } else {
        // The middle of the first cloud, with its lowest point at zero.
        match s.clouds.first() {
            Some(c) => {
                let b = c.bounds;
                [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, b.min[2]]
            }
            None => s.local_origin,
        }
    };
    s.local_origin = origin;
    Ok(json!({
        "message": format!(
            "the site origin is at {:.3}, {:.3}, {:.3}: the drawing's (0, 0, 0) is that point in the world",
            origin[0], origin[1], origin[2]
        ),
        "origin": origin,
        "clouds": s.clouds.len(),
    }))
}

/// The coordinate reference system of the project, for the files that do not name one.
///
/// A scan that says nothing is not a scan without a system: the survey office knows. The code is
/// remembered with the drawing and written next to an exported drawing. `falseEasting` and
/// `falseNorthing` are the local grid's own offsets: a survey office that writes the northing
/// without its leading millions needs them, or the export lands kilometres away.
fn lascrs(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("reset").and_then(Value::as_bool).unwrap_or(false) {
        s.crs = None;
        return Ok(json!({ "message": "the project's CRS is forgotten", "crs": Value::Null }));
    }
    let epsg = p.get("epsg").and_then(Value::as_u64).map(|v| v as u32);
    let name = p.get("name").and_then(Value::as_str).map(str::to_string);
    let false_easting = number(p, "falseEasting").unwrap_or(0.0);
    let false_northing = number(p, "falseNorthing").unwrap_or(0.0);
    if epsg.is_none() && name.is_none() {
        return Err(EngineError::Other("`epsg` or `name` is required".into()));
    }
    let crs = cadcraft_pointcloud::Crs { epsg, wkt: name, false_easting, false_northing };
    s.crs = Some(crs.clone());
    Ok(json!({
        "message": match (crs.epsg, crs.name()) {
            (Some(code), Some(name)) => format!("the project is in EPSG:{code} ({name})"),
            (Some(code), None) => format!("the project is in EPSG:{code}"),
            _ => "the project's CRS is set".to_string(),
        },
        "crs": crs_json(&Some(crs)),
    }))
}

/// How opaque a cloud is drawn.
///
/// A dense cloud hides the drawing under it. With the opacity down, the 3D view thins the points
/// by a deterministic mask (the x-ray), so a wall seen edge-on does not become a solid slab of
/// pixels, and the lines drawn over the cloud stay readable.
fn lasopacity(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let opacity = match number(p, "opacity") {
        Some(v) => v,
        None => {
            // Without a value the opacity toggles between opaque and half.
            let current = s.clouds[i].opacity;
            if current > 0.6 { 0.35 } else { 1.0 }
        }
    };
    s.clouds[i].set_opacity(opacity as f32);
    let opacity = s.clouds[i].opacity;
    Ok(json!({
        "message": format!("{}: the cloud is drawn at {:.0}%", s.clouds[i].name, opacity * 100.0),
        "index": i,
        "opacity": opacity,
    }))
}

/// Tie the project to a point on the map.
///
/// A survey office says "this scan is at 44.1124° N, 24.9746° E"; the app turns that into the
/// project's grid (Stereo 70, an UTM zone) and works out the local grid's own offsets: the file's
/// coordinates minus the standard grid of that point. The offsets go into the WKT written next to
/// an exported drawing, so the export lands where the survey office says it does, and the drawing's
/// (0, 0, 0) moves to the declared point.
fn lasgeo(s: &mut Session, p: &Value) -> Result<Value> {
    let lat = number(p, "lat").ok_or_else(|| EngineError::Other("`lat` is required".into()))?;
    let lon = number(p, "lon").ok_or_else(|| EngineError::Other("`lon` is required".into()))?;
    let epsg = s.crs.as_ref().and_then(|c| c.epsg).ok_or_else(|| EngineError::Other("the project's CRS is not set: LASCRS {epsg} first".into()))?;
    let grid = cadcraft_pointcloud::grid_from_lat_lon(epsg, cadcraft_pointcloud::LatLon { lat, lon })
        .ok_or_else(|| EngineError::Other(format!("EPSG:{epsg} has no projection the fork knows")))?;
    let Some(cloud) = s.clouds.first() else {
        return Err(EngineError::Other("no cloud is open: the reference point is the middle of the first one".into()));
    };
    // The reference: the middle of the first cloud, in the file's own coordinates.
    let b = cloud.bounds;
    let reference = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, b.min[2]];
    // The local grid's offsets: the file's coordinates minus the standard grid of the same point.
    let mut crs = s.crs.clone().unwrap_or_default();
    crs.false_easting = reference[0] - grid[0];
    crs.false_northing = reference[1] - grid[1];
    s.crs = Some(crs.clone());
    // The drawing's (0, 0, 0) is the declared point, which is the middle of the cloud.
    s.local_origin = reference;
    Ok(json!({
        "message": format!(
            "the project is at {lat:.7}, {lon:.7} — EPSG:{epsg} {:.2}, {:.2}; the local grid is E {:+.2} N {:+.2}",
            grid[0], grid[1], crs.false_easting, crs.false_northing
        ),
        "lat": lat,
        "lon": lon,
        "epsg": epsg,
        "grid": grid,
        "reference": reference,
        "origin": s.local_origin,
        "crs": crs_json(&Some(crs)),
    }))
}

/// Compare two clouds: the distance from every point of one to the nearest point of the other.
///
/// The comparison a survey office makes after a second visit: the tested cloud against a
/// reference, with the statistics of the distances (the mean, the standard deviation, the RMS, the
/// extremes and the 95th percentile) and the cloud drawn on a blue-to-red deviation scale.
fn lascmp(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let reference = p.get("reference").and_then(Value::as_u64).map(|v| v as usize).unwrap_or_else(|| {
        // Without a reference, the other cloud: the last one that is not the tested one.
        s.clouds.iter().enumerate().rev().find(|(j, _)| *j != i).map(|(j, _)| j).unwrap_or(i)
    });
    if reference >= s.clouds.len() {
        return Err(EngineError::Other(format!("no reference cloud {reference}; {} open", s.clouds.len())));
    }
    if reference == i {
        return Err(EngineError::Other("the reference must be another cloud".into()));
    }
    let cell = number(p, "cell").unwrap_or(0.0);
    // The reference's points are taken out of the session, not copied out of it: a scan of a
    // hundred million points would not fit twice.
    let reference_points = s.clouds[reference].world_points();
    let cell = s.clouds[reference].search_cell(cell);
    let stats = s.clouds[i].compare_with_points(&reference_points, cell);
    s.clouds[i].color = cadcraft_pointcloud::CloudColor::Deviation;
    let name = s.clouds[i].name.clone();
    let reference_name = s.clouds[reference].name.clone();
    Ok(json!({
        "message": format!("{name} against {reference_name}: {}", stats.summary()),
        "index": i,
        "reference": reference,
        "referenceName": reference_name,
        "count": stats.count,
        "missing": stats.missing,
        "mean": stats.mean,
        "std": stats.std,
        "rms": stats.rms,
        "min": stats.min,
        "max": stats.max,
        "p95": stats.p95,
        "scale": s.clouds[i].dev_scale,
    }))
}

/// Show or hide a cloud; without `visible` it toggles.
fn lasvisible(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let want = p.get("visible").and_then(Value::as_bool);
    let c = s.clouds.get_mut(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
    c.visible = want.unwrap_or(!c.visible);
    Ok(json!({ "index": i, "visible": c.visible, "name": c.name }))
}

/// Frame a cloud's XY bounds in the drawing view (2D).
fn lasframe(s: &mut Session, p: &Value) -> Result<Value> {
    let i = cloud_index(s, p)?;
    let (name, b) = {
        let c = s.clouds.get(i).ok_or_else(|| EngineError::Other(format!("no point cloud {i}")))?;
        (c.name.clone(), (c.bounds.min, c.bounds.max))
    };
    let (w, h) = (b.1[0] - b.0[0], b.1[1] - b.0[1]);
    // The site frame: the cloud is drawn where the drawing is.
    let center = Vec2::new((b.0[0] + b.1[0]) / 2.0 - s.local_origin[0], (b.0[1] + b.1[1]) / 2.0 - s.local_origin[1]);
    let height = w.max(h).max(1e-6) * 1.15;
    let mut framed = false;
    if let Ok(st) = s.state_mut() {
        st.set_view(View { center, height });
        framed = true;
    }
    Ok(json!({ "index": i, "name": name, "framed": framed, "center": [center.x, center.y], "height": height }))
}

fn point3(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    let mut p = [0.0; 3];
    for (i, s) in a.iter().enumerate() {
        p[i] = s.as_f64().filter(|f| f.is_finite())?;
    }
    Some(p)
}

/// Add one 3D polyline to the session's sketch (the 3D view calls this when a stroke is finished).
fn draw3d(s: &mut Session, p: &Value) -> Result<Value> {
    let a = p.get("points").and_then(Value::as_array).ok_or_else(|| EngineError::Other("`points` must be an array of [x, y, z]".into()))?;
    if a.is_empty() {
        return Err(EngineError::Other("`points` needs at least one vertex".into()));
    }
    if a.len() > MAX_STROKE_VERTICES {
        return Err(EngineError::Other(format!("a 3D stroke holds at most {MAX_STROKE_VERTICES} vertices, {} asked", a.len())));
    }
    let mut pts = Vec::with_capacity(a.len());
    for (i, v) in a.iter().enumerate() {
        let pt = point3(v).ok_or_else(|| EngineError::Other(format!("`points[{i}]` must be [x, y, z] with finite numbers")))?;
        pts.push(pt);
    }
    let layer = p.get("layer").and_then(Value::as_str).unwrap_or(cadcraft_pointcloud::DEFAULT_LAYER);
    let closed = p.get("closed").and_then(Value::as_bool).unwrap_or(false);
    let vertices = s.sketch.add(&pts, layer, closed);
    Ok(json!({
        "message": format!("3D stroke: {vertices} vertices on layer {layer}{} ({} strokes, {} segments)", if closed { ", closed" } else { "" }, s.sketch.count(), s.sketch.segments()),
        "vertices": vertices,
        "strokes": s.sketch.count(),
        "segments": s.sketch.segments(),
        "layer": layer,
        "closed": closed,
    }))
}

/// How a click in the 3D view picks its point: `exact` (the cloud point itself) or `smooth` (the
/// average of the points around it, which cancels the scan noise).
fn snap3d(s: &mut Session, p: &Value) -> Result<Value> {
    let text = text(p, "mode")?;
    let mode = cadcraft_pointcloud::SnapMode::parse(text).ok_or_else(|| EngineError::Other(format!("unknown snap mode `{text}`: smooth, exact")))?;
    s.snap3d = mode;
    Ok(json!({
        "message": format!("3D snap: {}", mode.name()),
        "mode": mode.name(),
    }))
}

/// Measure a segment between two points: its length, its run in plan, its rise and its slope.
fn measure3d(p: &Value) -> Result<Value> {
    let p1 = p.get("p1").and_then(point3).ok_or_else(|| EngineError::Other("`p1` must be [x, y, z]".into()))?;
    let p2 = p.get("p2").and_then(point3).ok_or_else(|| EngineError::Other("`p2` must be [x, y, z]".into()))?;
    let m = cadcraft_pointcloud::measure(p1, p2);
    Ok(json!({
        "message": cadcraft_pointcloud::readout(&m),
        "dist_3d": m.length,
        "dist_2d_xy": m.flat,
        "dx": m.dx,
        "dy": m.dy,
        "dz": m.dz,
        "slope_deg": m.slope_deg,
        "slope_pct": m.slope_pct,
    }))
}

fn sketch_clear(s: &mut Session) -> Result<Value> {
    let n = s.sketch.clear();
    Ok(json!({
        "message": if n == 0 { "the 3D sketch is empty".to_string() } else { format!("{n} 3D strokes cleared") },
        "cleared": n,
        "strokes": 0,
    }))
}

fn sketch_undo(s: &mut Session) -> Result<Value> {
    let removed = s.sketch.undo();
    Ok(json!({
        "message": match removed {
            Some(n) => format!("3D stroke undone ({n} vertices)"),
            None => "the 3D sketch is empty".to_string(),
        },
        "undone": removed.is_some(),
        "strokes": s.sketch.count(),
    }))
}

fn sketch_list(s: &mut Session) -> Result<Value> {
    let strokes: Vec<Value> = s
        .sketch
        .strokes()
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let first = st.points.first().copied().unwrap_or([0.0; 3]);
            let last = st.points.last().copied().unwrap_or([0.0; 3]);
            json!({
                "index": i,
                "vertices": st.points.len(),
                "first": first,
                "last": last,
                "layer": st.layer,
                "closed": st.closed,
            })
        })
        .collect();
    Ok(json!({
        "strokes": strokes,
        "count": strokes.len(),
        "vertices": s.sketch.points(),
        "segments": s.sketch.segments(),
    }))
}

/// Write the 3D sketch as DXF LINE entities, Z included, so it survives the session and can be
/// traced over in the 2D drawing. Every stroke keeps its own layer; an explicit `layer` writes
/// them all on that one instead.
fn sketch_export(s: &mut Session, p: &Value) -> Result<Value> {
    let out = PathBuf::from(text(p, "path")?);
    if s.sketch.is_empty() {
        return Err(EngineError::Other("the 3D sketch is empty: draw with 3DLINE first".into()));
    }
    let forced = p.get("layer").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty());
    let strokes: Vec<cadcraft_pointcloud::Stroke3> = match forced {
        Some(layer) => s
            .sketch
            .strokes()
            .iter()
            .map(|st| {
                let mut copy = st.clone();
                copy.layer = layer.to_string();
                copy
            })
            .collect(),
        None => s.sketch.strokes().to_vec(),
    };
    let written = cadcraft_pointcloud::write_strokes_dxf(&out, &strokes, "3D-SKETCH").map_err(oops)?;
    let mut layers: Vec<&str> = strokes.iter().map(|st| st.layer.as_str()).collect();
    layers.sort_unstable();
    layers.dedup();
    Ok(json!({
        "message": format!("{written} entities written to {} on {}", out.display(), layers.join(", ")),
        "path": out.display().to_string(),
        "layer": layers.first().copied().unwrap_or("3D-SKETCH"),
        "layers": layers,
        "entities": written,
        "strokes": s.sketch.count(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_with_nothing_open_reports_zero() {
        let mut s = Session::new();
        let r = lasclose(&mut s, &json!({ "all": true })).ok();
        assert_eq!(r.and_then(|v| v["closed"].as_u64()), Some(0));
        assert!(s.clouds.is_empty());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let mut s = Session::new();
        let err = lasopen(&mut s, &json!({ "path": "no-such-cloud.las" })).err();
        assert!(err.is_some_and(|e| e.to_string().contains("no-such-cloud.las")));
        assert!(s.clouds.is_empty());
    }

    #[test]
    fn lasclouds_lists_what_is_open() {
        let mut s = Session::new();
        let r = lasclouds(&mut s).ok();
        assert_eq!(r.and_then(|v| v["count"].as_u64()), Some(0));
    }

    #[test]
    fn clip_sets_a_box_and_resets() {
        use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, Pt, Slice};
        let mut b = Bounds3::EMPTY;
        b.add(0.0, 0.0, 0.0);
        b.add(2.0, 2.0, 3.0);
        let pts = vec![Pt { x: 1.0, y: 1.0, z: 1.0, rgb: None, intensity: 0, class: 1 }];
        let slice = Slice { points: pts, total: 1, in_band: 1, bounds: b };
        let mut s = Session::new();
        s.clouds.push(Cloud::from_slice("t".into(), String::new(), 0.0, 3.0, None, slice, 1000, CloudColor::Rgb));
        assert!(lasclip(&mut s, &json!({"min": [0.0, 0.0, 0.0], "max": [2.0, 2.0, 1.5]})).is_ok());
        assert_eq!(s.clouds[0].clip.map(|c| c.max[2]), Some(1.5), "the ceiling is cut at Z 1.5");
        assert!(lasclip(&mut s, &json!({"min": [0.0, 0.0, 2.0], "max": [1.0, 1.0, 1.0]})).is_err(), "a box with max below min is refused");
        assert!(lasclip(&mut s, &json!({"reset": true})).is_ok());
        assert!(s.clouds[0].clip.is_none());
    }

    #[test]
    fn the_intensity_scale_is_the_user_s_choice() {
        use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, Pt, Slice};
        let mut b = Bounds3::EMPTY;
        b.add(0.0, 0.0, 0.0);
        b.add(1.0, 1.0, 1.0);
        let pts = vec![Pt { x: 0.5, y: 0.5, z: 0.5, rgb: None, intensity: 100, class: 1 }];
        let slice = Slice { points: pts, total: 1, in_band: 1, bounds: b };
        let mut s = Session::new();
        s.clouds.push(Cloud::from_slice("t".into(), String::new(), 0.0, 1.0, None, slice, 1000, CloudColor::Intensity));
        let r = lpectrum(&mut s, &json!({ "spectrum": "viridis" })).ok();
        assert_eq!(r.as_ref().and_then(|v| v["spectrum"].as_str()), Some("viridis"));
        assert_eq!(s.clouds[0].spectrum, cadcraft_pointcloud::Spectrum::Viridis);
        // Without a name, the scale walks to the next one.
        lpectrum(&mut s, &json!({})).ok();
        assert_eq!(s.clouds[0].spectrum, cadcraft_pointcloud::Spectrum::Turbo);
        // An unknown scale is an error, not a panic and not a silent change.
        let before = s.clouds[0].spectrum;
        assert!(lpectrum(&mut s, &json!({ "spectrum": "nope" })).is_err());
        assert_eq!(s.clouds[0].spectrum, before);
    }

    #[test]
    fn the_cloud_is_classified_and_the_report_counts_the_classes() {
        use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, Pt, Slice};
        let mut b = Bounds3::EMPTY;
        b.add(0.0, 0.0, 0.0);
        b.add(2.0, 2.0, 4.0);
        // A floor and a wall above it.
        let mut pts: Vec<Pt> = Vec::new();
        for i in 0..30 {
            for j in 0..30 {
                pts.push(Pt { x: i as f64 * 0.1, y: j as f64 * 0.1, z: 0.0, rgb: None, intensity: 100, class: 1 });
            }
        }
        for i in 0..30 {
            for k in 0..20 {
                pts.push(Pt { x: i as f64 * 0.1, y: 0.0, z: 2.5 + k as f64 * 0.1, rgb: None, intensity: 100, class: 1 });
            }
        }
        let n = pts.len() as u64;
        let slice = Slice { points: pts, total: n, in_band: n, bounds: b };
        let mut s = Session::new();
        s.clouds.push(Cloud::from_slice("t".into(), String::new(), 0.0, 4.0, Some(0.05), slice, 1000, CloudColor::Rgb));
        let r = lasclass(&mut s, &json!({ "mode": "auto" })).ok();
        let classes = r.as_ref().and_then(|v| v["classes"].as_object()).cloned();
        assert!(classes.is_some(), "the report counts the classes: {r:?}");
        let classes = classes.unwrap_or_default();
        assert!(classes.contains_key("ground"), "the floor is the ground: {classes:?}");
        assert!(classes.contains_key("building"), "the wall is a building: {classes:?}");
        // An unknown mode is an error, not a panic.
        assert!(lasclass(&mut s, &json!({ "mode": "nope" })).is_err());
    }

    #[test]
    fn the_groups_are_named_by_the_user() {
        use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, Pt, Slice};
        let mut b = Bounds3::EMPTY;
        b.add(0.0, 0.0, 0.0);
        b.add(2.0, 2.0, 4.0);
        // A floor and a wall above it.
        let mut pts: Vec<Pt> = Vec::new();
        for i in 0..30 {
            for j in 0..30 {
                pts.push(Pt { x: i as f64 * 0.1, y: j as f64 * 0.1, z: 0.0, rgb: None, intensity: 100, class: 1 });
            }
        }
        for i in 0..30 {
            for k in 0..20 {
                pts.push(Pt { x: i as f64 * 0.1, y: 0.0, z: 2.5 + k as f64 * 0.1, rgb: None, intensity: 200, class: 1 });
            }
        }
        let n = pts.len() as u64;
        let slice = Slice { points: pts, total: n, in_band: n, bounds: b };
        let mut s = Session::new();
        s.clouds.push(Cloud::from_slice("t".into(), String::new(), 0.0, 4.0, Some(0.05), slice, 1000, CloudColor::Rgb));
        let r = lascluster(&mut s, &json!({ "k": 3 })).ok();
        let groups = r.as_ref().and_then(|v| v["groups"].as_array()).cloned();
        assert!(groups.as_ref().is_some_and(|g| !g.is_empty()), "the groups come back: {r:?}");
        let groups = groups.unwrap_or_default();
        assert_eq!(groups.len(), 3);
        // The user names one of the groups: the classes are given again.
        let named = lascluster(&mut s, &json!({ "cluster": 0, "class": "building" })).ok();
        assert_eq!(named.as_ref().and_then(|v| v["class"].as_str()), Some("building"));
        assert_eq!(s.clouds[0].cluster_info[0].class, cadcraft_pointcloud::CLASS_BUILDING);
        // A group that is not there is an error, not a panic.
        assert!(lascluster(&mut s, &json!({ "cluster": 99, "class": "building" })).is_err());
        assert!(lascluster(&mut s, &json!({ "cluster": 0, "class": "nonsense" })).is_err());
    }

    #[test]
    fn a_3d_stroke_is_stored_listed_and_undone() {
        let mut s = Session::new();
        let r = draw3d(&mut s, &json!({"points": [[0.0, 0.0, 1.5], [2.0, 0.0, 1.5], [2.0, 2.0, 1.5]]})).ok();
        assert_eq!(r.as_ref().and_then(|v| v["vertices"].as_u64()), Some(3));
        assert_eq!(s.sketch.segments(), 2, "three vertices make two segments");
        let list = sketch_list(&mut s).ok();
        assert_eq!(list.as_ref().and_then(|v| v["count"].as_u64()), Some(1));
        assert_eq!(list.as_ref().and_then(|v| v["segments"].as_u64()), Some(2));
        assert_eq!(sketch_undo(&mut s).ok().and_then(|v| v["undone"].as_bool()), Some(true));
        assert!(s.sketch.is_empty());
        assert_eq!(sketch_undo(&mut s).ok().and_then(|v| v["undone"].as_bool()), Some(false), "undo on empty is a message, not an error");
        draw3d(&mut s, &json!({"points": [[1.0, 1.0, 1.0]]})).ok();
        assert_eq!(sketch_clear(&mut s).ok().and_then(|v| v["cleared"].as_u64()), Some(1));
        assert_eq!(s.sketch.points(), 0);
    }

    #[test]
    fn hostile_3d_points_are_refused_without_storing_anything() {
        let mut s = Session::new();
        for bad in
            [json!({}), json!({"points": []}), json!({"points": [[0.0, 0.0]]}), json!({"points": [[0.0, "x", 1.0]]}), json!({"points": "not a list"})]
        {
            assert!(draw3d(&mut s, &bad).is_err(), "{bad}");
            assert!(s.sketch.is_empty(), "{bad}");
        }
    }

    #[test]
    fn the_sketch_exports_as_dxf_and_needs_strokes_first() {
        let mut s = Session::new();
        let dir = std::env::temp_dir().join("cadcraft-3d-test");
        std::fs::create_dir_all(&dir).ok();
        let out = dir.join("sketch.dxf");
        assert!(sketch_export(&mut s, &json!({"path": out.to_string_lossy()})).is_err(), "nothing to export yet");
        draw3d(&mut s, &json!({"points": [[0.0, 0.0, 0.5], [1.0, 1.0, 0.5]]})).ok();
        let r = sketch_export(&mut s, &json!({"path": out.to_string_lossy()})).ok();
        assert_eq!(r.as_ref().and_then(|v| v["entities"].as_u64()), Some(1));
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        assert!(text.contains("LINE"), "{text}");
        assert!(text.contains(cadcraft_pointcloud::DEFAULT_LAYER), "the stroke keeps its own layer: {text}");

        // An explicit layer writes the whole sketch on that one.
        let out2 = dir.join("sketch-layer.dxf");
        sketch_export(&mut s, &json!({"path": out2.to_string_lossy(), "layer": "WALLS"})).ok();
        let text = std::fs::read_to_string(&out2).unwrap_or_default();
        assert!(text.contains("WALLS"), "{text}");
        assert!(!text.contains(cadcraft_pointcloud::DEFAULT_LAYER), "{text}");
    }

    #[test]
    fn the_opacity_is_clamped_and_toggles() {
        use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, Pt, Slice};
        let mut b = Bounds3::EMPTY;
        b.add(0.0, 0.0, 0.0);
        b.add(2.0, 2.0, 3.0);
        let slice = Slice { points: vec![Pt { x: 1.0, y: 1.0, z: 1.0, rgb: None, intensity: 0, class: 1 }], total: 1, in_band: 1, bounds: b };
        let mut s = Session::new();
        s.clouds.push(Cloud::from_slice("t".into(), String::new(), 0.0, 3.0, None, slice, 1000, CloudColor::Rgb));
        assert_eq!(s.clouds[0].opacity, 1.0, "opaque by default");
        let r = lasopacity(&mut s, &json!({ "opacity": 0.3 })).ok();
        assert_eq!(r.as_ref().and_then(|v| v["opacity"].as_f64()), Some(0.3f32 as f64));
        lasopacity(&mut s, &json!({ "opacity": 5.0 })).ok();
        assert_eq!(s.clouds[0].opacity, 1.0, "above 1 is opaque");
        lasopacity(&mut s, &json!({ "opacity": 0.0 })).ok();
        assert_eq!(s.clouds[0].opacity, 0.05, "below 5% is not drawn at all");
        lasopacity(&mut s, &json!({ "opacity": f64::NAN })).ok();
        assert!(s.clouds[0].opacity.is_finite(), "a broken number changes nothing");
        assert!(s.clouds[0].opacity >= 0.05 && s.clouds[0].opacity <= 1.0, "and stays in range");
    }

    #[test]
    fn only_las_and_laz_are_cloud_paths() {
        assert!(is_cloud_path("scan.las"));
        assert!(is_cloud_path("scan.LAZ"));
        assert!(!is_cloud_path("scan.dxf"));
        assert!(!is_cloud_path("scan"));
    }

    /// Typing `OPEN scan.las` used to end in IO's "unsupported file type": it must reach LASOPEN,
    /// whose own error names the file it could not read.
    #[test]
    fn open_hands_las_and_laz_to_lasopen() {
        for name in ["no-such-cloud.las", "no-such-cloud.LAZ"] {
            let mut s = Session::new();
            let error = s.execute("open", &json!({ "path": name })).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(error.contains(name), "{error}");
            assert!(!error.contains("unsupported file type"), "{error}");
            assert!(s.clouds.is_empty());
        }
    }

    /// A stroke remembers its layer and whether it is closed, and the export keeps both.
    #[test]
    fn a_stroke_keeps_its_layer_and_its_closing() {
        let mut s = Session::new();
        let r = draw3d(&mut s, &json!({"points": [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]], "layer": "WALLS", "closed": true})).ok();
        assert_eq!(r.as_ref().and_then(|v| v["closed"].as_bool()), Some(true));
        assert_eq!(r.as_ref().and_then(|v| v["layer"].as_str()), Some("WALLS"));
        assert_eq!(s.sketch.segments(), 3, "two sides and the closing one");
        let list = sketch_list(&mut s).ok();
        let first = list.as_ref().map(|v| v["strokes"][0].clone());
        assert_eq!(first.as_ref().and_then(|v| v["layer"].as_str()), Some("WALLS"));
        assert_eq!(first.as_ref().and_then(|v| v["closed"].as_bool()), Some(true));

        let dir = std::env::temp_dir().join("cadcraft-3d-test");
        std::fs::create_dir_all(&dir).ok();
        let out = dir.join("layers.dxf");
        let r = sketch_export(&mut s, &json!({"path": out.to_string_lossy()})).ok();
        assert_eq!(r.as_ref().and_then(|v| v["entities"].as_u64()), Some(3));
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        assert!(text.contains("WALLS"), "{text}");
    }

    /// Without a layer a stroke lands on the default one.
    #[test]
    fn a_stroke_without_a_layer_gets_the_default_one() {
        let mut s = Session::new();
        draw3d(&mut s, &json!({"points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]})).ok();
        assert_eq!(s.sketch.strokes()[0].layer, cadcraft_pointcloud::DEFAULT_LAYER);
    }

    #[test]
    fn the_snap_mode_is_read_set_and_refused_when_unknown() {
        let mut s = Session::new();
        assert_eq!(s.snap3d, cadcraft_pointcloud::SnapMode::Smooth, "smooth by default");
        let r = snap3d(&mut s, &json!({"mode": "exact"})).ok();
        assert_eq!(r.as_ref().and_then(|v| v["mode"].as_str()), Some("exact"));
        assert_eq!(s.snap3d, cadcraft_pointcloud::SnapMode::Exact);
        assert!(snap3d(&mut s, &json!({"mode": "nope"})).is_err());
        assert!(snap3d(&mut s, &json!({})).is_err(), "the mode is required");
        assert_eq!(s.snap3d, cadcraft_pointcloud::SnapMode::Exact, "a refused mode changes nothing");
    }

    #[test]
    fn measuring_reports_the_length_the_run_and_the_slope() {
        let r = measure3d(&json!({"p1": [0.0, 0.0, 0.0], "p2": [3.0, 4.0, 12.0]})).ok();
        assert!((r.as_ref().and_then(|v| v["dist_3d"].as_f64()).unwrap_or(0.0) - 13.0).abs() < 1e-9);
        assert!((r.as_ref().and_then(|v| v["dist_2d_xy"].as_f64()).unwrap_or(0.0) - 5.0).abs() < 1e-9);
        assert_eq!(r.as_ref().and_then(|v| v["dx"].as_f64()), Some(3.0));
        assert_eq!(r.as_ref().and_then(|v| v["dy"].as_f64()), Some(4.0));
        assert_eq!(r.as_ref().and_then(|v| v["dz"].as_f64()), Some(12.0));
        assert!((r.as_ref().and_then(|v| v["slope_deg"].as_f64()).unwrap_or(0.0) - 67.3801).abs() < 0.001);
        assert!((r.as_ref().and_then(|v| v["slope_pct"].as_f64()).unwrap_or(0.0) - 240.0).abs() < 1e-9);
    }

    #[test]
    fn a_vertical_measurement_has_no_percentage() {
        let r = measure3d(&json!({"p1": [0.0, 0.0, 0.0], "p2": [0.0, 0.0, 2.5]})).ok();
        assert!((r.as_ref().and_then(|v| v["slope_deg"].as_f64()).unwrap_or(0.0) - 90.0).abs() < 1e-9);
        assert!(r.as_ref().and_then(|v| v["slope_pct"].as_f64()).is_none());
    }

    #[test]
    fn hostile_measure_points_are_refused() {
        for bad in [
            json!({}),
            json!({"p1": [0.0, 0.0, 0.0]}),
            json!({"p1": [0.0, 0.0], "p2": [1.0, 1.0, 1.0]}),
            json!({"p1": [0.0, "x", 0.0], "p2": [1.0, 1.0, 1.0]}),
        ] {
            assert!(measure3d(&bad).is_err(), "{bad}");
        }
    }
}
