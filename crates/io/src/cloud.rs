//! The point clouds a file carries.
//!
//! A cloud is written as ordinary `POINT` entities on a layer of its own, so any program reads it
//! (AutoCAD included: they are points), and a header variable says what they are, so RENCAD
//! recognises them again as the cloud they were:
//!
//! ```text
//! $RENCAD_CLOUDS
//! [{"name":"Ap 1","layer":"PCLOUD-Ap 1","source":"D:/scan/Ap 1.las","crs":null,"color":"rgb"}]
//! ```
//!
//! The points themselves are the entities; the variable carries the rest.

use cadcraft_color::Color;
use cadcraft_doc::{CLOUD_HEADER_VAR, Drawing, EntityKind, PointCloud};

/// The description of the clouds, as the header variable carries it.
///
/// The points are not in it: they are the entities on the layers. What is here is what cannot be
/// read off an entity — the name, where the cloud came from, its coordinate system, how it is
/// coloured.
pub fn header_value(clouds: &[PointCloud]) -> String {
    let mut out = String::from("[");
    for (i, c) in clouds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":{},\"layer\":{},\"source\":{},\"crs\":{},\"color\":{},\"points\":{}}}",
            quoted(&c.name),
            quoted(&c.layer),
            quoted(&c.source),
            c.crs.as_ref().map_or_else(|| "null".to_string(), |s| quoted(s)),
            quoted(&c.color),
            c.points.len()
        ));
    }
    out.push(']');
    out
}

/// A JSON string, quoted and escaped.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The clouds a header variable describes.
///
/// The points are not here: they are the `POINT` entities on the layers the description names.
pub fn from_header(value: &str) -> Vec<PointCloud> {
    let mut out = Vec::new();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) else { return out };
    let Some(list) = parsed.as_array() else { return out };
    for item in list {
        let Some(obj) = item.as_object() else { continue };
        let name = obj.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        let layer = obj.get("layer").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        if name.is_empty() && layer.is_empty() {
            continue;
        }
        out.push(PointCloud {
            name,
            layer,
            source: obj.get("source").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
            crs: obj.get("crs").and_then(serde_json::Value::as_str).map(str::to_string),
            color: obj.get("color").and_then(serde_json::Value::as_str).unwrap_or("rgb").to_string(),
            ..PointCloud::default()
        });
    }
    out
}

/// The name of the header variable the clouds are described in.
pub const VAR: &str = CLOUD_HEADER_VAR;

/// The description of the clouds, in pieces small enough for a DXF string.
pub fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > 200 {
        let (head, tail) = rest.split_at(200);
        out.push(head.to_string());
        rest = tail;
    }
    out.push(rest.to_string());
    out
}

/// Take the clouds out of a drawing: the points on the clouds' layers become the clouds, and the
/// entities are removed, so the drawing does not carry them twice.
pub fn take_from(drawing: &mut Drawing) -> Vec<PointCloud> {
    let Some(value) = drawing.header.get(VAR).and_then(cadcraft_doc::HVal::as_str).map(str::to_string) else {
        return Vec::new();
    };
    let mut clouds = from_header(&value);
    if clouds.is_empty() {
        return clouds;
    }
    for cloud in clouds.iter_mut() {
        let layer = cloud.layer.clone();
        let handles: Vec<cadcraft_doc::Handle> = drawing
            .model
            .iter()
            .filter(|e| e.common.layer.eq_ignore_ascii_case(&layer))
            .filter_map(|e| match &e.kind {
                EntityKind::Point(_) => Some(e.handle),
                _ => None,
            })
            .collect();
        for h in handles {
            let Some(e) = drawing.model.remove(h) else { continue };
            let EntityKind::Point(p) = &e.kind else { continue };
            cloud.points.push([p.p.x, p.p.y, p.p.z]);
            if let Color::True(rgb) = e.common.color {
                cloud.colors.push([rgb.0, rgb.1, rgb.2]);
            }
        }
        // The colours are there for every point or for none: the cloud is read as one thing.
        if cloud.colors.len() != cloud.points.len() {
            cloud.colors.clear();
        }
    }
    clouds.retain(|c| !c.points.is_empty());
    clouds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_survives_the_round_trip() {
        let mut c = PointCloud::new("Ap 1", "PCLOUD-Ap 1");
        c.source = "D:/scan/Ap 1.las".into();
        c.crs = Some("EPSG:31700".into());
        c.color = "intensity".into();
        c.points.push([1.0, 2.0, 3.0]);
        let value = header_value(std::slice::from_ref(&c));
        let back = from_header(&value);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "Ap 1");
        assert_eq!(back[0].layer, "PCLOUD-Ap 1");
        assert_eq!(back[0].source, "D:/scan/Ap 1.las");
        assert_eq!(back[0].crs.as_deref(), Some("EPSG:31700"));
        assert_eq!(back[0].color, "intensity");
    }

    #[test]
    fn a_broken_value_gives_no_clouds() {
        assert!(from_header("not json").is_empty());
        assert!(from_header("{}").is_empty());
        assert!(from_header("[1,2]").is_empty());
    }

    #[test]
    fn the_quotes_are_escaped() {
        let mut c = PointCloud::new("a \"b\"", "L");
        c.source = "c:\\d".into();
        let value = header_value(std::slice::from_ref(&c));
        let back = from_header(&value);
        assert_eq!(back[0].name, "a \"b\"");
        assert_eq!(back[0].source, "c:\\d");
    }

    /// The whole way round: a cloud is written into a drawing and read back out of it.
    #[test]
    fn a_cloud_survives_the_file() {
        let mut cloud = PointCloud::new("Ap 1", "PCLOUD-Ap 1");
        cloud.source = "D:/scan/Ap 1.las".into();
        cloud.color = "intensity".into();
        cloud.points = vec![[0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        cloud.colors = vec![[10, 20, 30], [40, 50, 60], [70, 80, 90]];
        let drawing = Drawing::default();
        let bytes = crate::write_clouds(&drawing, std::slice::from_ref(&cloud), "c.dxf").expect("the file is written");
        let (back, clouds) = crate::read_clouds(&bytes, "c.dxf").expect("the file is read");
        assert_eq!(clouds.len(), 1, "the cloud comes back: {clouds:?}");
        assert_eq!(clouds[0].name, "Ap 1");
        assert_eq!(clouds[0].layer, "PCLOUD-Ap 1");
        assert_eq!(clouds[0].source, "D:/scan/Ap 1.las");
        assert_eq!(clouds[0].color, "intensity");
        assert_eq!(clouds[0].points.len(), 3, "every point is there");
        assert_eq!(clouds[0].points[2], [4.0, 5.0, 6.0]);
        assert_eq!(clouds[0].colors.len(), 3, "the colours come back too");
        assert_eq!(clouds[0].colors[1], [40, 50, 60]);
        // The points are the cloud's, not loose entities in the drawing.
        assert!(back.model.is_empty(), "the drawing holds no loose points: {}", back.model.len());
    }

    #[test]
    fn a_drawing_without_clouds_is_left_alone() {
        let drawing = Drawing::default();
        let bytes = crate::write(&drawing, "c.dxf").expect("the file is written");
        let (back, clouds) = crate::read_clouds(&bytes, "c.dxf").expect("the file is read");
        assert!(clouds.is_empty());
        assert!(back.model.is_empty());
    }

    /// The DWG is what the user asked for: the cloud has to be in it too.
    #[test]
    fn a_cloud_survives_the_dwg() {
        let mut cloud = PointCloud::new("Ap 1", "PCLOUD-Ap 1");
        cloud.points = vec![[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]];
        cloud.colors = vec![[10, 20, 30], [40, 50, 60]];
        let drawing = Drawing::default();
        let bytes = crate::write_clouds(&drawing, std::slice::from_ref(&cloud), "c.dwg").expect("the file is written");
        assert!(cadcraft_dwg::is_dwg(&bytes), "it is a DWG");
        // What the DWG bridge gives back: the description must be in it.
        let (dxf, _) = cadcraft_dwg::dwg_to_dxf_noted(&bytes, cadcraft_dwg::Limits::DEFAULT).expect("the DWG is read back");
        let dxf = String::from_utf8_lossy(&dxf).to_string();
        assert!(
            dxf.contains("CADCRAFT_CLOUDS"),
            "the description survives the DWG: xrecords={} clouds={}",
            dxf.contains("XRECORD"),
            dxf.contains("RENCAD_CLOUDS")
        );
        let (back, clouds) = crate::read_clouds(&bytes, "c.dwg").expect("the file is read");
        assert_eq!(clouds.len(), 1, "the cloud comes back from the DWG: {clouds:?}");
        assert_eq!(clouds[0].name, "Ap 1");
        assert_eq!(clouds[0].points.len(), 2, "every point is there");
        assert_eq!(clouds[0].points[1], [1.0, 2.0, 3.0]);
        assert!(back.model.is_empty(), "the drawing holds no loose points");
    }
}
