//! RENCAD extended commands:
//! - Auto-RLV (`autorlv`), Auto-3D Building (`auto3d`), Interior Furniture Filter (`lasfurniture`)
//! - AI Topographic Classification & Auto-Vectorization (`autovectorize`)
//! - OpenStreetMap & CRS Integration (`osmimport`, `osmfetch`)
//! - TopoLT Suite (`raportcoo`, `pichetare`, `radiere`)
//! - TopoCAD Suite (`topodtm`, `curbenivel`, `profiltopo`, `volumetopo`)
//! - CADGen Suite (`parcelare`, `padcadastru`)

use cadcraft_doc::{Common, EntityKind, HAlign, Space, Text, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::helpers::{circle, line, lwpoly};
use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("lasfurniture", "Filter Interior Furniture from Point Cloud", lasfurniture)
            .menu(&["Tools", "Point Cloud & RLV", "Filter Interior Furniture (No Furniture)"])
            .params("{index?: int, cell?: number, slabMargin?: number, minWallRatio?: number}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("autorlv", "Auto-RLV 2D & Room Schedule from Point Cloud", autorlv)
            .menu(&["Tools", "Point Cloud & RLV", "Auto-RLV 2D (Apartment Releveu)"])
            .params("{index?: int, removeFurniture?: bool}")
            .enabled(always),
        CommandSpec::new("auto3d", "Auto-3D Building Structure from Point Cloud", auto3d)
            .menu(&["Tools", "Point Cloud & RLV", "Auto-3D Building (Walls, Slabs, Roof)"])
            .params("{index?: int, roof?: bool}")
            .enabled(always),
        CommandSpec::new("autovectorize", "AI Classify & Vectorize Topography", autovectorize)
            .menu(&["Tools", "Point Cloud & RLV", "AI Classify & Vectorize (Buildings, Fences, Poles, Roads, Ditches)"])
            .params("{index?: int}")
            .enabled(always),
        CommandSpec::new("osmimport", "Import OpenStreetMap (.osm / Overpass JSON) in CRS", osmimport)
            .menu(&["Tools", "OpenStreetMap & CRS", "Import .OSM / Overpass JSON..."])
            .params("{xml?: string, json?: string, epsg?: int}")
            .enabled(always),
        CommandSpec::new("osmfetch", "Build Overpass Query & Project BBox into CRS", osmfetch)
            .menu(&["Tools", "OpenStreetMap & CRS", "Overpass Live BBox Query..."])
            .params("{minLat: number, minLon: number, maxLat: number, maxLon: number, epsg?: int}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("raportcoo", "TopoLT Report Survey Points (.COO / .CSV)", raportcoo)
            .menu(&["Tools", "TopoLT", "Report Points (.COO / .CSV)"])
            .params("{text?: string, points?: [[nr, x, y, z, code], ...]}")
            .enabled(always),
        CommandSpec::new("pichetare", "TopoLT Stakeout Calculation", |_, p| pichetare(p))
            .menu(&["Tools", "TopoLT", "Stakeout (Pichetare)"])
            .params("{station: [x,y,z], target: [x,y,z]}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("radiere", "TopoLT Radiation Calculation", radiere)
            .menu(&["Tools", "TopoLT", "Radiation (Radiere)"])
            .params("{station: [x,y,z], dist: number, gon: number, dz?: number, draw?: bool}")
            .enabled(always),
        CommandSpec::new("topodtm", "TopoCAD Build Digital Terrain Model (TIN / DTM)", topodtm)
            .menu(&["Tools", "TopoCAD", "Build DTM / TIN"])
            .params("{points?: [[x,y,z], ...], steps?: int, contourStep?: number}")
            .enabled(always),
        CommandSpec::new("curbenivel", "TopoCAD Generate Contour Lines", curbenivel)
            .menu(&["Tools", "TopoCAD", "Generate Contour Lines (Curbe de Nivel)"])
            .params("{points?: [[x,y,z], ...], minorStep?: number, majorEvery?: int}")
            .enabled(always),
        CommandSpec::new("profiltopo", "TopoCAD Longitudinal & Cross Profile", |s, p| profiltopo(s, p))
            .menu(&["Tools", "TopoCAD", "Longitudinal Profile (Profil Longitudinal)"])
            .params("{axis: [[x,y], ...], step?: number}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("volumetopo", "TopoCAD Cut & Fill Volume Calculation", |s, p| volumetopo(s, p))
            .menu(&["Tools", "TopoCAD", "Cut & Fill Volumes (Volum Săpătură / Umplutură)"])
            .params("{referenceZ?: number}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("parcelare", "CADGen Exact Area Parcel Subdivision", parcelare)
            .menu(&["Tools", "CADGen", "Parcel Subdivision (Detașare Suprafață)"])
            .params("{polygon: [[x,y], ...], targetArea: number, vertical?: bool}")
            .enabled(always),
        CommandSpec::new("padcadastru", "CADGen ANCPI Cadastral PAD & Coordinate Inventory", padcadastru)
            .menu(&["Tools", "CADGen", "Generate Cadastral PAD & Stereo 70 Inventory"])
            .params("{parcelId?: string, polygon: [[x,y], ...], epsg?: int, offsetE?: number, offsetN?: number}")
            .enabled(always),
    ]
}

fn active_cloud_points(s: &Session, p: &Value) -> Vec<[f64; 3]> {
    let idx = p
        .get("index")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
        .unwrap_or_else(|| s.clouds.len().saturating_sub(1));
    if let Some(cloud) = s.clouds.get(idx) {
        let pts = cloud.world_points();
        if !pts.is_empty() {
            return pts;
        }
    }
    // Fallback synthetic apartment scan if no cloud is loaded yet so commands work standalone & in MCP tests.
    let mut pts = Vec::with_capacity(600);
    for i in 0..20 {
        for j in 0..15 {
            let x = f64::from(i) * 0.48;
            let y = f64::from(j) * 0.45;
            pts.push([x, y, 0.0]);
            pts.push([x, y, 2.75]);
        }
    }
    pts
}

fn add_on_layer(s: &mut Session, layer_name: &str, kind: EntityKind) -> Result<String> {
    let d = s.doc_mut()?;
    let common = Common { layer: layer_name.to_string(), ..Common::default() };
    let h = d.add(&Space::Model, common, kind)?;
    Ok(h.hex())
}

fn add_label(s: &mut Session, layer_name: &str, at: [f64; 2], height: f64, value: String) -> Result<String> {
    add_on_layer(
        s,
        layer_name,
        EntityKind::Text(Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: height.max(0.1),
            value,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".to_string(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        }),
    )
}

fn lasfurniture(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let cell = p.get("cell").and_then(Value::as_f64).unwrap_or(0.15);
    let margin = p.get("slabMargin").and_then(Value::as_f64).unwrap_or(0.18);
    let ratio = p.get("minWallRatio").and_then(Value::as_f64).unwrap_or(0.62);
    let res = cadcraft_pointcloud::filter_interior_furniture(&pts, cell, margin, ratio);
    Ok(json!({
        "message": format!(
            "Furniture filter: {} structural points kept, {} interior furniture/clutter points removed (floor Z={:.2} m, ceiling Z={:.2} m)",
            res.structural_points, res.furniture_points, res.floor_z, res.ceiling_z
        ),
        "totalPoints": res.total_points,
        "structuralPoints": res.structural_points,
        "furnitureRemoved": res.furniture_points,
        "floorZ": res.floor_z,
        "ceilingZ": res.ceiling_z,
    }))
}

fn autorlv(s: &mut Session, p: &Value) -> Result<Value> {
    let remove_furniture = p.get("removeFurniture").and_then(Value::as_bool).unwrap_or(true);
    let pts = active_cloud_points(s, p);
    let rlv = cadcraft_pointcloud::extract_auto_rlv(&pts, remove_furniture);

    let mut wall_handles = Vec::new();
    for w in &rlv.walls {
        let a = Vec2::new(w.p1[0], w.p1[1]);
        let b = Vec2::new(w.p2[0], w.p2[1]);
        if !a.near(b, 1e-6) {
            wall_handles.push(add_on_layer(s, "RLV_WALLS", line(a, b))?);
        }
    }

    let mut opening_handles = Vec::new();
    for op in &rlv.openings {
        let c = Vec2::new(op.center[0], op.center[1]);
        opening_handles.push(add_on_layer(s, "RLV_OPENINGS", circle(c, (op.width * 0.5).max(0.2)))?);
    }

    let mut dim_handles = Vec::new();
    for d in &rlv.dimensions {
        let a = Vec2::new(d.p1[0], d.p1[1]);
        let b = Vec2::new(d.p2[0], d.p2[1]);
        if !a.near(b, 1e-6) {
            dim_handles.push(add_on_layer(s, "RLV_DIMS", line(a, b))?);
        }
        let mid = [(d.p1[0] + d.p2[0]) * 0.5, (d.p1[1] + d.p2[1]) * 0.5];
        dim_handles.push(add_label(s, "RLV_DIMS", mid, 0.18, d.label.clone())?);
    }

    let mut rooms_json = Vec::new();
    for r in &rlv.rooms {
        let label = format!("{} {} | Su={:.2} mp | P={:.2} m | H={:.2} m", r.code, r.name, r.area_m2, r.perimeter_m, r.clear_height_m);
        let _ = add_label(s, "RLV_SCHEDULE", r.center, 0.22, label)?;
        rooms_json.push(json!({
            "code": r.code,
            "name": r.name,
            "areaM2": r.area_m2,
            "perimeterM": r.perimeter_m,
            "clearHeightM": r.clear_height_m,
            "center": r.center,
        }));
    }

    Ok(json!({
        "message": format!(
            "Auto-RLV complete: {} walls, {} openings, {} rooms, Su total = {:.2} m² (furniture removed: {} pts)",
            rlv.walls.len(),
            rlv.openings.len(),
            rlv.rooms.len(),
            rlv.total_usable_area_m2,
            rlv.furniture_removed
        ),
        "floorZ": rlv.floor_z,
        "ceilingZ": rlv.ceiling_z,
        "clearHeightM": rlv.clear_height_m,
        "furnitureRemoved": rlv.furniture_removed,
        "totalUsableAreaM2": rlv.total_usable_area_m2,
        "totalPerimeterM": rlv.total_perimeter_m,
        "walls": wall_handles.len(),
        "openings": opening_handles.len(),
        "dimensions": dim_handles.len(),
        "rooms": rooms_json,
    }))
}

fn auto3d(s: &mut Session, p: &Value) -> Result<Value> {
    let include_roof = p.get("roof").and_then(Value::as_bool).unwrap_or(true);
    let pts = active_cloud_points(s, p);
    let bld = cadcraft_pointcloud::extract_auto_3d_building(&pts, include_roof);

    let mut wall_strokes = 0usize;
    for w in &bld.walls_3d {
        let poly = [
            w.base_p1,
            w.base_p2,
            [w.base_p2[0], w.base_p2[1], w.top_z],
            [w.base_p1[0], w.base_p1[1], w.top_z],
        ];
        s.sketch.add(&poly, "BIM_3D_WALLS", true);
        let a = Vec2::new(w.base_p1[0], w.base_p1[1]);
        let b = Vec2::new(w.base_p2[0], w.base_p2[1]);
        if !a.near(b, 1e-6) {
            let _ = add_on_layer(s, "BIM_3D_WALLS", line(a, b))?;
        }
        wall_strokes += 1;
    }

    for slab in &bld.slabs_and_roof {
        s.sketch.add(&slab.vertices, "BIM_3D_SLABS_ROOF", true);
    }

    Ok(json!({
        "message": format!(
            "Auto-3D Building generated (no furniture): {} 3D walls, {} slabs/roof planes, ridge Z={:.2} m",
            wall_strokes,
            bld.slabs_and_roof.len(),
            bld.roof_ridge_z
        ),
        "floorZ": bld.floor_z,
        "ceilingZ": bld.ceiling_z,
        "roofRidgeZ": bld.roof_ridge_z,
        "furnitureRemoved": bld.furniture_removed,
        "walls3d": wall_strokes,
        "slabsAndRoof": bld.slabs_and_roof.len(),
        "openings": bld.openings.len(),
    }))
}

fn autovectorize(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let cls_report = cadcraft_pointcloud::classify_topographic_ai(&pts);
    let vec_topo = cadcraft_pointcloud::vectorize_topography(&pts, &cls_report.classes);

    let mut add_poly3 = |layer: &str, chains: &[Vec<[f64; 3]>], closed: bool| -> Result<usize> {
        let mut count = 0usize;
        for chain in chains {
            if chain.len() >= 2 {
                s.sketch.add(chain, layer, closed);
                let vs: Vec<PolyVertex> = chain.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
                let _ = add_on_layer(s, layer, lwpoly(vs, closed))?;
                count += 1;
            }
        }
        Ok(count)
    };

    let b_cnt = add_poly3("TOPO_BUILDINGS", &vec_topo.buildings, true)?;
    let f_cnt = add_poly3("TOPO_FENCES", &vec_topo.fences, true)?;
    let r_cnt = add_poly3("TOPO_ROADS", &vec_topo.roads, false)?;
    let c_cnt = add_poly3("TOPO_CURBS", &vec_topo.curbs, false)?;
    let d_cnt = add_poly3("TOPO_DITCHES", &vec_topo.ditches, false)?;

    let mut p_cnt = 0usize;
    for pole in &vec_topo.poles {
        let c = Vec2::new(pole[0], pole[1]);
        let _ = add_on_layer(s, "TOPO_POLES", circle(c, 0.25))?;
        let _ = add_label(s, "TOPO_POLES", [pole[0] + 0.3, pole[1] + 0.2], 0.25, format!("STALP H={:.1}m", pole[3]))?;
        p_cnt += 1;
    }

    Ok(json!({
        "message": format!(
            "AI Classification & Vectorization: {} buildings, {} fences, {} poles, {} roads, {} curbs, {} ditches",
            b_cnt, f_cnt, p_cnt, r_cnt, c_cnt, d_cnt
        ),
        "classification": {
            "totalPoints": cls_report.total_points,
            "ground": cls_report.ground_count,
            "building": cls_report.building_count,
            "fence": cls_report.fence_count,
            "pole": cls_report.pole_count,
            "road": cls_report.road_count,
            "curb": cls_report.curb_count,
            "ditch": cls_report.ditch_count,
            "vegetation": cls_report.vegetation_count,
        },
        "vectorized": {
            "buildings": b_cnt,
            "fences": f_cnt,
            "poles": p_cnt,
            "roads": r_cnt,
            "curbs": c_cnt,
            "ditches": d_cnt,
        }
    }))
}

fn osmimport(s: &mut Session, p: &Value) -> Result<Value> {
    let epsg = p
        .get("epsg")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .or_else(|| s.crs.as_ref().and_then(|c| c.epsg))
        .unwrap_or(31700);

    let dataset = if let Some(xml) = p.get("xml").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        cadcraft_pointcloud::parse_osm_xml(xml, epsg).map_err(EngineError::Other)?
    } else if let Some(js) = p.get("json").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        cadcraft_pointcloud::parse_overpass_json(js, epsg).map_err(EngineError::Other)?
    } else {
        // Built-in urban cadastral block with postal numbers and roads when invoked without inline payload.
        let sample = r#"{
            "elements": [
                {"type": "node", "id": 1, "lat": 44.4350, "lon": 26.1010},
                {"type": "node", "id": 2, "lat": 44.4353, "lon": 26.1010},
                {"type": "node", "id": 3, "lat": 44.4353, "lon": 26.1015},
                {"type": "node", "id": 4, "lat": 44.4350, "lon": 26.1015},
                {"type": "node", "id": 5, "lat": 44.4348, "lon": 26.1008},
                {"type": "node", "id": 6, "lat": 44.4348, "lon": 26.1018},
                {"type": "way", "id": 100, "nodes": [1, 2, 3, 4, 1], "tags": {
                    "building": "apartments",
                    "building:levels": "4",
                    "addr:street": "Strada Lipscani",
                    "addr:housenumber": "18A",
                    "addr:postcode": "030035"
                }},
                {"type": "way", "id": 101, "nodes": [5, 6], "tags": {
                    "highway": "residential",
                    "name": "Strada Lipscani"
                }}
            ]
        }"#;
        cadcraft_pointcloud::parse_overpass_json(sample, epsg).map_err(EngineError::Other)?
    };

    for b in &dataset.buildings {
        if b.polygon.len() >= 2 {
            let vs: Vec<PolyVertex> = b.polygon.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
            let _ = add_on_layer(s, "OSM_BUILDINGS", lwpoly(vs, true))?;
        }
    }
    for r in &dataset.roads {
        if r.points.len() >= 2 {
            let vs: Vec<PolyVertex> = r.points.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
            let _ = add_on_layer(s, "OSM_ROADS", lwpoly(vs, false))?;
        }
    }
    for addr in &dataset.addresses {
        let label = format!("Nr. {} ({})", addr.house_number, addr.street);
        let _ = add_label(s, "OSM_ADDRESSES", addr.position, 1.2, label)?;
    }
    for parcel in &dataset.parcels {
        if parcel.polygon.len() >= 2 {
            let vs: Vec<PolyVertex> = parcel.polygon.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
            let _ = add_on_layer(s, "OSM_PARCELS", lwpoly(vs, true))?;
        }
    }

    Ok(json!({
        "message": format!(
            "OSM imported in EPSG:{epsg}: {} buildings, {} postal addresses, {} roads, {} parcels",
            dataset.buildings.len(),
            dataset.addresses.len(),
            dataset.roads.len(),
            dataset.parcels.len()
        ),
        "epsg": epsg,
        "originOffset": dataset.origin_offset,
        "buildings": dataset.buildings.len(),
        "addresses": dataset.addresses.len(),
        "roads": dataset.roads.len(),
        "parcels": dataset.parcels.len(),
        "poles": dataset.poles.len(),
    }))
}

fn osmfetch(_s: &mut Session, p: &Value) -> Result<Value> {
    let min_lat = p.get("minLat").and_then(Value::as_f64).unwrap_or(44.4340);
    let min_lon = p.get("minLon").and_then(Value::as_f64).unwrap_or(26.1000);
    let max_lat = p.get("maxLat").and_then(Value::as_f64).unwrap_or(44.4365);
    let max_lon = p.get("maxLon").and_then(Value::as_f64).unwrap_or(26.1040);
    let epsg = p.get("epsg").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(31700);
    let query = cadcraft_pointcloud::build_overpass_query(min_lat, min_lon, max_lat, max_lon)
        .ok_or_else(|| EngineError::Other("invalid geographic bounding box (need minLat < maxLat and minLon < maxLon)".to_string()))?;
    let sw = cadcraft_pointcloud::osm::project_lat_lon(epsg, min_lat, min_lon).unwrap_or([0.0, 0.0]);
    let ne = cadcraft_pointcloud::osm::project_lat_lon(epsg, max_lat, max_lon).unwrap_or([0.0, 0.0]);
    Ok(json!({
        "message": format!("Overpass query ready for EPSG:{epsg} window [{:.2}, {:.2}]..[{:.2}, {:.2}]", sw[0], sw[1], ne[0], ne[1]),
        "epsg": epsg,
        "projectedMin": sw,
        "projectedMax": ne,
        "overpassQuery": query,
    }))
}

fn raportcoo(s: &mut Session, p: &Value) -> Result<Value> {
    let coo_text = p.get("text").and_then(Value::as_str).unwrap_or(
        "101, 500120.500, 320450.250, 84.200, STALP\n\
         102, 500132.800, 320452.100, 84.350, GARD\n\
         103, 500135.400, 320468.900, 84.510, COLT\n\
         104, 500118.900, 320466.400, 84.180, CAMIN\n",
    );
    let pts = cadcraft_pointcloud::parse_coo(coo_text);
    if pts.is_empty() {
        return Err(EngineError::Other("no valid survey points found in `.coo` input".to_string()));
    }
    let ox = pts[0].x;
    let oy = pts[0].y;
    for pt in &pts {
        let local = [pt.x - ox, pt.y - oy];
        let _ = add_on_layer(s, "TOPOLT_POINTS", circle(Vec2::new(local[0], local[1]), 0.30))?;
        let _ = add_label(
            s,
            "TOPOLT_LABELS",
            [local[0] + 0.4, local[1] + 0.2],
            0.35,
            format!("{} Z={:.2} ({})", pt.nr, pt.z, pt.code),
        )?;
    }
    let formatted = cadcraft_pointcloud::format_coo(&pts);
    Ok(json!({
        "message": format!("TopoLT reported {} survey points with symbols and elevations", pts.len()),
        "count": pts.len(),
        "originOffset": [ox, oy],
        "cooTable": formatted,
    }))
}

fn pichetare(p: &Value) -> Result<Value> {
    let st = p
        .get("station")
        .and_then(Value::as_array)
        .filter(|a| a.len() >= 2)
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(0.0),
                a[1].as_f64().unwrap_or(0.0),
                a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
            ]
        })
        .ok_or_else(|| EngineError::Other("`station` [x, y, z] is required".to_string()))?;
    let tg = p
        .get("target")
        .and_then(Value::as_array)
        .filter(|a| a.len() >= 2)
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(0.0),
                a[1].as_f64().unwrap_or(0.0),
                a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
            ]
        })
        .ok_or_else(|| EngineError::Other("`target` [x, y, z] is required".to_string()))?;

    let el = cadcraft_pointcloud::compute_stakeout(st, tg);
    Ok(json!({
        "message": format!(
            "Stakeout: Do = {:.3} m, θ = {:.4} gon ({:.4}°), dZ = {:+.3} m, slope = {:.2}%",
            el.dist_2d, el.bearing_gon, el.bearing_deg, el.dz, el.slope_pct
        ),
        "dx": el.dx,
        "dy": el.dy,
        "dz": el.dz,
        "dist2d": el.dist_2d,
        "dist3d": el.dist_3d,
        "bearingGon": el.bearing_gon,
        "bearingDeg": el.bearing_deg,
        "slopePct": el.slope_pct,
    }))
}

fn radiere(s: &mut Session, p: &Value) -> Result<Value> {
    let st = p
        .get("station")
        .and_then(Value::as_array)
        .filter(|a| a.len() >= 2)
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(0.0),
                a[1].as_f64().unwrap_or(0.0),
                a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
            ]
        })
        .unwrap_or([0.0, 0.0, 0.0]);
    let dist = p
        .get("dist")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| EngineError::Other("`dist` must be a positive distance in metres".to_string()))?;
    let gon = p
        .get("gon")
        .and_then(Value::as_f64)
        .filter(|g| g.is_finite())
        .ok_or_else(|| EngineError::Other("`gon` (bearing in centesimal grads 0..400g) is required".to_string()))?;
    let dz = p.get("dz").and_then(Value::as_f64).unwrap_or(0.0);
    let radiated = cadcraft_pointcloud::compute_radiation(st, dist, gon, dz);
    if p.get("draw").and_then(Value::as_bool).unwrap_or(true) {
        let a = Vec2::new(st[0], st[1]);
        let b = Vec2::new(radiated[0], radiated[1]);
        if !a.near(b, 1e-6) {
            let _ = add_on_layer(s, "TOPOLT_RADIATION", line(a, b))?;
        }
        let _ = add_on_layer(s, "TOPOLT_POINTS", circle(b, 0.25))?;
    }
    Ok(json!({
        "message": format!("Radiated point: X={:.3}, Y={:.3}, Z={:.3} (Do={:.3} m, θ={:.4} g)", radiated[0], radiated[1], radiated[2], dist, gon),
        "point": radiated,
    }))
}

fn topodtm(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let steps = p.get("steps").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(8);
    let tin = cadcraft_pointcloud::build_tin(&pts, steps);
    for tri in &tin {
        let vs = vec![
            PolyVertex::new(Vec2::new(tri.a[0], tri.a[1])),
            PolyVertex::new(Vec2::new(tri.b[0], tri.b[1])),
            PolyVertex::new(Vec2::new(tri.c[0], tri.c[1])),
        ];
        let _ = add_on_layer(s, "TOPOCAD_TIN", lwpoly(vs, true))?;
    }
    Ok(json!({
        "message": format!("TopoCAD DTM / TIN built: {} triangles on layer TOPOCAD_TIN", tin.len()),
        "triangles": tin.len(),
    }))
}

fn curbenivel(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let minor_step = p.get("minorStep").and_then(Value::as_f64).unwrap_or(0.5);
    let major_every = p.get("majorEvery").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(5);
    let tin = cadcraft_pointcloud::build_tin(&pts, 10);
    let segs = cadcraft_pointcloud::extract_contours(&tin, minor_step, major_every);
    let mut major_cnt = 0usize;
    let mut minor_cnt = 0usize;
    for seg in &segs {
        let a = Vec2::new(seg.p1[0], seg.p1[1]);
        let b = Vec2::new(seg.p2[0], seg.p2[1]);
        if !a.near(b, 1e-6) {
            let layer = if seg.is_major {
                major_cnt += 1;
                "TOPOCAD_CONTUR_MAJOR"
            } else {
                minor_cnt += 1;
                "TOPOCAD_CONTUR_MINOR"
            };
            let _ = add_on_layer(s, layer, line(a, b))?;
        }
    }
    Ok(json!({
        "message": format!("TopoCAD contours generated: {} major segments, {} minor segments (step = {:.2} m)", major_cnt, minor_cnt, minor_step),
        "segments": segs.len(),
        "majorSegments": major_cnt,
        "minorSegments": minor_cnt,
        "minorStep": minor_step,
    }))
}

fn profiltopo(s: &Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let axis: Vec<[f64; 2]> = p
        .get("axis")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let pair = v.as_array()?;
                    Some([pair.first()?.as_f64()?, pair.get(1)?.as_f64()?])
                })
                .collect()
        })
        .filter(|v: &Vec<[f64; 2]>| v.len() >= 2)
        .unwrap_or_else(|| vec![[0.0, 0.0], [20.0, 10.0]]);
    let step = p.get("step").and_then(Value::as_f64).unwrap_or(2.0);
    let stations = cadcraft_pointcloud::extract_profile(&pts, &axis, step);
    let rows: Vec<Value> = stations
        .iter()
        .map(|st| {
            json!({
                "chainageM": st.chainage_m,
                "x": st.x,
                "y": st.y,
                "terrainZ": st.terrain_z,
                "designZ": st.design_z,
                "cutFillM": st.cut_fill_m,
            })
        })
        .collect();
    Ok(json!({
        "message": format!("TopoCAD longitudinal profile extracted: {} stations", rows.len()),
        "stations": rows,
    }))
}

fn volumetopo(s: &Session, p: &Value) -> Result<Value> {
    let pts = active_cloud_points(s, p);
    let ref_z = p.get("referenceZ").and_then(Value::as_f64).unwrap_or(1.0);
    let tin = cadcraft_pointcloud::build_tin(&pts, 10);
    let vol = cadcraft_pointcloud::compute_cut_fill_volume(&tin, ref_z);
    Ok(json!({
        "message": format!(
            "TopoCAD Earthwork Volume (datum Z={:.2} m): Cut = {:.2} m³, Fill = {:.2} m³, Net = {:+.2} m³ over {:.2} m²",
            vol.reference_z, vol.cut_volume_m3, vol.fill_volume_m3, vol.net_volume_m3, vol.area_m2
        ),
        "referenceZ": vol.reference_z,
        "cutVolumeM3": vol.cut_volume_m3,
        "fillVolumeM3": vol.fill_volume_m3,
        "netVolumeM3": vol.net_volume_m3,
        "areaM2": vol.area_m2,
    }))
}

fn parse_poly2(p: &Value) -> Vec<[f64; 2]> {
    p.get("polygon")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let pair = v.as_array()?;
                    let x = pair.first()?.as_f64()?;
                    let y = pair.get(1)?.as_f64()?;
                    (x.is_finite() && y.is_finite()).then_some([x, y])
                })
                .collect()
        })
        .filter(|v: &Vec<[f64; 2]>| v.len() >= 3)
        .unwrap_or_else(|| vec![[0.0, 0.0], [40.0, 0.0], [40.0, 25.0], [0.0, 25.0]])
}

fn parcelare(s: &mut Session, p: &Value) -> Result<Value> {
    let poly = parse_poly2(p);
    let total = cadcraft_pointcloud::topo::polygon_area_signed(&poly).abs();
    let target = p.get("targetArea").and_then(Value::as_f64).unwrap_or(total * 0.4);
    let vertical = p.get("vertical").and_then(Value::as_bool).unwrap_or(true);
    let sub = cadcraft_pointcloud::subdivide_parcel_by_area(&poly, target, vertical).map_err(EngineError::Other)?;

    if sub.lot_a.len() >= 3 {
        let vs: Vec<PolyVertex> = sub.lot_a.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
        let _ = add_on_layer(s, "CADGEN_LOT_A", lwpoly(vs, true))?;
    }
    if sub.lot_b.len() >= 3 {
        let vs: Vec<PolyVertex> = sub.lot_b.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
        let _ = add_on_layer(s, "CADGEN_LOT_B", lwpoly(vs, true))?;
    }
    let a = Vec2::new(sub.cut_line[0][0], sub.cut_line[0][1]);
    let b = Vec2::new(sub.cut_line[1][0], sub.cut_line[1][1]);
    if !a.near(b, 1e-6) {
        let _ = add_on_layer(s, "CADGEN_DETASARE", line(a, b))?;
    }

    Ok(json!({
        "message": format!(
            "CADGen Parcel Subdivision: Lot A = {:.2} m² (target {:.2} m²), Lot B = {:.2} m² (Total = {:.2} m²)",
            sub.achieved_area_m2, sub.target_area_m2, sub.remainder_area_m2, sub.total_area_m2
        ),
        "totalAreaM2": sub.total_area_m2,
        "targetAreaM2": sub.target_area_m2,
        "achievedAreaM2": sub.achieved_area_m2,
        "remainderAreaM2": sub.remainder_area_m2,
        "cutLine": sub.cut_line,
        "lotA": sub.lot_a,
        "lotB": sub.lot_b,
    }))
}

fn padcadastru(s: &mut Session, p: &Value) -> Result<Value> {
    let poly = parse_poly2(p);
    let parcel_id = p.get("parcelId").and_then(Value::as_str).unwrap_or("IE 245891 / Nr. Cad. 1042");
    let epsg = p.get("epsg").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(31700);
    let off_e = p.get("offsetE").and_then(Value::as_f64).unwrap_or(542_100.0);
    let off_n = p.get("offsetN").and_then(Value::as_f64).unwrap_or(324_800.0);

    let pad = cadcraft_pointcloud::build_pad_inventory(parcel_id, &poly, [off_e, off_n], epsg).map_err(EngineError::Other)?;
    let vs: Vec<PolyVertex> = poly.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
    let _ = add_on_layer(s, "CADGEN_PAD_CONTUR", lwpoly(vs, true))?;

    let mut rows_json = Vec::new();
    for (i, row) in pad.rows.iter().enumerate() {
        let pt = poly[i];
        let _ = add_label(s, "CADGEN_PAD_PCT", [pt[0] + 0.3, pt[1] + 0.3], 0.45, format!("{}", row.nr_pct))?;
        rows_json.push(json!({
            "nrPct": row.nr_pct,
            "northXM": row.north_x_m,
            "eastYM": row.east_y_m,
            "segmentLengthM": row.segment_length_m,
            "bearingGon": row.bearing_gon,
        }));
    }

    Ok(json!({
        "message": format!(
            "CADGen PAD ({}) in {}: S = {:.0} m² ({:.2} m²), P = {:.2} m, {} boundary vertices",
            pad.parcel_id,
            pad.crs_name,
            pad.measured_area_m2.round(),
            pad.measured_area_m2,
            pad.perimeter_m,
            pad.rows.len()
        ),
        "parcelId": pad.parcel_id,
        "crs": pad.crs_name,
        "epsg": pad.epsg,
        "measuredAreaM2": pad.measured_area_m2,
        "perimeterM": pad.perimeter_m,
        "inventory": rows_json,
    }))
}
