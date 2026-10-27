//! MCP tool definitions and dispatch.

use serde_json::{Value, json};

use crate::backend::Backend;

/// The `tools/list` payload: one definition (name, description, input schema) per MCP tool,
/// including the UI-only tools when `has_ui` says a window is available.
pub fn tool_definitions(has_ui: bool) -> Value {
    let mut tools = vec![
        json!({"name": "command_line", "description": "Type one line at the CADCraft command line, exactly as a user would at an AutoCAD-style prompt. Starts commands by name or alias (LINE, L, CIRCLE, C, PLINE, RECTANG, MOVE, TRIM, OFFSET, ZOOM…) and answers the active prompt: points `x,y`, relative `@dx,dy`, polar `@dist<angle`, distances, keywords (e.g. `c` for Close), empty text = Enter. Spaces act as Enter, so `circle 0,0 5` works in one call. Returns the new prompt and command-line output, and `error` when the line was refused.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "script", "description": "Run a multi-line script (like an AutoCAD .scr file): one or more inputs per line, blank line = Enter.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "execute", "description": "Run any command with JSON parameters (no prompts or dialogs). Use list_commands to find ids and parameter docs. Examples: line {points:[[0,0],[10,0]]}, circle {center:[0,0], radius:5}, layer.new {name:\"Walls\", color:\"red\", current:true}, move {handles:[\"10A\"], delta:[5,0]}, zoom {mode:\"extents\"}.",
            "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}, "required": ["command"]}}),
        json!({"name": "list_commands", "description": "List commands with ids, labels, menu paths, aliases and parameter docs. Optional filter: a substring of the command id, label or an alias.",
            "inputSchema": {"type": "object", "properties": {"filter": {"type": "string"}}}}),
        json!({"name": "inspect_drawing", "description": "Summary of the active drawing: entity counts, extents, layers, styles, blocks, layouts, selection, undo history and (optionally) entities with handles and geometry.",
            "inputSchema": {"type": "object", "properties": {"entities": {"type": "boolean"}, "limit": {"type": "integer"}}}}),
        json!({"name": "query_entities", "description": "Entities filtered by type (Line, Circle, Polyline…), layer, current selection (selected:true; active space only) and/or a window [[x0,y0],[x1,y1]] (objects whose geometry is inside or crosses it; crossing:false = entirely inside). Returns count (all matches), returned (this page) and entities; page with offset and limit (default 500).",
            "inputSchema": {"type": "object", "properties": {"type": {"type": "string"}, "layer": {"type": "string"}, "selected": {"type": "boolean"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "crossing": {"type": "boolean"}, "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "render", "description": "Render the active drawing to a PNG and return it as an image: fitted to the extents by default, or just the area `window` [[x0,y0],[x1,y1]] (drawing units), e.g. one sheet or one room of a large drawing. With a window and no size the image is 1600 px wide with the window's shape.",
            "inputSchema": {"type": "object", "properties": {"width": {"type": "integer", "minimum": 16}, "height": {"type": "integer", "minimum": 16}, "fit": {"type": "boolean"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "path": {"type": "string"}}}}),
        json!({"name": "find_text", "description": "Read the drawing's texts: TEXT, MTEXT (formatting removed), block attributes and multileaders, each with its text, insertion point `at`, height, rotation, style, layer and handle. Filter by `contains` (any case), `layer` and/or `window` [[x0,y0],[x1,y1]] around the insertion points; page with offset and limit (default 200). Use it to find room names, sheet titles and notes without reading all the geometry.",
            "inputSchema": {"type": "object", "properties": {"contains": {"type": "string"}, "layer": {"type": "string"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "new_drawing", "description": "Create a new drawing (imperial by default, or metric).",
            "inputSchema": {"type": "object", "properties": {"metric": {"type": "boolean"}}}}),
        json!({"name": "open", "description": "Open a DXF or DWG file.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "save", "description": "Save the drawing (DXF or DWG by extension). A .svg, .png or .pdf path exports instead: the drawing keeps its name and unsaved changes.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}}),
        json!({"name": "cancel", "description": "Press Escape: cancel the running command (or clear the selection).", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "osm_import", "description": "Import OpenStreetMap (.osm XML or Overpass JSON) with buildings, postal numbers (addr:housenumber, addr:street), roads and parcels projected into the drawing's CRS (default Stereo 70 EPSG:31700).",
            "inputSchema": {"type": "object", "properties": {"xml": {"type": "string"}, "json": {"type": "string"}, "epsg": {"type": "integer", "minimum": 1024}}}}),
        json!({"name": "crs_transform", "description": "Set the project Coordinate Reference System (Stereo 70 EPSG:31700, ETRS89 EPSG:4258/3035, WGS84 EPSG:4326, Web Mercator EPSG:3857, UTM zones) and optional local falseEasting/falseNorthing offsets.",
            "inputSchema": {"type": "object", "properties": {"epsg": {"type": "integer", "minimum": 1024}, "name": {"type": "string"}, "falseEasting": {"type": "number"}, "falseNorthing": {"type": "number"}}}}),
        json!({"name": "pointcloud_rlv", "description": "Run Auto-RLV 2D on the active LAS point cloud: finds the floor/ceiling slabs, splits the scan into storeys, traces the walls, closes the room zones, searches every wall's elevation band for door and window openings, and measures the cloud-to-model deviation, routing the points the model cannot explain onto RLV_FURNITURE. Draws wall axes on RLV_WALLS, zone rings on RLV_ZONES, slab polygons on RLV_SLABS, columns on RLV_COLUMNS, jamb lines on RLV_OPENINGS, room dimensions on RLV_DIMS and the room schedule on RLV_SCHEDULE, and returns areas, perimeters, column/parapet counts, opening counts and the deviation statistics. Pass ifc to also write an IFC 4 STEP file with the same model; its entity graph is read back and checked by this repository's own IFC reader.",
            "inputSchema": {"type": "object", "properties": {"index": {"type": "integer", "minimum": 0}, "ifc": {"type": "string", "description": "Path to write the IFC 4 STEP file to; omitted means nothing is written"}, "params": {"type": "object", "description": "Cloud2BIM knobs: cell, z_step, slab_ratio, slice_from, slice_to, threshold, close_radius, epsilon, safety_margin, single_sided_slab_thickness, min_wall_length, min_wall_thickness, max_wall_thickness, angle_tolerance_deg, exterior_wall_thickness, exterior_scan, zone_gap, min_zone_area, opening_cell, opening_threshold, opening_rank, min_opening_width, max_opening_width, door_min_height, door_max_sill, window_min_sill, opening_band, opening_decimate, max_wall_band_points, deviation_threshold, deviation_cell, min_furniture_points, max_furniture_clusters, min_column_size, max_column_size, max_column_aspect_ratio"}}}}),
        json!({"name": "pointcloud_auto3d", "description": "Reconstruct the 3D structure of the building from the active LAS point cloud: one extruded wall per detected wall axis (two face planes each) on RLV_3D_WALLS, columns on RLV_3D_COLUMNS, and the detected floor/ceiling slab polygons on RLV_3D_SLABS. It reads each wall's elevation band for openings and reports their counts, and models only surfaces the scan has: no roof is invented. Pass ifc to write the same model as an IFC 4 STEP file.",
            "inputSchema": {"type": "object", "properties": {"index": {"type": "integer", "minimum": 0}, "ifc": {"type": "string", "description": "Path to write the IFC 4 STEP file to; omitted means nothing is written"}, "params": {"type": "object", "description": "Cloud2BIM knobs, the same as pointcloud_rlv except the deviation and furniture ones, which Auto-3D does not compute"}}}}),
        json!({"name": "pointcloud_classify", "description": "Run AI semantic & geometric topographic classification on the point cloud (ground, building, fence, pole, road, curb, ditch, vegetation) and automatically vectorize features onto CAD layers.",
            "inputSchema": {"type": "object", "properties": {"index": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "topolt_survey", "description": "TopoLT module: report survey points from .coo/.csv text onto CAD layers with topographic symbols and elevations, or compute stakeout (pichetare) and radiation (radiere).",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}, "station": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 3}, "target": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 3}}}}),
        json!({"name": "topocad_terrain", "description": "TopoCAD module: build Digital Terrain Model (DTM / TIN), generate major/minor contour lines (curbe de nivel) and compute earthwork cut/fill volumes.",
            "inputSchema": {"type": "object", "properties": {"minorStep": {"type": "number"}, "majorEvery": {"type": "integer", "minimum": 1}, "referenceZ": {"type": "number"}}}}),
        json!({"name": "cadgen_cadastral", "description": "CADGen module: subdivide a parcel polygon to detach an exact target area (m²) or generate a Romanian ANCPI Cadastral PAD plan and Stereo 70 coordinate inventory table.",
            "inputSchema": {"type": "object", "properties": {"parcelId": {"type": "string"}, "targetArea": {"type": "number"}, "vertical": {"type": "boolean"}, "epsg": {"type": "integer", "minimum": 1024}}}}),
    ];
    if has_ui {
        tools.push(json!({"name": "screenshot", "description": "Screenshot the CADCraft window (PNG).", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}}));
        tools.push(json!({"name": "ui_inspect", "description": "UI state: panels, canvas rect, view, cursor and snap, performance.", "inputSchema": {"type": "object", "properties": {}}}));
        tools.push(json!({"name": "ui_click", "description": "Click in the drawing area at world coordinates (or screen with space:\"screen\"). Right button = Enter.", "inputSchema": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "space": {"type": "string"}, "button": {"type": "string"}}, "required": ["x", "y"]}}));
    }
    json!({ "tools": tools })
}

/// Wrap a JSON value as a successful MCP result containing pretty-printed text.
fn text(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

/// Wrap an error message as an MCP tool error result (`isError`), not a protocol error.
fn err_text(e: &str) -> Value {
    json!({"content": [{"type": "text", "text": e}], "isError": true})
}

/// Whether `v` fits the subset of JSON Schema the tool definitions use: `type`, `items`,
/// `minItems`, `maxItems` and `minimum`.
fn fits(v: &Value, schema: &Value) -> bool {
    let typed = match schema["type"].as_str() {
        Some("string") => v.is_string(),
        // JSON Schema integers include numbers such as 2.0.
        Some("integer") => v.as_f64().is_some_and(|f| f.fract() == 0.0),
        Some("number") => v.is_number(),
        Some("boolean") => v.is_boolean(),
        Some("array") => v.is_array(),
        Some("object") => v.is_object(),
        _ => true,
    };
    let len = v.as_array().map(Vec::len);
    typed
        && schema["minimum"].as_f64().is_none_or(|m| v.as_f64().is_none_or(|f| f >= m))
        && schema["minItems"].as_u64().is_none_or(|m| len.is_none_or(|n| n as u64 >= m))
        && schema["maxItems"].as_u64().is_none_or(|m| len.is_none_or(|n| n as u64 <= m))
        && (schema["items"].is_null() || v.as_array().is_none_or(|a| a.iter().all(|i| fits(i, &schema["items"]))))
}

/// Arguments checked against the tool's input schema: an object, required properties present and
/// each given property fitting its schema (`null` counts as not given). `Err` is the
/// invalid-params message.
fn check_args(name: &str, args: &Value) -> Result<Value, String> {
    let defs = tool_definitions(true);
    let schema = defs["tools"].as_array().and_then(|t| t.iter().find(|t| t["name"] == name)).map(|t| t["inputSchema"].clone());
    let Some(schema) = schema else { return Err(format!("unknown tool `{name}`")) };
    let a = match args {
        Value::Null => json!({}),
        Value::Object(_) => args.clone(),
        _ => return Err(format!("`arguments` of `{name}` must be an object")),
    };
    for key in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if a.get(key).is_none_or(Value::is_null) {
            return Err(format!("`{name}` needs `{key}`"));
        }
    }
    for (key, prop) in schema["properties"].as_object().into_iter().flatten() {
        if let Some(v) = a.get(key).filter(|v| !v.is_null())
            && !fits(v, prop)
        {
            return Err(format!("`{key}` of `{name}` must match {prop}"));
        }
    }
    Ok(a)
}

/// Whether a command-catalog entry's id, label or one of its aliases contains `f` (lower case).
fn command_matches(c: &Value, f: &str) -> bool {
    let has = |s: &str| s.to_ascii_lowercase().contains(f);
    ["id", "label"].iter().any(|k| c[*k].as_str().is_some_and(has))
        || c["aliases"].as_array().is_some_and(|a| a.iter().filter_map(Value::as_str).any(has))
}

/// Run a tool. `Err` is a protocol error (unknown tool, or arguments that don't fit its input
/// schema), answered with JSON-RPC invalid params; a failing tool is an `isError` result.
pub fn call_tool(b: &mut dyn Backend, name: &str, args: &Value) -> Result<Value, String> {
    let a = check_args(name, args)?;
    let r = match name {
        "command_line" => b.call("cmdline.input", a),
        "script" => b.call("cmdline.script", a),
        "execute" => b.call(
            "engine.execute",
            json!({"command": a.get("command").cloned().unwrap_or(Value::Null), "params": a.get("params").cloned().unwrap_or(json!({}))}),
        ),
        "list_commands" => b.call("engine.commands", json!({})).map(|v| {
            let f = a.get("filter").and_then(Value::as_str).map(str::to_ascii_lowercase);
            match (v, f) {
                (Value::Array(items), Some(f)) => Value::Array(items.into_iter().filter(|c| command_matches(c, &f)).collect()),
                (v, _) => v,
            }
        }),
        "inspect_drawing" => b.call("drawing.inspect", a),
        "query_entities" => b.call("engine.execute", json!({"command": "entities", "params": a})),
        "find_text" => b.call("engine.execute", json!({"command": "texts", "params": a})),
        "new_drawing" => b.call("engine.execute", json!({"command": "new", "params": a})),
        "open" => b.call("app.open", a),
        "save" => b.call("app.save", a),
        "cancel" => b.call("cmdline.key", json!({"key": "escape"})),
        "osm_import" => b.call("engine.execute", json!({"command": "osmimport", "params": a})),
        "crs_transform" => b.call("engine.execute", json!({"command": "lascrs", "params": a})),
        "pointcloud_rlv" => b.call("engine.execute", json!({"command": "autorlv", "params": a})),
        "pointcloud_auto3d" => b.call("engine.execute", json!({"command": "auto3d", "params": a})),
        "pointcloud_classify" => b.call("engine.execute", json!({"command": "autovectorize", "params": a})),
        "topolt_survey" => {
            if a.get("station").is_some() && a.get("target").is_some() {
                b.call("engine.execute", json!({"command": "pichetare", "params": a}))
            } else {
                b.call("engine.execute", json!({"command": "raportcoo", "params": a}))
            }
        }
        "topocad_terrain" => {
            if a.get("referenceZ").is_some() {
                b.call("engine.execute", json!({"command": "volumetopo", "params": a}))
            } else {
                b.call("engine.execute", json!({"command": "curbenivel", "params": a}))
            }
        }
        "cadgen_cadastral" => {
            if a.get("targetArea").is_some() {
                b.call("engine.execute", json!({"command": "parcelare", "params": a}))
            } else {
                b.call("engine.execute", json!({"command": "padcadastru", "params": a}))
            }
        }
        "screenshot" => b.call("ui.screenshot", a),
        "ui_inspect" => b.call("ui.inspect", json!({})),
        "ui_click" => b.call("ui.pointer", a),
        "render" => {
            return Ok(match b.call("ui.render", a) {
                Ok(v) => match v.get("pngBase64").and_then(Value::as_str) {
                    Some(data) => json!({"content": [{"type": "image", "data": data, "mimeType": "image/png"}]}),
                    None => text(&v),
                },
                Err(e) => err_text(&e),
            });
        }
        other => return Err(format!("unknown tool `{other}`")),
    };
    Ok(match r {
        Ok(v) => text(&v),
        Err(e) => err_text(&e),
    })
}
