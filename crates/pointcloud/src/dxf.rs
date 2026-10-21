//! Writing a slice as a DXF drawing of POINT entities (R2000, ASCII).
//!
//! Deliberately small: a header that says "metres", one layer, and one POINT per point. CADCraft
//! reads its own output, AutoCAD reads it, and the user can trace over it and save the trace as
//! an ordinary drawing.

use std::path::Path;

use cadcraft_dxf::{Tag, Value, write_ascii};

use crate::{Error, Pt, Result};

/// Append a string-valued tag (`0`, `2`, `8`, `9`, `1`, `6` …).
fn s(tags: &mut Vec<Tag>, code: i32, text: &str) {
    tags.push(Tag::new(code, Value::Str(text.to_string())));
}

/// Write `points` to `path` on `layer`. Returns the number of POINT entities written.
pub fn write_points_dxf(path: &Path, points: &[Pt], layer: &str) -> Result<usize> {
    let layer = if layer.trim().is_empty() { "PC-SLICE" } else { layer.trim() };
    let mut tags: Vec<Tag> = Vec::with_capacity(64 + points.len() * 8);

    // HEADER: R2000, metres.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "HEADER");
    s(&mut tags, 9, "$ACADVER");
    s(&mut tags, 1, "AC1015");
    s(&mut tags, 9, "$INSUNITS");
    tags.push(Tag::new(70, Value::Int(6)));
    s(&mut tags, 0, "ENDSEC");

    // TABLES: layer 0 and the slice layer.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "TABLES");
    s(&mut tags, 0, "TABLE");
    s(&mut tags, 2, "LAYER");
    tags.push(Tag::new(70, Value::Int(2)));
    for (name, color) in [("0", 7), (layer, 7)] {
        s(&mut tags, 0, "LAYER");
        s(&mut tags, 2, name);
        tags.push(Tag::new(70, Value::Int(0)));
        tags.push(Tag::new(62, Value::Int(color)));
        s(&mut tags, 6, "CONTINUOUS");
    }
    s(&mut tags, 0, "ENDTAB");
    s(&mut tags, 0, "ENDSEC");

    // ENTITIES: one POINT per point, true colour when the cloud has it.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "ENTITIES");
    let mut written = 0usize;
    for p in points {
        if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
            continue;
        }
        s(&mut tags, 0, "POINT");
        s(&mut tags, 8, layer);
        tags.push(Tag::new(10, Value::Real(p.x)));
        tags.push(Tag::new(20, Value::Real(p.y)));
        tags.push(Tag::new(30, Value::Real(p.z)));
        match p.rgb {
            Some([r, g, b]) => {
                let rgb = (i64::from(r) << 16) | (i64::from(g) << 8) | i64::from(b);
                tags.push(Tag::new(420, Value::Int(rgb)));
            }
            None => tags.push(Tag::new(62, Value::Int(7))),
        }
        written += 1;
    }
    s(&mut tags, 0, "ENDSEC");
    s(&mut tags, 0, "EOF");

    std::fs::write(path, write_ascii(&tags)).map_err(|e| Error::Io { path: path.display().to_string(), source: e })?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_point_per_input() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("slice.dxf");
        let pts = [Pt { x: 1.0, y: 2.0, z: 3.0, rgb: Some([10, 20, 30]), intensity: 0 }, Pt { x: -1.5, y: 0.25, z: 1.75, rgb: None, intensity: 0 }];
        let n = write_points_dxf(&path, &pts, "PC-SLICE").unwrap_or(0);
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains("POINT"));
        assert!(text.contains("PC-SLICE"));
        assert!(text.contains("420"));
    }
}
