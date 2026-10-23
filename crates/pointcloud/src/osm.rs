//! OpenStreetMap (`.osm` XML & Overpass API JSON) clean-room parser and CRS projector.
//!
//! Extracts buildings (`building=*`), postal numbers (`addr:housenumber`, `addr:street`,
//! `addr:postcode`, `addr:city`), roads (`highway=*`), parcels (`landuse=*`, `boundary=*`) and
//! utility poles (`power=pole`) and projects them into the active coordinate reference system
//! (e.g. Stereo 70 `EPSG:31700`, UTM, ETRS89, Web Mercator).

use std::collections::HashMap;

use crate::geo::{LatLon, grid_from_lat_lon};

/// A building footprint extracted from OpenStreetMap, projected into drawing coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct OsmBuilding {
    pub id: u64,
    pub name: Option<String>,
    pub street: Option<String>,
    pub house_number: Option<String>,
    pub levels: u32,
    pub height_m: f64,
    pub polygon: Vec<[f64; 2]>,
}

/// A postal address / house number label extracted from OpenStreetMap.
#[derive(Clone, Debug, PartialEq)]
pub struct OsmAddress {
    pub id: u64,
    pub house_number: String,
    pub street: String,
    pub postcode: Option<String>,
    pub city: Option<String>,
    pub position: [f64; 2],
}

/// A road centerline extracted from OpenStreetMap (`highway=*`).
#[derive(Clone, Debug, PartialEq)]
pub struct OsmRoad {
    pub id: u64,
    pub kind: String,
    pub name: Option<String>,
    pub width_m: f64,
    pub points: Vec<[f64; 2]>,
}

/// A parcel / land-use polygon extracted from OpenStreetMap (`landuse=*`, `boundary=*`).
#[derive(Clone, Debug, PartialEq)]
pub struct OsmParcel {
    pub id: u64,
    pub category: String,
    pub label: Option<String>,
    pub polygon: Vec<[f64; 2]>,
}

/// A utility pole / street lamp node extracted from OpenStreetMap.
#[derive(Clone, Debug, PartialEq)]
pub struct OsmPole {
    pub id: u64,
    pub kind: String,
    pub position: [f64; 2],
}

/// Complete OpenStreetMap dataset projected into the drawing's coordinate reference system.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsmDataset {
    pub epsg: u32,
    pub origin_offset: [f64; 2],
    pub buildings: Vec<OsmBuilding>,
    pub addresses: Vec<OsmAddress>,
    pub roads: Vec<OsmRoad>,
    pub parcels: Vec<OsmParcel>,
    pub poles: Vec<OsmPole>,
}

/// Build an Overpass QL query for a geographic bounding box `(min_lat, min_lon, max_lat, max_lon)`
/// retrieving buildings, postal numbers (`addr:housenumber`), roads, parcels and utility poles.
pub fn build_overpass_query(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Option<String> {
    if !min_lat.is_finite() || !min_lon.is_finite() || !max_lat.is_finite() || !max_lon.is_finite() {
        return None;
    }
    if min_lat >= max_lat || min_lon >= max_lon {
        return None;
    }
    Some(format!(
        "[out:json][timeout:25];(\
way[\"building\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
node[\"addr:housenumber\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
way[\"addr:housenumber\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
way[\"highway\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
way[\"landuse\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
node[\"power\"=\"pole\"]({min_lat:.6},{min_lon:.6},{max_lat:.6},{max_lon:.6});\
);out body;>;out skel qt;"
    ))
}

/// Project a `(lat, lon)` coordinate into the target EPSG code (defaulting to Stereo 70 `31700`
/// when `epsg == 0` or unsupported, or Web Mercator `3857` / WGS84 metric local tangent).
pub fn project_lat_lon(epsg: u32, lat: f64, lon: f64) -> Option<[f64; 2]> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    if let Some(xy) = grid_from_lat_lon(epsg, LatLon { lat, lon }) {
        return Some(xy);
    }
    match epsg {
        3857 => {
            let r = 6_378_137.0;
            let x = lon.to_radians() * r;
            let clamped = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
            let y = r * ((std::f64::consts::FRAC_PI_4 + clamped * 0.5).tan()).ln();
            Some([x, y])
        }
        4326 | 4258 | 4269 => Some([lon, lat]),
        _ => grid_from_lat_lon(31700, LatLon { lat, lon }),
    }
}

/// Parse an Overpass API JSON response (`elements: [...]`) and project all features into `epsg`.
pub fn parse_overpass_json(json_text: &str, epsg: u32) -> Result<OsmDataset, String> {
    let root: serde_json::Value = serde_json::from_str(json_text).map_err(|e| format!("invalid Overpass JSON: {e}"))?;
    let elements =
        root.get("elements").and_then(serde_json::Value::as_array).ok_or_else(|| "missing `elements` array in Overpass JSON".to_string())?;

    let mut nodes: HashMap<u64, [f64; 2]> = HashMap::new();
    let mut origin_offset: Option<[f64; 2]> = None;

    for el in elements {
        if el.get("type").and_then(serde_json::Value::as_str) == Some("node") {
            let Some(id) = el.get("id").and_then(serde_json::Value::as_u64) else { continue };
            let Some(lat) = el.get("lat").and_then(serde_json::Value::as_f64) else { continue };
            let Some(lon) = el.get("lon").and_then(serde_json::Value::as_f64) else { continue };
            let Some(xy) = project_lat_lon(epsg, lat, lon) else { continue };
            let off = *origin_offset.get_or_insert(xy);
            nodes.insert(id, [xy[0] - off[0], xy[1] - off[1]]);
        }
    }

    let mut dataset = OsmDataset { epsg, origin_offset: origin_offset.unwrap_or([0.0, 0.0]), ..OsmDataset::default() };

    for el in elements {
        let Some(id) = el.get("id").and_then(serde_json::Value::as_u64) else { continue };
        let el_type = el.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
        let Some(tags) = el.get("tags").and_then(serde_json::Value::as_object) else { continue };
        let tag_str = |k: &str| tags.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

        if el_type == "node" {
            let Some(&pos) = nodes.get(&id) else { continue };
            if let Some(hn) = tag_str("addr:housenumber") {
                dataset.addresses.push(OsmAddress {
                    id,
                    house_number: hn,
                    street: tag_str("addr:street").unwrap_or_else(|| "Stradă".to_string()),
                    postcode: tag_str("addr:postcode"),
                    city: tag_str("addr:city"),
                    position: pos,
                });
            }
            if tag_str("power").as_deref() == Some("pole") || tag_str("highway").as_deref() == Some("street_lamp") {
                dataset.poles.push(OsmPole {
                    id,
                    kind: tag_str("power").or_else(|| tag_str("highway")).unwrap_or_else(|| "pole".to_string()),
                    position: pos,
                });
            }
        } else if el_type == "way" {
            let Some(nd_list) = el.get("nodes").and_then(serde_json::Value::as_array) else { continue };
            let mut pts: Vec<[f64; 2]> = Vec::with_capacity(nd_list.len());
            for nd in nd_list {
                if let Some(nid) = nd.as_u64()
                    && let Some(&pos) = nodes.get(&nid)
                {
                    pts.push(pos);
                }
            }
            if pts.len() < 2 {
                continue;
            }
            if tags.contains_key("building") {
                let levels = tag_str("building:levels").and_then(|s| s.parse::<u32>().ok()).map(|l| l.clamp(1, 80)).unwrap_or(2);
                let height_m = tag_str("height")
                    .and_then(|s| s.trim_end_matches('m').trim().parse::<f64>().ok())
                    .filter(|h| h.is_finite() && *h > 1.5)
                    .unwrap_or(f64::from(levels) * 3.1);
                let house_number = tag_str("addr:housenumber");
                let street = tag_str("addr:street");
                if let Some(ref hn) = house_number {
                    let centroid = polygon_centroid(&pts);
                    dataset.addresses.push(OsmAddress {
                        id,
                        house_number: hn.clone(),
                        street: street.clone().unwrap_or_else(|| "Stradă".to_string()),
                        postcode: tag_str("addr:postcode"),
                        city: tag_str("addr:city"),
                        position: centroid,
                    });
                }
                dataset.buildings.push(OsmBuilding { id, name: tag_str("name"), street, house_number, levels, height_m, polygon: pts });
            } else if let Some(hw) = tag_str("highway") {
                let width_m = match hw.as_str() {
                    "primary" | "trunk" => 11.0,
                    "secondary" => 9.0,
                    "tertiary" | "residential" => 7.0,
                    "service" | "living_street" => 4.5,
                    _ => 3.5,
                };
                dataset.roads.push(OsmRoad { id, kind: hw, name: tag_str("name"), width_m, points: pts });
            } else if let Some(lu) = tag_str("landuse").or_else(|| tag_str("boundary")) {
                dataset.parcels.push(OsmParcel { id, category: lu, label: tag_str("name").or_else(|| tag_str("ref")), polygon: pts });
            }
        }
    }

    Ok(dataset)
}

/// Parse a local `.osm` XML string into an `OsmDataset` projected into `epsg`.
pub fn parse_osm_xml(xml: &str, epsg: u32) -> Result<OsmDataset, String> {
    let mut nodes: HashMap<u64, [f64; 2]> = HashMap::new();
    let mut origin_offset: Option<[f64; 2]> = None;
    let mut dataset = OsmDataset { epsg, ..OsmDataset::default() };

    // First pass: collect `<node id="..." lat="..." lon="..."/>`
    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<node ") {
            let Some(id) = attr_u64(trimmed, "id") else { continue };
            let Some(lat) = attr_f64(trimmed, "lat") else { continue };
            let Some(lon) = attr_f64(trimmed, "lon") else { continue };
            let Some(xy) = project_lat_lon(epsg, lat, lon) else { continue };
            let off = *origin_offset.get_or_insert(xy);
            nodes.insert(id, [xy[0] - off[0], xy[1] - off[1]]);
        }
    }
    dataset.origin_offset = origin_offset.unwrap_or([0.0, 0.0]);

    // Second pass: walk `<way ...> ... </way>` and `<node ...> ... </node>` blocks
    let mut in_way: Option<u64> = None;
    let mut in_node: Option<u64> = None;
    let mut way_nds: Vec<u64> = Vec::new();
    let mut tags: HashMap<String, String> = HashMap::new();

    for line in xml.lines() {
        let t = line.trim();
        if t.starts_with("<way ") {
            in_way = attr_u64(t, "id");
            way_nds.clear();
            tags.clear();
        } else if t.starts_with("<node ") && !t.ends_with("/>") {
            in_node = attr_u64(t, "id");
            tags.clear();
        } else if t.starts_with("<nd ") {
            if let Some(r) = attr_u64(t, "ref") {
                way_nds.push(r);
            }
        } else if t.starts_with("<tag ") {
            if let (Some(k), Some(v)) = (attr_str(t, "k"), attr_str(t, "v")) {
                tags.insert(k, v);
            }
        } else if t.starts_with("</node>") {
            if let Some(id) = in_node.take()
                && let Some(&pos) = nodes.get(&id)
            {
                if let Some(hn) = tags.get("addr:housenumber") {
                    dataset.addresses.push(OsmAddress {
                        id,
                        house_number: hn.clone(),
                        street: tags.get("addr:street").cloned().unwrap_or_else(|| "Stradă".to_string()),
                        postcode: tags.get("addr:postcode").cloned(),
                        city: tags.get("addr:city").cloned(),
                        position: pos,
                    });
                }
                if tags.get("power").map(String::as_str) == Some("pole") {
                    dataset.poles.push(OsmPole { id, kind: "pole".to_string(), position: pos });
                }
            }
            tags.clear();
        } else if t.starts_with("</way>") {
            if let Some(id) = in_way.take() {
                let pts: Vec<[f64; 2]> = way_nds.iter().filter_map(|nid| nodes.get(nid).copied()).collect();
                if pts.len() >= 2 {
                    if tags.contains_key("building") {
                        let levels = tags.get("building:levels").and_then(|s| s.parse::<u32>().ok()).unwrap_or(2);
                        let height_m = f64::from(levels) * 3.1;
                        let house_number = tags.get("addr:housenumber").cloned();
                        let street = tags.get("addr:street").cloned();
                        if let Some(ref hn) = house_number {
                            dataset.addresses.push(OsmAddress {
                                id,
                                house_number: hn.clone(),
                                street: street.clone().unwrap_or_else(|| "Stradă".to_string()),
                                postcode: tags.get("addr:postcode").cloned(),
                                city: tags.get("addr:city").cloned(),
                                position: polygon_centroid(&pts),
                            });
                        }
                        dataset.buildings.push(OsmBuilding {
                            id,
                            name: tags.get("name").cloned(),
                            street,
                            house_number,
                            levels,
                            height_m,
                            polygon: pts,
                        });
                    } else if let Some(hw) = tags.get("highway").cloned() {
                        dataset.roads.push(OsmRoad { id, kind: hw, name: tags.get("name").cloned(), width_m: 7.0, points: pts });
                    } else if let Some(lu) = tags.get("landuse").or_else(|| tags.get("boundary")).cloned() {
                        dataset.parcels.push(OsmParcel { id, category: lu, label: tags.get("name").cloned(), polygon: pts });
                    }
                }
            }
            way_nds.clear();
            tags.clear();
        }
    }

    Ok(dataset)
}

fn polygon_centroid(pts: &[[f64; 2]]) -> [f64; 2] {
    if pts.is_empty() {
        return [0.0, 0.0];
    }
    let (mut sx, mut sy) = (0.0, 0.0);
    for p in pts {
        sx += p[0];
        sy += p[1];
    }
    let n = pts.len() as f64;
    [sx / n, sy / n]
}

fn attr_str(line: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn attr_u64(line: &str, name: &str) -> Option<u64> {
    attr_str(line, name)?.parse::<u64>().ok()
}

fn attr_f64(line: &str, name: &str) -> Option<f64> {
    attr_str(line, name)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overpass_json_extracts_buildings_roads_and_postal_numbers_in_stereo70() {
        let sample = r#"{
            "elements": [
                {"type": "node", "id": 1, "lat": 44.4350, "lon": 26.1010},
                {"type": "node", "id": 2, "lat": 44.4352, "lon": 26.1010},
                {"type": "node", "id": 3, "lat": 44.4352, "lon": 26.1014},
                {"type": "node", "id": 4, "lat": 44.4350, "lon": 26.1014},
                {"type": "way", "id": 100, "nodes": [1, 2, 3, 4, 1], "tags": {
                    "building": "apartments",
                    "building:levels": "4",
                    "addr:street": "Strada Lipscani",
                    "addr:housenumber": "18A",
                    "addr:postcode": "030035"
                }},
                {"type": "way", "id": 101, "nodes": [1, 4], "tags": {
                    "highway": "residential",
                    "name": "Strada Lipscani"
                }}
            ]
        }"#;
        let ds = parse_overpass_json(sample, 31700).expect("valid Overpass JSON");
        assert_eq!(ds.buildings.len(), 1);
        assert_eq!(ds.buildings[0].house_number.as_deref(), Some("18A"));
        assert_eq!(ds.buildings[0].levels, 4);
        assert_eq!(ds.addresses.len(), 1);
        assert_eq!(ds.addresses[0].street, "Strada Lipscani");
        assert_eq!(ds.roads.len(), 1);
    }

    #[test]
    fn osm_xml_parses_buildings_and_addresses() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6">
  <node id="10" lat="44.4350" lon="26.1010"/>
  <node id="11" lat="44.4353" lon="26.1010"/>
  <node id="12" lat="44.4353" lon="26.1015"/>
  <way id="200">
    <nd ref="10"/>
    <nd ref="11"/>
    <nd ref="12"/>
    <tag k="building" v="yes"/>
    <tag k="addr:housenumber" v="42"/>
    <tag k="addr:street" v="Bulevardul Unirii"/>
  </way>
</osm>"#;
        let ds = parse_osm_xml(xml, 31700).expect("valid OSM XML");
        assert_eq!(ds.buildings.len(), 1);
        assert_eq!(ds.addresses.len(), 1);
        assert_eq!(ds.addresses[0].house_number, "42");
    }
}
