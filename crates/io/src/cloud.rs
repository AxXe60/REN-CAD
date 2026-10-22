//! The point clouds a drawing refers to.
//!
//! A cloud is not in the drawing: a scan holds millions of points, and a file that carries them
//! all is a file nobody can send. What the drawing carries is the *reference*, and an `XRECORD`
//! of the named object dictionary describes it, so RENCAD loads the scan again when the drawing
//! is opened:
//!
//! ```text
//! $RENCAD_CLOUDS
//! [{"name":"Ap 1","path":"D:/scan/Ap 1.las","z0":2.04,"z1":2.2,"cell":0.05,"color":"rgb","spectrum":"bluered","crs":null,"offset":[498162,290290,0]}]
//! ```
//!
//! The reference is what travels in the file; the scan stays where it was. The `XRECORD` and not
//! a header variable, because the DWG keeps the records and drops the variables it does not know.

use cadcraft_doc::{CLOUD_HEADER_VAR, Drawing, PointCloud};

/// The description of the clouds, as the header variable carries it.
///
/// The points are not in it: they are in the scan on the disk. What is here is the reference, and
/// the parts of it that cannot be read off the drawing: the name, the file the scan is in, the
/// band of Z, the grid, the colour, the coordinate system and the place.
pub fn header_value(clouds: &[PointCloud]) -> String {
    let mut out = String::from("[");
    for (i, c) in clouds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":{},\"path\":{},\"z0\":{},\"z1\":{},\"cell\":{},\"color\":{},\"spectrum\":{},\"crs\":{},\"offset\":[{},{},{}]}}",
            quoted(&c.name),
            quoted(&c.path),
            num(c.z0),
            num(c.z1),
            num(c.cell),
            quoted(&c.color),
            quoted(&c.spectrum),
            c.crs.as_ref().map_or_else(|| "null".to_string(), |s| quoted(s)),
            num(c.offset[0]),
            num(c.offset[1]),
            num(c.offset[2]),
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

/// A JSON number for the value: something endless becomes a zero, so the description stays text
/// a reader can parse back.
fn num(x: f64) -> String {
    if x.is_finite() { format!("{x}") } else { "0".to_string() }
}

/// The clouds a header variable describes: the references, read back the way they were written.
///
/// A value that is not the description gives no clouds, and never a panic: the description is
/// hostile input, and a hostile number is a zero.
pub fn from_header(value: &str) -> Vec<PointCloud> {
    let mut out = Vec::new();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) else { return out };
    let Some(list) = parsed.as_array() else { return out };
    for item in list {
        let Some(obj) = item.as_object() else { continue };
        let name = obj.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        let path = obj.get("path").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        if name.is_empty() && path.is_empty() {
            continue;
        }
        let place = obj.get("offset").and_then(serde_json::Value::as_array);
        out.push(PointCloud {
            name,
            path,
            z0: f64_of(obj.get("z0")),
            z1: f64_of(obj.get("z1")),
            cell: f64_of(obj.get("cell")),
            color: obj.get("color").and_then(serde_json::Value::as_str).unwrap_or("rgb").to_string(),
            spectrum: obj.get("spectrum").and_then(serde_json::Value::as_str).unwrap_or("bluered").to_string(),
            crs: obj.get("crs").and_then(serde_json::Value::as_str).map(str::to_string),
            offset: [f64_at(place, 0), f64_at(place, 1), f64_at(place, 2)],
        });
    }
    out
}

/// A number of the description, or zero when it is not one.
fn f64_of(value: Option<&serde_json::Value>) -> f64 {
    value.and_then(serde_json::Value::as_f64).unwrap_or_default()
}

/// The `i`th number of the place, or zero when the place has no such number.
fn f64_at(place: Option<&Vec<serde_json::Value>>, i: usize) -> f64 {
    f64_of(place.and_then(|p| p.get(i)))
}

/// The name of the header variable the clouds are described in.
pub const VAR: &str = CLOUD_HEADER_VAR;

/// The description of the clouds, in pieces small enough for a DXF string.
///
/// The split never cuts a character in half: a path with a letter outside the ASCII in it (a
/// Romanian name, a Chinese one) must not become an invalid byte in the file.
pub fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > 200 {
        let mut cut = 200;
        // Move the cut back to the start of the character it lands in.
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (head, tail) = rest.split_at(cut);
        out.push(head.to_string());
        rest = tail;
    }
    out.push(rest.to_string());
    out
}

/// Take the clouds out of a drawing: the references the header variable carries.
///
/// The drawing does not hold the points any more, so there is nothing to remove: what comes back
/// is the reference, and the scan is loaded from the disk when the drawing is opened.
pub fn take_from(drawing: &mut Drawing) -> Vec<PointCloud> {
    let Some(value) = drawing.header.get(VAR).and_then(cadcraft_doc::HVal::as_str).map(str::to_string) else {
        return Vec::new();
    };
    from_header(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole description goes round: every field of the reference comes back.
    #[test]
    fn the_description_survives_the_round_trip() {
        let mut c = PointCloud::new("Ap 1", "D:/scan/Ap 1.las");
        c.z0 = 2.04;
        c.z1 = 2.2;
        c.cell = 0.05;
        c.color = "intensity".into();
        c.spectrum = "viridis".into();
        c.crs = Some("EPSG:31700".into());
        c.offset = [498_162.0, 290_290.0, 0.0];
        let value = header_value(std::slice::from_ref(&c));
        let back = from_header(&value);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "Ap 1");
        assert_eq!(back[0].path, "D:/scan/Ap 1.las");
        assert_eq!(back[0].z0, 2.04);
        assert_eq!(back[0].z1, 2.2);
        assert_eq!(back[0].cell, 0.05);
        assert_eq!(back[0].color, "intensity");
        assert_eq!(back[0].spectrum, "viridis");
        assert_eq!(back[0].crs.as_deref(), Some("EPSG:31700"));
        assert_eq!(back[0].offset, [498_162.0, 290_290.0, 0.0]);
    }

    #[test]
    fn a_broken_value_gives_no_clouds() {
        assert!(from_header("not json").is_empty());
        assert!(from_header("{}").is_empty());
        assert!(from_header("[1,2]").is_empty());
        // The numbers are hostile input too: what is not a number is a zero, never a panic.
        let hostile = from_header(r#"[{"name":"n","z0":"not a number","offset":[1,"x",true]}]"#);
        assert_eq!(hostile[0].z0, 0.0);
        assert_eq!(hostile[0].offset, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn the_quotes_are_escaped() {
        let c = PointCloud::new("a \"b\"", "c:\\d");
        let value = header_value(std::slice::from_ref(&c));
        let back = from_header(&value);
        assert_eq!(back[0].name, "a \"b\"");
        assert_eq!(back[0].path, "c:\\d");
    }

    /// A path with a letter outside the ASCII in it must not be cut in half.
    #[test]
    fn the_pieces_never_cut_a_character_in_half() {
        let c = PointCloud::new("Curte Ulei", "D:/scan/съемка.las");
        let value = header_value(std::slice::from_ref(&c));
        for piece in chunks(&value) {
            assert!(piece.is_char_boundary(0), "the piece is whole text");
        }
        let back = from_header(&value);
        assert_eq!(back[0].path, "D:/scan/съемка.las");
    }

    /// The whole way round: a drawing written with the reference reads back with the same
    /// reference, and the file carries no points of the cloud.
    #[test]
    fn a_cloud_survives_the_file() {
        let mut cloud = PointCloud::new("Ap 1", "D:/scan/Ap 1.las");
        cloud.z0 = 2.04;
        cloud.z1 = 2.2;
        cloud.cell = 0.05;
        cloud.color = "intensity".into();
        cloud.spectrum = "magma".into();
        cloud.crs = Some("EPSG:31700".into());
        cloud.offset = [498_162.0, 290_290.0, 0.0];
        let drawing = Drawing::default();
        let bytes = crate::write_clouds(&drawing, std::slice::from_ref(&cloud), "c.dxf").expect("the file is written");
        let (back, clouds) = crate::read_clouds(&bytes, "c.dxf").expect("the file is read");
        assert_eq!(clouds.len(), 1, "the reference comes back: {clouds:?}");
        assert_eq!(clouds[0].name, "Ap 1");
        assert_eq!(clouds[0].path, "D:/scan/Ap 1.las");
        assert_eq!(clouds[0].z0, 2.04);
        assert_eq!(clouds[0].z1, 2.2);
        assert_eq!(clouds[0].cell, 0.05);
        assert_eq!(clouds[0].color, "intensity");
        assert_eq!(clouds[0].spectrum, "magma");
        assert_eq!(clouds[0].crs.as_deref(), Some("EPSG:31700"));
        assert_eq!(clouds[0].offset, [498_162.0, 290_290.0, 0.0]);
        // The scan is not in the file any more: the drawing carries the reference only.
        assert!(back.model.is_empty(), "the file holds no points of the cloud: {}", back.model.len());
        assert!(back.layers.iter().all(|l| !l.name.starts_with("PCLOUD-")), "the cloud has no layer of its own");
    }

    #[test]
    fn a_drawing_without_clouds_is_left_alone() {
        let drawing = Drawing::default();
        let bytes = crate::write(&drawing, "c.dxf").expect("the file is written");
        let (back, clouds) = crate::read_clouds(&bytes, "c.dxf").expect("the file is read");
        assert!(clouds.is_empty());
        assert!(back.model.is_empty());
    }

    /// The DWG is what the user asked for: the reference has to be in it too (the DWG keeps the
    /// XRECORD, a header variable alone would not survive).
    #[test]
    fn a_cloud_survives_the_dwg() {
        let mut cloud = PointCloud::new("Ap 1", "D:/scan/Ap 1.las");
        cloud.cell = 0.05;
        cloud.color = "elevation".into();
        cloud.offset = [10.0, 20.0, 30.0];
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
        assert_eq!(clouds.len(), 1, "the reference comes back from the DWG: {clouds:?}");
        assert_eq!(clouds[0].name, "Ap 1");
        assert_eq!(clouds[0].path, "D:/scan/Ap 1.las");
        assert_eq!(clouds[0].color, "elevation");
        assert_eq!(clouds[0].offset, [10.0, 20.0, 30.0]);
        assert!(back.model.is_empty(), "the file holds no points of the cloud");
    }
}
