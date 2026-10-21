//! Point cloud commands: LASINFO, LASEXPORT and the LASOPEN canvas overlay.
//!
//! M1 was stateless: read a LAS/LAZ file, cut a Z band, write a DXF. M2 keeps the band in the
//! session ([`Session::clouds`]) so the canvas draws it under the drawing and the user traces
//! over it without a second file.

use std::path::PathBuf;

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
            .params("{path: string, z0?: number, z1?: number, decimate?: number, name?: string}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("lasclose", "Close Point Cloud", lasclose).params("{index?: int, all?: bool}").enabled(always).noundo(),
        CommandSpec::new("lasclouds", "List Point Clouds", |s, _| lasclouds(s)).params("{}").enabled(always).noundo(),
    ]
}

fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or_else(|| EngineError::Other(format!("`{key}` is required")))
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

/// Keep a band in the session for the canvas. The default 2 cm grid is the drafting setting that
/// keeps walls readable and the frame rate honest; pass `decimate: 0.005` (or 0) for everything.
fn lasopen(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(text(p, "path")?);
    let info = cadcraft_pointcloud::read_info(&path).map_err(oops)?;
    let z0 = number(p, "z0").unwrap_or(info.bounds.min[2]);
    let z1 = number(p, "z1").unwrap_or(info.bounds.max[2]);
    let decimate = number(p, "decimate").filter(|v| *v > 0.0).or(Some(0.02));
    let slice = cadcraft_pointcloud::slice(&path, z0, z1, decimate).map_err(oops)?;
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("Point cloud").to_string());
    let b = slice.bounds;
    let count = slice.points.len();
    // First cloud in an empty drawing: frame it, so LASOPEN is all it takes to see the scan.
    if s.doc().is_ok_and(|d| d.model.is_empty()) {
        let w = b.max[0] - b.min[0];
        let h = b.max[1] - b.min[1];
        let center = Vec2::new((b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0);
        if let Ok(st) = s.state_mut() {
            st.set_view(View { center, height: w.max(h).max(1.0) * 1.2 });
        }
    }
    s.clouds.push(cadcraft_pointcloud::Cloud {
        name: name.clone(),
        path: path.display().to_string(),
        points: slice.points,
        bounds: b,
        visible: true,
    });
    Ok(json!({
        "message": format!("{name}: {count} of {} points in the band", slice.in_band),
        "name": name,
        "points": count,
        "inBand": slice.in_band,
        "filePoints": slice.total,
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
                "points": c.points.len(),
                "visible": c.visible,
                "bounds": { "min": c.bounds.min, "max": c.bounds.max },
            })
        })
        .collect();
    Ok(json!({ "count": list.len(), "clouds": list }))
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
}
