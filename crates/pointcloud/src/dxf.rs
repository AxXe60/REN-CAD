//! Writing a slice as a DXF drawing of POINT entities (R2000, ASCII).
//!
//! Deliberately small: a header that says "metres", one layer, and one POINT per point. CADCraft
//! reads its own output, AutoCAD reads it, and the user can trace over it and save the trace as
//! an ordinary drawing.

use std::path::Path;

use cadcraft_dxf::{Tag, Value, write_ascii};

use crate::sketch::Stroke3;
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

/// The true colour of the 3D sketch in exported drawings: warm orange, so it reads over a grey
/// cloud when the DXF is opened next to the trace.
pub const SKETCH_RGB: i64 = 0x00F2_7A1A;

/// A layer name DXF accepts: trimmed, without the characters it reserves (`< > / \ " : ; ? * | = '`)
/// and not longer than 255.
fn sanitize_layer(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if matches!(c, '<' | '>' | '/' | '\\' | '"' | ':' | ';' | '?' | '*' | '|' | '=' | '\'') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() { "0".to_string() } else { cleaned.chars().take(255).collect() }
}

/// Write 3D strokes as LINE entities (a lone vertex becomes a POINT), keeping their Z and their
/// layer: every stroke is written on the layer it was drawn on, and a closed stroke gets its
/// closing segment too. Returns the number of entities written.
pub fn write_strokes_dxf(path: &Path, strokes: &[Stroke3], layer: &str) -> Result<usize> {
    let fallback = sanitize_layer(if layer.trim().is_empty() { "3D-SKETCH" } else { layer });
    // The layer table holds layer 0 and every layer the strokes use.
    let mut names: Vec<String> = vec!["0".to_string()];
    for stroke in strokes {
        let name = sanitize_layer(if stroke.layer.trim().is_empty() { &fallback } else { &stroke.layer });
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let mut tags: Vec<Tag> = Vec::with_capacity(64 + strokes.iter().map(|s| s.points.len()).sum::<usize>() * 12);

    // HEADER: R2000, metres.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "HEADER");
    s(&mut tags, 9, "$ACADVER");
    s(&mut tags, 1, "AC1015");
    s(&mut tags, 9, "$INSUNITS");
    tags.push(Tag::new(70, Value::Int(6)));
    s(&mut tags, 0, "ENDSEC");

    // TABLES: layer 0 and the sketch layers.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "TABLES");
    s(&mut tags, 0, "TABLE");
    s(&mut tags, 2, "LAYER");
    tags.push(Tag::new(70, Value::Int(names.len() as i64)));
    for name in &names {
        s(&mut tags, 0, "LAYER");
        s(&mut tags, 2, name);
        tags.push(Tag::new(70, Value::Int(0)));
        tags.push(Tag::new(62, Value::Int(7)));
        s(&mut tags, 6, "CONTINUOUS");
    }
    s(&mut tags, 0, "ENDTAB");
    s(&mut tags, 0, "ENDSEC");

    // ENTITIES: one LINE per segment, one POINT for a stroke of a single vertex.
    s(&mut tags, 0, "SECTION");
    s(&mut tags, 2, "ENTITIES");
    let mut written = 0usize;
    for stroke in strokes {
        let name = sanitize_layer(if stroke.layer.trim().is_empty() { &fallback } else { &stroke.layer });
        let pts: Vec<&[f64; 3]> = stroke.points.iter().filter(|p| p.iter().all(|v| v.is_finite())).collect();
        match pts.as_slice() {
            [] => {}
            [p] => {
                s(&mut tags, 0, "POINT");
                s(&mut tags, 8, &name);
                tags.push(Tag::new(10, Value::Real(p[0])));
                tags.push(Tag::new(20, Value::Real(p[1])));
                tags.push(Tag::new(30, Value::Real(p[2])));
                tags.push(Tag::new(420, Value::Int(SKETCH_RGB)));
                written += 1;
            }
            _ => {
                let mut segments: Vec<(&[f64; 3], &[f64; 3])> = pts.windows(2).map(|w| (w[0], w[1])).collect();
                if stroke.closed {
                    let (last, first) = (pts[pts.len() - 1], pts[0]);
                    if last != first {
                        segments.push((last, first));
                    }
                }
                for (a, b) in segments {
                    s(&mut tags, 0, "LINE");
                    s(&mut tags, 8, &name);
                    tags.push(Tag::new(10, Value::Real(a[0])));
                    tags.push(Tag::new(20, Value::Real(a[1])));
                    tags.push(Tag::new(30, Value::Real(a[2])));
                    tags.push(Tag::new(11, Value::Real(b[0])));
                    tags.push(Tag::new(21, Value::Real(b[1])));
                    tags.push(Tag::new(31, Value::Real(b[2])));
                    tags.push(Tag::new(420, Value::Int(SKETCH_RGB)));
                    written += 1;
                }
            }
        }
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
        let pts = [
            Pt { x: 1.0, y: 2.0, z: 3.0, rgb: Some([10, 20, 30]), intensity: 0, class: 1 },
            Pt { x: -1.5, y: 0.25, z: 1.75, rgb: None, intensity: 0, class: 1 },
        ];
        let n = write_points_dxf(&path, &pts, "PC-SLICE").unwrap_or(0);
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains("POINT"));
        assert!(text.contains("PC-SLICE"));
        assert!(text.contains("420"));
    }

    #[test]
    fn writes_a_line_per_segment_with_its_z() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("sketch.dxf");
        let strokes = vec![
            Stroke3::new(vec![[0.0, 0.0, 2.5], [3.0, 0.0, 2.5], [3.0, 4.0, 2.5]], "3D-SKETCH", false),
            Stroke3::new(vec![[9.0, 9.0, 1.0]], "3D-SKETCH", false),
        ];
        let n = write_strokes_dxf(&path, &strokes, "3D-SKETCH").unwrap_or(0);
        assert_eq!(n, 3, "two segments and one lone vertex");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(text.matches("LINE").count(), 2);
        assert_eq!(text.matches("POINT").count(), 1);
        assert!(text.contains("3D-SKETCH"));
        assert!(text.contains("2.5"), "the stroke keeps its height: {text}");
    }

    /// Every stroke is written on its own layer, and the layer table lists them all.
    #[test]
    fn every_stroke_is_written_on_its_own_layer() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("sketch-layers.dxf");
        let strokes = vec![
            Stroke3::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], "WALLS", false),
            Stroke3::new(vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]], "FLOOR", false),
        ];
        let n = write_strokes_dxf(&path, &strokes, "3D-SKETCH").unwrap_or(0);
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains("WALLS"), "{text}");
        assert!(text.contains("FLOOR"), "{text}");
        assert!(!text.contains("3D-SKETCH"), "the fallback layer is not used when a stroke has one: {text}");
        assert_eq!(text.matches("LAYER").count(), 4, "layer 0 plus the two stroke layers: {text}");
    }

    /// A closed stroke is written with the segment that joins its last vertex to its first.
    #[test]
    fn a_closed_stroke_gets_its_closing_segment() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("sketch-closed.dxf");
        let strokes = vec![Stroke3::new(vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]], "ROOM", true)];
        let n = write_strokes_dxf(&path, &strokes, "3D-SKETCH").unwrap_or(0);
        assert_eq!(n, 3, "two sides and the closing one");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(text.matches("LINE").count(), 3, "{text}");
    }

    #[test]
    fn non_finite_sketch_vertices_are_skipped() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("sketch-bad.dxf");
        let strokes = vec![Stroke3::new(vec![[0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0]], "S", false), Stroke3::new(vec![[f64::INFINITY; 3]], "S", false)];
        assert_eq!(write_strokes_dxf(&path, &strokes, "S").unwrap_or(9), 1, "the finite vertex survives as a POINT; the rest is dropped");
    }

    /// A layer name with characters DXF reserves is cleaned up instead of breaking the file.
    #[test]
    fn layer_names_are_cleaned_up() {
        let dir = std::env::temp_dir().join("cadcraft-pc-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("sketch-bad-layer.dxf");
        let strokes = vec![Stroke3::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], "A/B:C*D", false)];
        assert_eq!(write_strokes_dxf(&path, &strokes, "S").unwrap_or(9), 1);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains("A_B_C_D"), "{text}");
        assert!(!text.contains("A/B"), "{text}");
    }
}
