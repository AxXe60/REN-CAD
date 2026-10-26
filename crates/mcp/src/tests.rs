//! An agent completes realistic drafting tasks through MCP only, and verifies through MCP only.

use serde_json::{Value, json};

use crate::{Headless, Server};

/// Send one JSON-RPC request to the server and return the parsed reply; fails the test if the
/// server does not answer or answers with invalid JSON.
fn call(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let r = s.handle_line(&line).expect("reply");
    serde_json::from_str(&r).unwrap()
}

/// Call an MCP tool and return its decoded result: JSON text is parsed, an image is passed
/// through, and a tool error fails the test with the reported message.
fn tool(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, 99, "tools/call", json!({"name": name, "arguments": args}));
    let c = &r["result"]["content"][0];
    assert_ne!(r["result"]["isError"], json!(true), "{name} failed: {c}");
    if c["type"] == "text" { serde_json::from_str(c["text"].as_str().unwrap()).unwrap_or(Value::Null) } else { c.clone() }
}

/// The MCP handshake works: `initialize` names the server, the `initialized` notification is
/// accepted, `tools/list` offers the command line, and an unknown method is -32601.
#[test]
fn lifecycle_and_tools() {
    let mut s = Server::new(Box::new(Headless::default()));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "cadcraft");
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(s.is_initialized());
    let tools = call(&mut s, 2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "command_line"));
    let bad = call(&mut s, 3, "nope", json!({}));
    assert_eq!(bad["error"]["code"], -32601);
}

/// Guards that an agent can complete a drawing (plate, holes, layer, text) using only the MCP
/// tools, and verify it only through MCP: counts, selection queries, undo and a PNG render.
#[test]
fn agent_draws_a_plate_with_holes() {
    let mut s = Server::new(Box::new(Headless::default()));
    // Task 1: an outline via the command line, AutoCAD style.
    tool(&mut s, "command_line", json!({"text": "rectang 0,0 100,60"}));
    // Task 2: four holes with JSON.
    for (x, y) in [(10, 10), (90, 10), (10, 50), (90, 50)] {
        tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [x, y], "radius": 4}}));
    }
    // Task 3: a layer and a centre line on it.
    tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "Center", "color": "red", "current": true}}));
    tool(&mut s, "command_line", json!({"text": "line -5,30 105,30"}));
    tool(&mut s, "command_line", json!({"text": ""}));
    // Task 4: a script with text. TEXT keeps asking for further lines until an empty one, as in
    // AutoCAD, so the script ends it with a blank line (= Enter).
    let r = tool(&mut s, "script", json!({"text": "TEXT 0,-10 5 0 PLATE 100x60\n\n"}));
    assert_eq!(r["state"]["running"], Value::Null, "TEXT must be finished: {r}");
    // Verify only through MCP.
    let d = tool(&mut s, "inspect_drawing", json!({}));
    assert_eq!(d["counts"]["Circle"], 4);
    assert_eq!(d["counts"]["Polyline"], 1);
    assert_eq!(d["counts"]["Line"], 1);
    assert_eq!(d["counts"]["Text"], 1);
    let lines = tool(&mut s, "query_entities", json!({"type": "Line", "layer": "Center"}));
    assert_eq!(lines["count"], 1);
    // Task 5: undo the text, check, then render.
    let u = tool(&mut s, "execute", json!({"command": "undo"}));
    assert_eq!(u["undone"], json!(["Single Line Text"]), "{u}");
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert!(d["counts"].get("Text").is_none());
    assert_eq!(d["counts"]["Circle"], 4);
    assert_eq!(d["counts"]["Polyline"], 1);
    assert_eq!(d["counts"]["Line"], 1);
    let lines = tool(&mut s, "query_entities", json!({"type": "Line", "layer": "Center"}));
    assert_eq!(lines["count"], 1, "the centre line must survive undoing the text");
    let img = tool(&mut s, "render", json!({"width": 320, "height": 200}));
    assert_eq!(img["type"], "image");
    assert!(img["data"].as_str().unwrap().len() > 100);
}

/// Guards that bad input is reported, not fatal: a refused command and a screenshot without a
/// UI are `isError` results, and malformed JSON is a parse error (-32700).
#[test]
fn errors_are_tool_errors_not_crashes() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = call(&mut s, 1, "tools/call", json!({"name": "execute", "arguments": {"command": "circle", "params": {"center": "x"}}}));
    assert_eq!(r["result"]["isError"], true);
    let r = call(&mut s, 2, "tools/call", json!({"name": "screenshot", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
    assert!(s.handle_line("{not json").unwrap().contains("-32700"));
}

/// Unknown tools and arguments that don't fit a tool's schema are invalid params (-32602), never
/// guessed at; an empty batch is one Invalid Request.
#[test]
fn bad_tool_calls_are_invalid_params() {
    let mut s = Server::new(Box::new(Headless::default()));
    tool(&mut s, "command_line", json!({"text": "circle 0,0 1"}));
    let bad = [
        json!({"name": "nope", "arguments": {}}),
        json!({"name": "command_line", "arguments": {}}),
        json!({"name": "command_line", "arguments": {"text": 5}}),
        json!({"name": "command_line", "arguments": "circle 1,1 1"}),
        json!({"name": "script", "arguments": {}}),
        json!({"name": "query_entities", "arguments": {"window": [-5, -5, 5, 5]}}),
        json!({"name": "query_entities", "arguments": {"window": [[-5, -5]]}}),
        json!({"name": "query_entities", "arguments": {"window": {"min": [-5, -5], "max": [5, 5]}}}),
        json!({"name": "query_entities", "arguments": {"type": 7}}),
        json!({"name": "query_entities", "arguments": {"layer": ["0"]}}),
        json!({"name": "query_entities", "arguments": {"offset": -1}}),
        json!({"name": "query_entities", "arguments": {"selected": "true"}}),
        json!({"name": "query_entities", "arguments": {"selected": 1}}),
    ];
    for (i, params) in bad.into_iter().enumerate() {
        let r = call(&mut s, i as u64, "tools/call", params.clone());
        assert_eq!(r["error"]["code"], -32602, "{params} -> {r}");
    }
    // Nothing was typed: the circle is still the only object and no command is running.
    let d = tool(&mut s, "inspect_drawing", json!({"entities": false}));
    assert_eq!(d["entityCount"], 1, "{d}");
    assert_eq!(s.handle_line("[]").map(|r| serde_json::from_str::<Value>(&r).unwrap()["error"]["code"].clone()), Some(json!(-32600)));
}

/// `open` reads both DXF and DWG, so `tools/list` must advertise both (#1875).
#[test]
fn open_tool_description_names_dxf_and_dwg() {
    let mut s = Server::new(Box::new(Headless::default()));
    let tools = call(&mut s, 1, "tools/list", json!({}));
    let open = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "open").unwrap();
    let desc = open["description"].as_str().unwrap();
    assert!(desc.contains("DXF"), "`open` must still name DXF: {desc}");
    assert!(desc.contains("DWG"), "`open` reads DWG too: {desc}");
}

/// Guards that `query_entities` advertises its `selected` filter and honours it (active-space
/// selection only, both ways) without changing the drawing.
#[test]
fn query_entities_selection_filter_is_discoverable_and_forwarded() {
    let mut s = Server::new(Box::new(Headless::default()));
    let tools = call(&mut s, 1, "tools/list", json!({}));
    let query = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "query_entities").unwrap();
    assert_eq!(query["inputSchema"]["properties"]["selected"]["type"], "boolean");
    tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [0, 0], "radius": 2}}));
    tool(&mut s, "execute", json!({"command": "circle", "params": {"center": [10, 0], "radius": 2}}));
    assert_eq!(tool(&mut s, "query_entities", json!({"selected": true}))["count"], 0);
    tool(&mut s, "execute", json!({"command": "select", "params": {"window": [[-3, -3], [3, 3]]}}));
    let before = tool(&mut s, "inspect_drawing", json!({}));
    let selected = tool(&mut s, "query_entities", json!({"selected": true, "type": "Circle"}));
    assert_eq!(selected["count"], 1);
    assert_eq!(selected["entities"][0]["geometry"]["center"]["x"], 0.0);
    assert_eq!(tool(&mut s, "query_entities", json!({"selected": false}))["count"], 2);
    assert_eq!(tool(&mut s, "inspect_drawing", json!({})), before);
}

/// `count` is every match while `returned` is the page; a window matches geometry, not bounding boxes.
#[test]
fn query_entities_pages_and_windows_by_geometry() {
    let mut s = Server::new(Box::new(Headless::default()));
    for c in ["circle 0,0 1", "circle 100,100 1", "circle 200,0 1"] {
        tool(&mut s, "command_line", json!({"text": c}));
    }
    // An arc whose bounding box covers the window but whose curve stays outside it.
    tool(&mut s, "execute", json!({"command": "arc", "params": {"p1": [40, 0], "p2": [50, 10], "p3": [60, 0]}}));
    let r = tool(&mut s, "query_entities", json!({"type": "Circle", "limit": 1, "offset": 1}));
    assert_eq!((r["count"].clone(), r["returned"].clone()), (json!(3), json!(1)), "{r}");
    assert_eq!(r["entities"][0]["geometry"]["center"]["x"], 100.0, "{r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[45, 1], [55, 5]]}));
    assert_eq!(r["count"], 0, "the arc passes above this window: {r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[-5, -5], [55, 5]]}));
    assert_eq!(r["count"], 2, "the circle at 0,0 and the arc crossing the window: {r}");
    let r = tool(&mut s, "query_entities", json!({"window": [[-5, -5], [55, 5]], "crossing": false}));
    assert_eq!(r["count"], 1, "only the circle is entirely inside: {r}");
}

/// Guards that the `list_commands` filter matches ids, labels and aliases but not parameter
/// docs, so searching `circle` does not return QSELECT merely for mentioning circles.
#[test]
fn list_commands_filter_matches_names_not_docs() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = tool(&mut s, "list_commands", json!({"filter": "circle"}));
    let ids: Vec<&str> = r.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"circle"), "{ids:?}");
    assert!(!ids.contains(&"qselect"), "QSELECT only mentions circles in its docs: {ids:?}");
    let r = tool(&mut s, "list_commands", json!({"filter": "pl"}));
    assert!(r.as_array().unwrap().iter().any(|c| c["id"] == "pline"), "aliases and ids match");
}

/// Guards that `find_text` returns TEXT and MTEXT with their placement, strips MTEXT formatting,
/// and filters by text, layer and window; a windowed render keeps that area's aspect ratio.
#[test]
fn find_text_reads_texts_and_render_shows_one_area() {
    let mut s = Server::new(Box::new(Headless::default()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "Texto"}}));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [1, 1], "height": 0.15, "text": "RECAMARA 1"}}));
    tool(&mut s, "execute", json!({"command": "text", "params": {"at": [50, 1], "height": 0.15, "text": "COCINA"}}));
    tool(
        &mut s,
        "execute",
        json!({"command": "mtext", "params": {"at": [1, 5], "height": 0.2, "width": 5, "text": "{\\fArial|b1;PLANTA} BAJA\\PNIVEL 0.00"}}),
    );
    tool(&mut s, "execute", json!({"command": "rectang", "params": {"p1": [0, 0], "p2": [10, 5]}}));

    let all = tool(&mut s, "find_text", json!({}));
    assert_eq!(all["count"], 3);
    let r = tool(&mut s, "find_text", json!({"contains": "recamara"}));
    assert_eq!(r["count"], 1);
    assert_eq!(r["texts"][0]["text"], "RECAMARA 1");
    assert_eq!(r["texts"][0]["at"], json!([1.0, 1.0]));
    assert_eq!(r["texts"][0]["height"], 0.15);
    // Multiline text comes back readable, without its formatting codes.
    let m = tool(&mut s, "find_text", json!({"contains": "planta"}));
    let t = m["texts"][0]["text"].as_str().unwrap();
    assert!(t.contains("PLANTA BAJA") && t.contains("NIVEL 0.00") && !t.contains('\\') && !t.contains('{'), "{t}");
    // By area: only what is inside the window.
    let w = tool(&mut s, "find_text", json!({"window": [[-1, -1], [20, 20]]}));
    assert_eq!(w["count"], 2);
    assert_eq!(tool(&mut s, "find_text", json!({"layer": "nope"}))["count"], 0);

    // A render of one area takes the area's shape.
    let r = call(&mut s, 9, "tools/call", json!({"name": "render", "arguments": {"window": [[0, 0], [10, 5]]}}));
    let c = &r["result"]["content"][0];
    assert_eq!(c["type"], "image", "{r}");
    let png = cadcraft_engine::cmd::file::base64_decode(c["data"].as_str().unwrap()).unwrap();
    let (w, h) = (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
    assert_eq!((w, h), (1600, 800));
}

/// Guards a lazy Remote bridge: it serves tools without the app, reports how to start it
/// (`--control`), and reaches the app once that app starts listening on the port.
#[test]
fn a_lazy_bridge_starts_without_the_app_and_says_how_to_open_it() {
    // Nothing listens on a port that was just freed.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let mut s = Server::new(Box::new(crate::Remote::lazy(&addr, false)));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "cadcraft");
    let tools = call(&mut s, 2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "screenshot"));
    let r = call(&mut s, 3, "tools/call", json!({"name": "command_line", "arguments": {"text": "line 0,0 1,1"}}));
    assert_eq!(r["result"]["isError"], json!(true));
    let msg = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(msg.contains("not answering") && msg.contains("--control"), "{msg}");

    // Once the app listens, the next call reaches it.
    let listener = std::net::TcpListener::bind(&addr).unwrap();
    let app = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        let (sock, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(sock.try_clone().unwrap()).read_line(&mut line).unwrap();
        let id = serde_json::from_str::<Value>(&line).unwrap()["id"].clone();
        writeln!(&sock, "{}", json!({"id": id, "ok": true, "result": {"prompt": "Command:"}})).unwrap();
    });
    let r = call(&mut s, 4, "tools/call", json!({"name": "command_line", "arguments": {"text": ""}}));
    assert_ne!(r["result"]["isError"], json!(true), "{r}");
    app.join().unwrap();
}

/// A 6 x 4 m room split in two by a partition at x = 3, 0.20 m walls, floor at z = 0 and ceiling
/// at z = 2.80. Only the surfaces an interior scan sees are sampled, so the modelling commands
/// must find five walls and two rooms — a bounding box would report six walls and one room.
fn headless_with_room_scan() -> Headless {
    use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, DEFAULT_MAX_POINTS, Pt, Slice};

    let mut pts = Vec::new();
    // Dense floor and ceiling planes: the slab histogram's tallest bin must be a plane, never a
    // wall level.
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
    let levels = [0.1, 0.9, 1.7, 2.45, 2.55, 2.65];
    let mut k = 0;
    while k <= 300 {
        let t = -0.01 + 0.02 * f64::from(k);
        for z in levels {
            pts.push([t, 0.2, z]);
            pts.push([t, 3.8, z]);
        }
        k += 1;
    }
    let mut k = 0;
    while k <= 200 {
        let t = -0.01 + 0.02 * f64::from(k);
        for z in levels {
            pts.push([0.2, t, z]);
            pts.push([5.8, t, z]);
        }
        k += 1;
    }
    let mut u = 2.91;
    while u <= 3.09 + 1e-9 {
        let mut y = 0.2;
        while y <= 3.8 + 1e-9 {
            for z in levels {
                pts.push([u, y, z]);
            }
            y += 0.1;
        }
        u += 0.02;
    }

    let mut bounds = Bounds3::EMPTY;
    let mut sliced = Vec::with_capacity(pts.len());
    for p in &pts {
        bounds.add(p[0], p[1], p[2]);
        sliced.push(Pt { x: p[0], y: p[1], z: p[2], rgb: None, intensity: 100, class: 1 });
    }
    let total = sliced.len() as u64;
    let slice = Slice { points: sliced, total, in_band: total, bounds };
    let cloud = Cloud::from_slice("room".to_string(), String::new(), 0.0, 2.8, None, slice, DEFAULT_MAX_POINTS, CloudColor::Elevation);
    let mut h = Headless::default();
    h.session.clouds.push(cloud);
    h
}

/// The modelling tools run on the room scan loaded above, and every claim in their reply is
/// checked against the drawing they made.
#[test]
fn rencad_mcp_tools_execute_end_to_end() {
    let mut s = Server::new(Box::new(headless_with_room_scan()));
    call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));

    let crs = tool(&mut s, "crs_transform", json!({"epsg": 31700}));
    assert_eq!(crs["crs"]["epsg"], 31700);

    let osm = tool(&mut s, "osm_import", json!({"epsg": 31700}));
    assert!(osm["buildings"].as_u64().unwrap_or(0) >= 1, "{osm}");
    assert!(osm["addresses"].as_u64().unwrap_or(0) >= 1, "{osm}");

    let rlv = tool(&mut s, "pointcloud_rlv", json!({"index": 0}));
    assert_eq!(rlv["walls"].as_u64(), Some(5), "four room sides and the partition, not a bounding box: {rlv}");
    assert_eq!(rlv["zones"].as_u64(), Some(2), "the partition splits the room in two: {rlv}");
    assert_eq!(rlv["slabs"].as_u64(), Some(2), "floor and ceiling: {rlv}");
    assert_eq!(rlv["rooms"].as_array().map(Vec::len), Some(2), "{rlv}");
    assert!(rlv["totalUsableAreaM2"].as_f64().unwrap_or(0.0) > 15.0, "{rlv}");
    assert_eq!(rlv["notAvailable"], json!(["dimensions"]), "{rlv}");
    assert!(rlv["openings"]["count"].as_u64().is_some(), "the wall search reports: {rlv}");
    assert!(rlv["deviation"]["rmseMm"].as_f64().is_some(), "the deviation is measured: {rlv}");
    assert_eq!(rlv["drawn"]["walls"].as_u64(), rlv["walls"].as_u64(), "{rlv}");
    let walls = tool(&mut s, "query_entities", json!({"layer": "RLV_WALLS"}));
    assert_eq!(walls["count"], 5, "the reply and the drawing agree: {walls}");
    let zones = tool(&mut s, "query_entities", json!({"layer": "RLV_ZONES"}));
    assert_eq!(zones["count"], 2, "{zones}");

    let b3d = tool(&mut s, "pointcloud_auto3d", json!({"index": 0}));
    assert_eq!(b3d["walls3d"].as_u64(), Some(5), "the same five walls, extruded: {b3d}");
    assert_eq!(b3d["slabs"].as_u64(), Some(2), "{b3d}");
    assert_eq!(b3d["notAvailable"], json!(["dimensions"]), "{b3d}");

    let cls = tool(&mut s, "pointcloud_classify", json!({}));
    assert!(cls["vectorized"]["fences"].as_u64().unwrap_or(0) >= 1, "{cls}");

    let topo = tool(&mut s, "topolt_survey", json!({"station": [0.0, 0.0, 100.0], "target": [10.0, 0.0, 101.0]}));
    assert_eq!(topo["dist2d"], 10.0, "{topo}");

    let terrain = tool(&mut s, "topocad_terrain", json!({"minorStep": 0.5, "majorEvery": 5}));
    assert!(terrain["segments"].as_u64().is_some(), "{terrain}");

    let pad = tool(&mut s, "cadgen_cadastral", json!({"parcelId": "IE 900100", "epsg": 31700}));
    assert_eq!(pad["parcelId"], "IE 900100", "{pad}");
}
