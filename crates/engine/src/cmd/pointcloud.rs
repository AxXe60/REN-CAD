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
        CommandSpec::new("lasexport", "Export Point Cloud Slice", |_, p| lasexport(p))
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
        CommandSpec::new("lasframe", "Frame Point Cloud", lasframe).params("{index?: int}").enabled(always).noundo(),
        CommandSpec::new("lasclose", "Close Point Cloud", lasclose).params("{index?: int, all?: bool}").enabled(always).noundo(),
        CommandSpec::new("lasclouds", "List Point Clouds", |s, _| lasclouds(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dline", "3D Line", draw3d).params("{points: [[x,y,z], ...]}").enabled(always).noundo(),
        CommandSpec::new("3dclear", "Clear 3D Sketch", |s, _| sketch_clear(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dundo", "Undo 3D Stroke", |s, _| sketch_undo(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dlist", "List 3D Sketch", |s, _| sketch_list(s)).params("{}").enabled(always).noundo(),
        CommandSpec::new("3dexport", "Export 3D Sketch", sketch_export).params("{path: string, layer?: string}").enabled(always).noundo(),
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
    }))
}

fn lasexport(p: &Value) -> Result<Value> {
    let path = PathBuf::from(text(p, "path")?);
    let out = PathBuf::from(text(p, "out")?);
    let info = cadcraft_pointcloud::read_info(&path).map_err(oops)?;
    let z0 = number(p, "z0").unwrap_or(info.bounds.min[2]);
    let z1 = number(p, "z1").unwrap_or(info.bounds.max[2]);
    let decimate = number(p, "decimate").filter(|v| *v > 0.0);
    let layer = p.get("layer").and_then(Value::as_str).unwrap_or("PC-SLICE").to_string();

    let slice = cadcraft_pointcloud::slice(&path, z0, z1, decimate).map_err(oops)?;
    let written = cadcraft_pointcloud::write_points_dxf(&out, &slice.points, &layer).map_err(oops)?;
    Ok(json!({
        "message": format!("{} of {} points in the band written to {}", written, slice.in_band, out.display()),
        "path": out.display().to_string(),
        "layer": layer,
        "points": written,
        "inBand": slice.in_band,
        "filePoints": slice.total,
        "bounds": { "min": slice.bounds.min, "max": slice.bounds.max },
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
    let center = Vec2::new((b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0);
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
    let vertices = s.sketch.add(&pts);
    Ok(json!({
        "message": format!("3D stroke: {vertices} vertices ({} strokes, {} segments)", s.sketch.count(), s.sketch.segments()),
        "vertices": vertices,
        "strokes": s.sketch.count(),
        "segments": s.sketch.segments(),
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
            let first = st.first().copied().unwrap_or([0.0; 3]);
            let last = st.last().copied().unwrap_or([0.0; 3]);
            json!({ "index": i, "vertices": st.len(), "first": first, "last": last })
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
/// traced over in the 2D drawing.
fn sketch_export(s: &mut Session, p: &Value) -> Result<Value> {
    let out = PathBuf::from(text(p, "path")?);
    if s.sketch.is_empty() {
        return Err(EngineError::Other("the 3D sketch is empty: draw with 3DLINE first".into()));
    }
    let layer = p.get("layer").and_then(Value::as_str).unwrap_or("3D-SKETCH");
    let written = cadcraft_pointcloud::write_strokes_dxf(&out, s.sketch.strokes(), layer).map_err(oops)?;
    Ok(json!({
        "message": format!("{written} entities written to {} on layer {layer}", out.display()),
        "path": out.display().to_string(),
        "layer": layer,
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
        let pts = vec![Pt { x: 1.0, y: 1.0, z: 1.0, rgb: None, intensity: 0 }];
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
        assert!(text.contains("LINE") && text.contains("3D-SKETCH"));
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
}
