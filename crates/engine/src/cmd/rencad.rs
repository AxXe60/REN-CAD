//! RENCAD extended commands:
//! - Auto-RLV (`autorlv`), Auto-3D Building (`auto3d`), Interior Furniture Filter (`lasfurniture`)
//! - AI Topographic Classification & Auto-Vectorization (`autovectorize`)
//! - OpenStreetMap & CRS Integration (`osmimport`, `osmfetch`)
//! - TopoLT Suite (`raportcoo`, `pichetare`, `radiere`)
//! - TopoCAD Suite (`topodtm`, `curbenivel`, `profiltopo`, `volumetopo`)
//! - CADGen Suite (`parcelare`, `padcadastru`)

use std::path::Path;

use cadcraft_doc::{Common, DimKind, EntityKind, HAlign, Space, Text, VAlign};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use cadcraft_pointcloud::bim::{deviate, dimensions, ifc, openings};
use cadcraft_pointcloud::{AutoRlvResult, BimOptions, StoreyModel, slice_bands_capped};
use serde_json::{Value, json};

use super::annotate::dim;
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
            .params("{index?: int, ifc?: string, params?: {cell?, z_step?, slab_ratio?, slice_from?, slice_to?, threshold?, close_radius?, epsilon?, safety_margin?, single_sided_slab_thickness?, min_wall_length?, min_wall_thickness?, max_wall_thickness?, angle_tolerance_deg?, exterior_wall_thickness?, exterior_scan?, zone_gap?, min_zone_area?, opening_cell?, opening_threshold?, opening_rank?, min_opening_width?, max_opening_width?, door_min_height?, door_max_sill?, window_min_sill?, opening_band?, opening_decimate?, max_wall_band_points?, deviation_threshold?, deviation_cell?, min_furniture_points?, max_furniture_clusters?, min_column_size?, max_column_size?, max_column_aspect_ratio?}}")
            .enabled(always),
        CommandSpec::new("auto3d", "Auto-3D Building Structure from Point Cloud", auto3d)
            .menu(&["Tools", "Point Cloud & RLV", "Auto-3D Building (Walls, Slabs)"])
            .params("{index?: int, ifc?: string, params?: {cell?, z_step?, slab_ratio?, slice_from?, slice_to?, threshold?, close_radius?, epsilon?, safety_margin?, single_sided_slab_thickness?, min_wall_length?, min_wall_thickness?, max_wall_thickness?, angle_tolerance_deg?, exterior_wall_thickness?, exterior_scan?, zone_gap?, min_zone_area?, opening_cell?, opening_threshold?, opening_rank?, min_opening_width?, max_opening_width?, door_min_height?, door_max_sill?, window_min_sill?, opening_band?, opening_decimate?, max_wall_band_points?, min_column_size?, max_column_size?, max_column_aspect_ratio?}}")
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
    let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or_else(|| s.clouds.len().saturating_sub(1));
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

/// The points the modelling commands may use: the named cloud, or the last one opened.
///
/// The other point-cloud commands keep [`active_cloud_points`]' synthetic scan so their standalone
/// demonstrations still run. The modelling commands do not: `AUTORLV` on a made-up apartment would
/// draw a building nobody scanned, which is the failure this command is replacing.
fn cloud_points_for_model(s: &Session, p: &Value) -> Result<Vec<[f64; 3]>> {
    let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or_else(|| s.clouds.len().saturating_sub(1));
    let Some(cloud) = s.clouds.get(idx) else {
        return Err(EngineError::Other("no point cloud is open: open the scan with LASOPEN first".to_string()));
    };
    let pts = cloud.world_points();
    if pts.is_empty() {
        return Err(EngineError::Other(format!("the cloud `{}` holds no points in its band", cloud.name)));
    }
    Ok(pts)
}

/// The pipeline options a modelling command carries, from its `params` object.
fn model_options(p: &Value) -> Result<cadcraft_pointcloud::BimOptions> {
    cadcraft_pointcloud::BimOptions::from_params(p.get("params").unwrap_or(&Value::Null))
        .map_err(|e| EngineError::Other(format!("the modelling options cannot be used: {e}")))
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

/// The path of the cloud a modelling command reads, chosen by the same index rule as
/// [`cloud_points_for_model`]. The openings stage streams a finer pass than the working level
/// carries, so it needs the file the cloud came from; a cloud opened from memory has no path and
/// the stage reports no openings rather than inventing them.
fn cloud_path_for_model(s: &Session, p: &Value) -> Result<String> {
    let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or_else(|| s.clouds.len().saturating_sub(1));
    let Some(cloud) = s.clouds.get(idx) else {
        return Err(EngineError::Other("no point cloud is open: open the scan with LASOPEN first".to_string()));
    };
    Ok(cloud.path.clone())
}

/// The openings of every storey, from one fine read of the scan.
///
/// `band` decides which walls can be read, and the original index is carried alongside each band
/// so an opening still names the wall it pierces after the unusable walls have been filtered out.
/// A failed read or a refused wall is "no openings here", never a failed model: the rest of the
/// scan was good, and the reply says the fine bands were unavailable.
fn storey_openings(path: &str, storeys: &[StoreyModel], opts: &BimOptions) -> Vec<Vec<openings::Opening>> {
    let mut out = Vec::with_capacity(storeys.len());
    for model in storeys {
        if !model.openings.is_empty() {
            out.push(model.openings.clone());
            continue;
        }
        if path.is_empty() {
            out.push(Vec::new());
            continue;
        }
        let mut bands = Vec::new();
        let mut origins = Vec::new();
        for (i, wall) in model.walls.iter().enumerate() {
            if let Some(band) = openings::band(wall, opts) {
                bands.push(band);
                origins.push(i);
            }
        }
        if bands.is_empty() {
            out.push(Vec::new());
            continue;
        }
        let Ok(per_band) = slice_bands_capped(Path::new(path), &bands, opts.opening_decimate, opts.max_wall_band_points) else {
            out.push(Vec::new());
            continue;
        };
        let mut holes = Vec::new();
        for (k, &i) in origins.iter().enumerate() {
            let (Some(points), Some(wall)) = (per_band.get(k), model.walls.get(i)) else {
                continue;
            };
            let fine: Vec<[f64; 3]> = points.iter().map(|pt| [pt.x, pt.y, pt.z]).collect();
            if let Ok(mut found) = openings::detect(&fine, wall, i, opts) {
                holes.append(&mut found);
            }
        }
        holes.sort_by(|a, b| {
            a.wall.cmp(&b.wall).then(a.from[0].total_cmp(&b.from[0])).then(a.from[1].total_cmp(&b.from[1])).then(a.kind.as_str().cmp(b.kind.as_str()))
        });
        out.push(holes);
    }
    out
}

/// Draw each opening as the two jamb lines that cross its wall body.
fn draw_openings(s: &mut Session, per_storey: &[Vec<openings::Opening>]) -> Result<usize> {
    let mut lines = 0usize;
    for holes in per_storey {
        for o in holes {
            let (dx, dy) = (o.to[0] - o.from[0], o.to[1] - o.from[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if !len.is_finite() || len <= 0.0 {
                continue;
            }
            let (nx, ny) = (-dy / len * o.thickness * 0.5, dx / len * o.thickness * 0.5);
            for at in [o.from, o.to] {
                add_on_layer(s, "RLV_OPENINGS", line(Vec2::new(at[0] - nx, at[1] - ny), Vec2::new(at[0] + nx, at[1] + ny)))?;
                lines += 1;
            }
        }
    }
    Ok(lines)
}

/// The number of openings, doors and windows across every storey.
fn opening_counts(per_storey: &[Vec<openings::Opening>]) -> (usize, usize, usize) {
    let mut count = 0usize;
    let mut doors = 0usize;
    let mut windows = 0usize;
    for o in per_storey.iter().flatten() {
        count += 1;
        match o.kind {
            openings::OpeningKind::Door => doors += 1,
            openings::OpeningKind::Window => windows += 1,
        }
    }
    (count, doors, windows)
}

/// Draw one plan rectangle per furniture cluster, as the deviation stage isolated it.
fn draw_furniture(s: &mut Session, clusters: &[deviate::FurnitureCluster]) -> Result<usize> {
    let mut rects = 0usize;
    for c in clusters {
        if !(c.max[0] > c.min[0] && c.max[1] > c.min[1]) {
            continue;
        }
        let vs = vec![
            PolyVertex::new(Vec2::new(c.min[0], c.min[1])),
            PolyVertex::new(Vec2::new(c.max[0], c.min[1])),
            PolyVertex::new(Vec2::new(c.max[0], c.max[1])),
            PolyVertex::new(Vec2::new(c.min[0], c.max[1])),
        ];
        add_on_layer(s, "RLV_FURNITURE", lwpoly(vs, true))?;
        rects += 1;
    }
    Ok(rects)
}

/// The IFC building the model describes.
///
/// A slab belongs to one storey, not both: the floor slab of storey `i` is `slabs[i]`, and the last
/// storey also carries the top slab over it. That keeps a shared middle slab from being written
/// twice while every storey still has its floor and its ceiling in the file.
fn building_from_rlv(rlv: &AutoRlvResult, per_storey: &[Vec<openings::Opening>]) -> ifc::Building {
    let storeys = rlv
        .storeys
        .iter()
        .enumerate()
        .map(|(i, model)| {
            let mut slabs = Vec::new();
            if let Some(slab) = rlv.slabs.get(i) {
                slabs.push(slab.clone());
            }
            if i + 1 == rlv.storeys.len()
                && let Some(slab) = rlv.slabs.get(i + 1)
            {
                slabs.push(slab.clone());
            }
            ifc::BuildingStorey {
                name: format!("Storey {}", i + 1),
                elevation: model.storey.base_z,
                height: model.storey.top_z - model.storey.base_z,
                walls: model.walls.clone(),
                slabs,
                zones: model.zones.clone(),
                openings: per_storey.get(i).cloned().unwrap_or_default(),
                columns: model.columns.clone(),
            }
        })
        .collect();
    ifc::Building { name: "RENCAD RLV".to_string(), timestamp: unix_seconds(), storeys }
}

/// Seconds since the Unix epoch. The IFC's `IfcTimeStamp` is exactly this, and the engine needs no
/// calendar conversion to write it.
fn unix_seconds() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The `ifc` parameter: a path to write, or nothing. A value that is there but is not a usable path
/// is refused rather than quietly ignored.
fn ifc_param(p: &Value) -> Result<Option<String>> {
    match p.get("ifc") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) if !path.trim().is_empty() => Ok(Some(path.clone())),
        Some(_) => Err(EngineError::Other("ifc must be a file path string".to_string())),
    }
}

/// Write the STEP file and read it back with our own checker before reporting it.
///
/// A file the writer produces is not advertised until [`ifc::check`] has closed its entity graph;
/// a write that fails is an error and no file is claimed.
fn write_ifc_file(path: &str, building: &ifc::Building) -> Result<Value> {
    let step = ifc::write(building).map_err(|e| EngineError::Other(format!("IFC: {e}")))?;
    let checked = ifc::check(&step).map_err(|e| EngineError::Other(format!("IFC self-check: {e}")))?;
    std::fs::write(path, step.as_bytes()).map_err(|e| EngineError::Other(format!("IFC write {path}: {e}")))?;
    Ok(json!({
        "path": path,
        "entities": checked.entities,
        "references": checked.references,
        "walls": checked.walls,
        "slabs": checked.slabs,
        "spaces": checked.spaces,
        "openings": checked.openings,
        "doors": checked.doors,
        "windows": checked.windows,
    }))
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
    let opts = model_options(p)?;
    let pts = cloud_points_for_model(s, p)?;
    let path = cloud_path_for_model(s, p)?;
    let rlv = cadcraft_pointcloud::extract_auto_rlv(&pts, &opts).map_err(|e| EngineError::Other(format!("Auto-RLV: {e}")))?;
    let per_storey = storey_openings(&path, &rlv.storeys, &opts);

    let mut wall_handles = Vec::new();
    let mut zone_handles = Vec::new();
    let mut rooms_json = Vec::new();
    let mut storeys_json = Vec::new();
    for model in &rlv.storeys {
        for w in &model.walls {
            let a = Vec2::new(w.from[0], w.from[1]);
            let b = Vec2::new(w.to[0], w.to[1]);
            if !a.near(b, 1e-6) {
                wall_handles.push(add_on_layer(s, "RLV_WALLS", line(a, b))?);
            }
        }
        for zone in &model.zones {
            if zone.ring.len() >= 3 {
                let vs: Vec<PolyVertex> = zone.ring.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
                zone_handles.push(add_on_layer(s, "RLV_ZONES", lwpoly(vs, true))?);
            }
        }
        storeys_json.push(json!({
            "index": model.storey.index,
            "baseZ": model.storey.base_z,
            "topZ": model.storey.top_z,
            "clearHeightM": model.storey.top_z - model.storey.base_z,
            "walls": model.walls.len(),
            "zones": model.zones.len(),
            "openings": per_storey.get(model.storey.index).map_or(0, Vec::len),
        }));
    }

    let mut col_handles = Vec::new();
    for model in &rlv.storeys {
        for col in &model.columns {
            match &col.shape {
                cadcraft_pointcloud::bim::ColumnShape::Circular { diameter } => {
                    let c = Vec2::new(col.center[0], col.center[1]);
                    col_handles.push(add_on_layer(s, "RLV_COLUMNS", circle(c, diameter / 2.0))?);
                }
                cadcraft_pointcloud::bim::ColumnShape::Rectangular { width, depth, rotation_deg } => {
                    let rad = rotation_deg.to_radians();
                    let cos_a = rad.cos();
                    let sin_a = rad.sin();
                    let hw = width / 2.0;
                    let hd = depth / 2.0;
                    let corners = [[-hw, -hd], [hw, -hd], [hw, hd], [-hw, hd]];
                    let vs: Vec<PolyVertex> = corners
                        .iter()
                        .map(|[lx, ly]| {
                            let gx = col.center[0] + lx * cos_a - ly * sin_a;
                            let gy = col.center[1] + lx * sin_a + ly * cos_a;
                            PolyVertex::new(Vec2::new(gx, gy))
                        })
                        .collect();
                    col_handles.push(add_on_layer(s, "RLV_COLUMNS", lwpoly(vs, true))?);
                }
            }
        }
    }

    let mut slab_handles = Vec::new();
    let mut slabs_json = Vec::new();
    for slab in &rlv.slabs {
        if slab.polygon.len() >= 3 {
            let vs: Vec<PolyVertex> = slab.polygon.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
            slab_handles.push(add_on_layer(s, "RLV_SLABS", lwpoly(vs, true))?);
        }
        for hole in &slab.holes {
            if hole.len() >= 3 {
                let hvs: Vec<PolyVertex> = hole.iter().map(|pt| PolyVertex::new(Vec2::new(pt[0], pt[1]))).collect();
                slab_handles.push(add_on_layer(s, "RLV_SLABS", lwpoly(hvs, true))?);
            }
        }
        slabs_json.push(json!({
            "baseZ": slab.base_z,
            "topZ": slab.top_z,
            "thicknessM": slab.thickness,
            "singleSided": slab.single_sided,
            "vertices": slab.polygon.len(),
            "holes": slab.holes.len(),
        }));
    }

    let opening_lines = draw_openings(s, &per_storey)?;
    let all_walls: Vec<cadcraft_pointcloud::bim::Wall> = rlv.storeys.iter().flat_map(|m| m.walls.iter().cloned()).collect();
    let dev = deviate::measure(&pts, &all_walls, &rlv.slabs, &opts).map_err(|e| EngineError::Other(format!("Deviation: {e}")))?;
    let furniture_rects = draw_furniture(s, &dev.furniture)?;

    let auto_dims = dimensions::generate_dimensions(&rlv.storeys, &opts);
    let mut dim_handles = Vec::new();
    for d in &auto_dims {
        let p1 = Vec2::new(d.p1[0], d.p1[1]);
        let p2 = Vec2::new(d.p2[0], d.p2[1]);
        let at = Vec2::new(d.at[0], d.at[1]);
        let k = dim(s, DimKind::Aligned, at, p1, p2, Vec2::ZERO, "");
        dim_handles.push(add_on_layer(s, "RLV_DIMS", k)?);
    }

    let mut label_handles = Vec::new();
    for model in &rlv.storeys {
        for r in &model.rooms {
            let label = format!("{} {} | Su={:.2} mp | P={:.2} m | H={:.2} m", r.code, r.name, r.area_m2, r.perimeter_m, r.clear_height_m);
            label_handles.push(add_label(s, "RLV_SCHEDULE", r.center, 0.22, label)?);
            rooms_json.push(json!({
                "code": r.code,
                "name": r.name,
                "areaM2": r.area_m2,
                "perimeterM": r.perimeter_m,
                "clearHeightM": r.clear_height_m,
                "center": r.center,
                "storey": model.storey.index,
            }));
        }
    }

    let (opening_count, doors, windows) = opening_counts(&per_storey);
    let ifc_json = match ifc_param(p)? {
        Some(path) => Some(write_ifc_file(&path, &building_from_rlv(&rlv, &per_storey))?),
        None => None,
    };

    let mut reply = json!({
        "message": format!(
            "Auto-RLV complete: {} slabs, {} storeys, {} walls, {} rooms, {} openings ({} doors, {} windows), Su total = {:.2} m²",
            rlv.slabs.len(),
            rlv.storeys.len(),
            rlv.walls(),
            rlv.rooms(),
            opening_count,
            doors,
            windows,
            rlv.total_usable_area_m2
        ),
        "floorZ": rlv.floor_z,
        "ceilingZ": rlv.ceiling_z,
        "walls": rlv.walls(),
        "zones": rlv.zones(),
        "slabs": rlv.slabs.len(),
        "columns": rlv.columns(),
        "parapets": rlv.parapets(),
        "roomCount": rlv.rooms(),
        "totalUsableAreaM2": rlv.total_usable_area_m2,
        "totalPerimeterM": rlv.total_perimeter_m,
        "dimensions": dim_handles.len(),
        "storeyDetails": storeys_json,
        "slabDetails": slabs_json,
        "rooms": rooms_json,
        "openings": {
            "count": opening_count,
            "doors": doors,
            "windows": windows,
            "perStorey": per_storey.iter().map(Vec::len).collect::<Vec<_>>(),
            "fineBands": if path.is_empty() { "unavailable" } else { "streamed" },
        },
        "deviation": {
            "points": dev.points,
            "rmseMm": dev.rmse_mm,
            "meanMm": dev.mean_mm,
            "maxMm": dev.max_mm,
            "aboveThreshold": dev.above_threshold,
            "furniture": dev.furniture.len(),
        },
        "notAvailable": [],
        "drawn": {
            "walls": wall_handles.len(),
            "zones": zone_handles.len(),
            "slabs": slab_handles.len(),
            "columns": col_handles.len(),
            "parapets": rlv.parapets(),
            "openings": opening_lines,
            "furniture": furniture_rects,
            "labels": label_handles.len(),
            "dimensions": dim_handles.len(),
        },
    });
    if let Some(ifc) = ifc_json {
        reply["ifc"] = ifc;
    }
    Ok(reply)
}

fn auto3d(s: &mut Session, p: &Value) -> Result<Value> {
    let opts = model_options(p)?;
    let pts = cloud_points_for_model(s, p)?;
    let path = cloud_path_for_model(s, p)?;
    let rlv = cadcraft_pointcloud::extract_auto_rlv(&pts, &opts).map_err(|e| EngineError::Other(format!("Auto-3D: {e}")))?;
    let per_storey = storey_openings(&path, &rlv.storeys, &opts);

    let mut wall_planes = 0usize;
    for model in &rlv.storeys {
        for w in &model.walls {
            let a = Vec2::new(w.from[0], w.from[1]);
            let b = Vec2::new(w.to[0], w.to[1]);
            if a.near(b, 1e-6) {
                continue;
            }
            let _ = add_on_layer(s, "RLV_3D_WALLS", line(a, b))?;
            let (dx, dy) = (w.to[0] - w.from[0], w.to[1] - w.from[1]);
            let length = (dx * dx + dy * dy).sqrt();
            if !length.is_finite() || length <= 0.0 {
                continue;
            }
            let (nx, ny) = (-dy / length * w.thickness * 0.5, dx / length * w.thickness * 0.5);
            for side in [1.0, -1.0] {
                let x1 = w.from[0] + nx * side;
                let y1 = w.from[1] + ny * side;
                let x2 = w.to[0] + nx * side;
                let y2 = w.to[1] + ny * side;
                let face = [[x1, y1, w.base_z], [x2, y2, w.base_z], [x2, y2, w.base_z + w.height], [x1, y1, w.base_z + w.height]];
                s.sketch.add(&face, "RLV_3D_WALLS", true);
            }
            wall_planes += 1;
        }
    }

    let mut col_count = 0usize;
    for model in &rlv.storeys {
        for col in &model.columns {
            let z0 = col.base_z;
            let z1 = col.base_z + col.height;
            match &col.shape {
                cadcraft_pointcloud::bim::ColumnShape::Circular { diameter } => {
                    let r = diameter / 2.0;
                    let segments = 16;
                    for i in 0..segments {
                        let a1 = (i as f64 * 360.0 / segments as f64).to_radians();
                        let a2 = ((i + 1) as f64 * 360.0 / segments as f64).to_radians();
                        let p1 = [col.center[0] + r * a1.cos(), col.center[1] + r * a1.sin()];
                        let p2 = [col.center[0] + r * a2.cos(), col.center[1] + r * a2.sin()];
                        let face = [[p1[0], p1[1], z0], [p2[0], p2[1], z0], [p2[0], p2[1], z1], [p1[0], p1[1], z1]];
                        s.sketch.add(&face, "RLV_3D_COLUMNS", true);
                    }
                }
                cadcraft_pointcloud::bim::ColumnShape::Rectangular { width, depth, rotation_deg } => {
                    let rad = rotation_deg.to_radians();
                    let cos_a = rad.cos();
                    let sin_a = rad.sin();
                    let hw = width / 2.0;
                    let hd = depth / 2.0;
                    let corners = [[-hw, -hd], [hw, -hd], [hw, hd], [-hw, hd]];
                    let world_pts: Vec<[f64; 2]> =
                        corners.iter().map(|[lx, ly]| [col.center[0] + lx * cos_a - ly * sin_a, col.center[1] + lx * sin_a + ly * cos_a]).collect();
                    for i in 0..4 {
                        let p1 = world_pts[i];
                        let p2 = world_pts[(i + 1) % 4];
                        let face = [[p1[0], p1[1], z0], [p2[0], p2[1], z0], [p2[0], p2[1], z1], [p1[0], p1[1], z1]];
                        s.sketch.add(&face, "RLV_3D_COLUMNS", true);
                    }
                }
            }
            col_count += 1;
        }
    }

    let mut slab_faces = 0usize;
    for (i, slab) in rlv.slabs.iter().enumerate() {
        if slab.polygon.len() < 3 {
            continue;
        }
        // The face a person sees from the room: the walking surface for the lowest slab, the
        // underside for every slab above it.
        let z = if i == 0 { slab.top_z } else { slab.base_z };
        let face: Vec<[f64; 3]> = slab.polygon.iter().map(|pt| [pt[0], pt[1], z]).collect();
        s.sketch.add(&face, "RLV_3D_SLABS", true);
        for hole in &slab.holes {
            if hole.len() >= 3 {
                let hface: Vec<[f64; 3]> = hole.iter().map(|pt| [pt[0], pt[1], z]).collect();
                s.sketch.add(&hface, "RLV_3D_SLABS", true);
            }
        }
        slab_faces += 1;
    }

    let (opening_count, doors, windows) = opening_counts(&per_storey);
    let ifc_json = match ifc_param(p)? {
        Some(path) => Some(write_ifc_file(&path, &building_from_rlv(&rlv, &per_storey))?),
        None => None,
    };

    let mut reply = json!({
        "message": format!(
            "Auto-3D Building: {} wall planes (two faces each), {} slab faces, {} openings ({} doors, {} windows)",
            wall_planes, slab_faces, opening_count, doors, windows
        ),
        "floorZ": rlv.floor_z,
        "ceilingZ": rlv.ceiling_z,
        "walls3d": wall_planes,
        "slabs": slab_faces,
        "columns": col_count,
        "parapets": rlv.parapets(),
        "openings": {
            "count": opening_count,
            "doors": doors,
            "windows": windows,
            "perStorey": per_storey.iter().map(Vec::len).collect::<Vec<_>>(),
            "fineBands": if path.is_empty() { "unavailable" } else { "streamed" },
        },
        "notAvailable": [],
        "drawn": {"walls": wall_planes, "slabs": slab_faces, "columns": col_count, "parapets": rlv.parapets()},
    });
    if let Some(ifc) = ifc_json {
        reply["ifc"] = ifc;
    }
    Ok(reply)
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
    let epsg = p.get("epsg").and_then(Value::as_u64).map(|v| v as u32).or_else(|| s.crs.as_ref().and_then(|c| c.epsg)).unwrap_or(31700);

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
        let _ = add_label(s, "TOPOLT_LABELS", [local[0] + 0.4, local[1] + 0.2], 0.35, format!("{} Z={:.2} ({})", pt.nr, pt.z, pt.code))?;
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
        .map(|a| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
        .ok_or_else(|| EngineError::Other("`station` [x, y, z] is required".to_string()))?;
    let tg = p
        .get("target")
        .and_then(Value::as_array)
        .filter(|a| a.len() >= 2)
        .map(|a| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
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
        .map(|a| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::test_las::write_test_las;
    use cadcraft_doc::Space as DocSpace;
    use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, DEFAULT_MAX_POINTS, Pt, Slice};

    /// A 6 x 4 m room split in two by a partition at x = 3, with 0.20 m walls, a floor at z = 0
    /// and a ceiling at z = 2.80. The points are the surfaces a scanner sees from inside: the
    /// floor and ceiling planes, the four room-side faces of the outer walls, and both faces of
    /// the partition. A bounding-box fake calls that six walls; the scan has five, and two rooms.
    ///
    /// The partition carries one 0.90 m door (y 1.90..2.80, up to z = 2.10): its rectangle has no
    /// points. Walls are sampled every 2 cm along their length and every 5 cm up them, so the
    /// 2 cm opening raster closes the seams between sample rows and the door stands alone as one
    /// hole instead of dissolving into them.
    fn room_points() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        // The floor and the ceiling, at 2 cm so the histogram's tallest bin is the plane and not
        // a wall level: a point cloud samples surfaces, and a wall contributes its area, not its
        // height, to any 5 cm Z bin.
        let mut i = 0;
        while i <= 300 {
            let x = f64::from(i) * 0.02;
            let mut j = 0;
            while j <= 200 {
                let y = f64::from(j) * 0.02;
                pts.push([x, y, 0.0]);
                pts.push([x, y, 2.8]);
                j += 1;
            }
            i += 1;
        }
        // The four room-side faces, one cell thick, running past the corners so they meet the
        // way walls do.
        let mut k = 0;
        while k <= 300 {
            let t = -0.01 + 0.02 * f64::from(k);
            let mut l = 0;
            while l <= 52 {
                let z = 0.1 + 0.05 * f64::from(l);
                pts.push([t, 0.2, z]);
                pts.push([t, 3.8, z]);
                l += 1;
            }
            k += 1;
        }
        let mut k = 0;
        while k <= 200 {
            let t = -0.01 + 0.02 * f64::from(k);
            let mut l = 0;
            while l <= 52 {
                let z = 0.1 + 0.05 * f64::from(l);
                pts.push([0.2, t, z]);
                pts.push([5.8, t, z]);
                l += 1;
            }
            k += 1;
        }
        // The partition, both faces seen: the pair that becomes one axis at x = 3, with the door
        // cut out of it.
        let mut u = 2.91;
        while u <= 3.09 + 1e-9 {
            let mut y = 0.2;
            while y <= 3.8 + 1e-9 {
                let in_door = (1.9 - 1e-9..=2.8 + 1e-9).contains(&y);
                let mut l = 0;
                while l <= 52 {
                    let z = 0.1 + 0.05 * f64::from(l);
                    if !(in_door && z <= 2.1 + 1e-9) {
                        pts.push([u, y, z]);
                    }
                    l += 1;
                }
                y += 0.02;
            }
            u += 0.02;
        }
        pts
    }

    fn session_with_room() -> Session {
        let pts = room_points();
        let mut bounds = Bounds3::EMPTY;
        let mut sliced = Vec::with_capacity(pts.len());
        let mut on_disk = Vec::with_capacity(pts.len());
        for p in &pts {
            bounds.add(p[0], p[1], p[2]);
            sliced.push(Pt { x: p[0], y: p[1], z: p[2], rgb: None, intensity: 100, class: 1 });
            on_disk.push((p[0], p[1], p[2], 1u8));
        }
        // The fine opening bands are read from the file, not from the display cloud: the fixture
        // has to exist on the disk and the cloud has to know where.
        let dir = std::env::temp_dir().join("cadcraft-autorlv-test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("room.las");
        write_test_las(&path, &on_disk);
        let total = sliced.len() as u64;
        let slice = Slice { points: sliced, total, in_band: total, bounds };
        let cloud =
            Cloud::from_slice("room".to_string(), path.display().to_string(), 0.0, 2.8, None, slice, DEFAULT_MAX_POINTS, CloudColor::Elevation);
        let mut s = Session::new();
        s.clouds.push(cloud);
        s
    }

    fn count_on_layer(s: &Session, layer: &str) -> usize {
        s.doc().ok().and_then(|d| d.space(&DocSpace::Model)).map(|store| store.iter().filter(|e| e.common.layer == layer).count()).unwrap_or(0)
    }

    #[test]
    fn autorlv_draws_the_walls_the_scan_has_and_not_a_bounding_box() {
        let mut s = session_with_room();
        let r = s.execute("autorlv", &json!({"index": 0}));
        assert!(r.is_ok(), "the pipeline runs: {r:?}");
        let reply = r.expect("the reply");
        assert_eq!(reply["walls"].as_u64(), Some(5), "the partition and four sides, not a fabricated six: {reply}");
        assert_eq!(reply["zones"].as_u64(), Some(2), "the partition splits the room in two: {reply}");
        assert!(reply["columns"].as_u64().is_some());
        assert!(reply["parapets"].as_u64().is_some());
        assert_eq!(reply["drawn"]["columns"].as_u64(), reply["columns"].as_u64());
        assert_eq!(reply["drawn"]["parapets"].as_u64(), reply["parapets"].as_u64());
        assert_eq!(reply["drawn"]["walls"].as_u64(), reply["walls"].as_u64(), "every found wall is drawn: {reply}");
        assert_eq!(reply["drawn"]["zones"].as_u64(), reply["zones"].as_u64(), "and every zone: {reply}");
        assert_eq!(count_on_layer(&s, "RLV_WALLS"), 5);
        assert_eq!(count_on_layer(&s, "RLV_ZONES"), 2);
        assert_eq!(count_on_layer(&s, "RLV_SLABS"), 2, "the floor and the ceiling");
        assert_eq!(count_on_layer(&s, "BIM_WALLS"), 0, "the fabricated layer is not written");
        assert_eq!(count_on_layer(&s, "RLV_OPENINGS"), 2, "two jamb lines for the one door: {reply}");
        assert_eq!(reply["openings"]["count"].as_u64(), Some(1), "the partition's door: {reply}");
        assert_eq!(reply["openings"]["doors"].as_u64(), Some(1), "and it is a door, not a window: {reply}");
        assert_eq!(reply["openings"]["windows"].as_u64(), Some(0), "{reply}");
        assert!(reply["deviation"]["rmseMm"].as_f64().is_some_and(|v| v.is_finite()), "the scan is measured against the model: {reply}");
        assert_eq!(reply["notAvailable"].as_array().map(Vec::len), Some(0), "all stages implemented: {reply}");
        assert!(count_on_layer(&s, "RLV_DIMS") > 0, "dimensions are placed on RLV_DIMS: {reply}");
        assert!(reply["drawn"]["dimensions"].as_u64().is_some_and(|n| n > 0), "drawn dimensions count reported: {reply}");
    }

    #[test]
    fn autorlv_writes_a_self_checked_ifc_file_when_it_is_asked_for() {
        let mut s = session_with_room();
        let path = std::env::temp_dir().join("cadcraft-autorlv.ifc");
        let r = s.execute("autorlv", &json!({"index": 0, "ifc": path.display().to_string()}));
        let reply = r.expect("the file writes");
        assert!(path.is_file(), "the IFC file is on the disk: {reply}");
        let text = std::fs::read_to_string(&path).expect("the file reads");
        assert!(text.contains("IFCWALL"), "the walls reach the file: {reply}");
        assert_eq!(reply["ifc"]["doors"].as_u64(), Some(1), "the door reaches the file: {reply}");
        assert!(reply["ifc"]["entities"].as_u64().is_some_and(|n| n > 0), "{reply}");
    }

    #[test]
    fn autorlv_without_a_cloud_refuses_instead_of_modelling_a_fake_apartment() {
        let mut s = Session::new();
        let r = s.execute("autorlv", &json!({}));
        assert!(r.is_err(), "no cloud, no model: {r:?}");
        assert_eq!(count_on_layer(&s, "RLV_WALLS"), 0, "and nothing is drawn from a fallback scan");
    }

    #[test]
    fn auto3d_reconstructs_3d_faces_and_notavailable_is_empty() {
        let mut s = session_with_room();
        let r = s.execute("auto3d", &json!({"index": 0}));
        assert!(r.is_ok(), "the 3d pipeline runs: {r:?}");
        let reply = r.expect("the reply");
        assert_eq!(reply["notAvailable"].as_array().map(Vec::len), Some(0));
        assert!(reply["slabs"].as_u64().is_some_and(|n| n >= 2));
        assert!(reply["walls3d"].as_u64().is_some_and(|n| n >= 5));
        assert!(reply["columns"].as_u64().is_some());
        assert!(reply["parapets"].as_u64().is_some());
        assert!(reply["drawn"]["columns"].as_u64().is_some());
        assert!(reply["drawn"]["parapets"].as_u64().is_some());
    }
}
