# Point clouds (LAS/LAZ) — RENCAD

> **Last reviewed:** 2026-10-21 · **Status:** M4 — palette and 3D view · **Fork addition**

RENCAD is the working name of this fork of CADCraft (the crates and the binary keep their upstream
names so the weekly rebase stays a fast-forward). The fork adds point cloud support: M1 read a
LAS/LAZ file, kept the points inside a Z band and wrote them as ordinary DXF POINT entities; M2
kept the band in the session and drew it in the canvas; M3 made that overlay fast and controllable
(a decimation pyramid, one GPU draw call, colour modes and live decimation); M4 adds the **Point
Cloud** palette and a **3D view** with an orbit camera over the same pyramid.

## Commands

All are plain `CommandSpec`s, so they work from the command line, the CLI, the JSON control channel
and MCP without extra wiring.

### `LASINFO {path}`

Reads the header only. Returns the point count, bounds, scale/offset, point format and whether the
file carries colour.

```bash
cadcraft-cli run --cmd 'lasinfo {"path":"D:/LIdar/Ap 1.las"}'
```

```json
{"points":3899546,"bounds":{"min":[-9.238,-9.742,-1.693],"max":[4.048,8.034,5.392]},
 "hasColor":true,"pointFormat":3,"scale":[0.0001,0.0001,0.0001]}
```

### `LASEXPORT {path, out, z0?, z1?, decimate?, layer?}`

Streams the file, keeps `z0 <= z <= z1` and writes a DXF of POINT entities (R2000, ASCII) on
`layer` (default `PC-SLICE`). `decimate` is a grid cell in metres — 0.01–0.05 m keeps wall outlines
and drops scanner noise. Without `z0`/`z1` it uses the file bounds.

```bash
cadcraft-cli run --cmd 'lasexport {"path":"D:/LIdar/Ap 1.las","out":"out/ap1_slice.dxf",
  "z0":1.535,"z1":1.695,"decimate":0.02}'
```

```json
{"points":8712,"inBand":153034,"filePoints":3899546,"layer":"PC-SLICE",
 "message":"8712 of 153034 points in the band written to out/ap1_slice.dxf"}
```

The band `1.535–1.695` m is apartment 1's floor (−0.505 m) plus 2.04–2.20 m: above the doors
(2.06 m) and below the wall cabinets (2.24 m), the same rule the scan-to-BIM work uses to separate
walls from furniture.

### `LASOPEN {path, z0?, z1?, decimate?, maxPoints?, color?, name?}`

Cuts the band, builds the display pyramid and keeps it in the session, drawn in the canvas under
the drawing — trace over it and save the trace as one DXF.

- `decimate` — grid cell in metres for the finest level, default 0.02 (2 cm). Pass `0.002` for a
  scan whose walls you want to measure.
- `maxPoints` — memory cap on the finest level, default 6,000,000. If the band is bigger the grid
  is coarsened until it fits, so a 100-million-point file cannot exhaust RAM.
- `color` — `rgb` (default), `elevation`, `intensity`, `single:#rrggbb`.

The first cloud in an empty drawing is framed automatically.

`OPEN` on a `.las` or `.laz` path runs `LASOPEN` instead of failing with "unsupported file type",
so **File ▸ Open** (the *Point clouds (LAS, LAZ)* filter), dropping the file on the window and the
palette's Open all keep the current drawing and add the cloud. Scripts, the CLI and MCP reach the
same command with `open {"path":"scan.las"}`; a path with spaces goes through the palette field,
the dialog or drag-and-drop, because the command line splits its input on spaces.

```bash
cadcraft-cli run --cmd 'lasopen {"path":"D:/LIdar/Ap 1.las","z0":1.535,"z1":1.695,
  "decimate":0.005,"color":"elevation"}'
```

```json
{"name":"Ap 1","points":23552,"levels":2,"inBand":153034,"filePoints":3899546,
 "color":"elevation","open":1}
```

### `LASCOLOR {mode, index?}`

Recolours an open cloud without re-reading the file. `rgb` is the scanner's true colour,
`elevation` a blue→cyan→green→yellow→red ramp across the band's Z, `intensity` a greyscale
normalised to the band's own return range (dim indoor returns would otherwise be black), and
`single:#rrggbb` (or just `#rrggbb`) one flat colour.

```json
{"index":0,"color":"intensity","message":"Ap 1: colour intensity"}
```

### `LASDECIMATE {cell, index?}`

Re-reads the file at a new grid cell, keeping the band, the name and the colour mode — no need to
`LASCLOSE`/`LASOPEN`.

```json
{"cell":0.01,"points":15488,"levels":1,
 "message":"Ap 1: 15488 points on a 0.01 m grid (1 levels)"}
```

### `LASCLOSE {index?, all?}` · `LASCLOUDS {}`

`LASCLOSE` drops the last cloud, the one at `index`, or all of them; `LASCLOUDS` lists what is open
with point counts, levels, colour mode and bounds.

### `LASVISIBLE {visible?, index?}`

Shows or hides a cloud without closing it (`visible` defaults to a toggle). The palette's **Show**
checkbox and the 2D and 3D views all read the same flag.

```json
{"index":0,"visible":false,"name":"Ap 1"}
```

### `LASFRAME {index?}`

Frames a cloud's XY bounds in the drawing view — the palette's **Fit** button, and the same extent
the first cloud gets when it opens.

```json
{"index":0,"name":"Ap 1","framed":true,"center":[-2.595,-0.854],"height":21.15}
```

## The Point Cloud palette

The fork adds a **Point Cloud** panel to the right dock (between Layers and Properties, opened by
default; `Window ▸ Point Cloud` reopens it, and closing it is remembered). It is deliberately thin:
every control runs the command above through `app.run`, so the palette, the command line and a
script cannot drift apart.

- The cloud list shows each cloud's points and level count; click a row to choose which cloud the
  controls act on.
- **Fit** frames it in 2D, **3D** switches the canvas to the 3D view (**2D** switches back),
  **Close** drops the cloud and **Show** hides it without closing.
- **Colour**: RGB, Elevation, Intensity, or the colour picker for a flat colour (`single:#rrggbb`).
- **Grid** and **Apply** re-read the file at a new decimation cell.
- When the 3D view is on, a point-size slider and **Fit 3D** appear, plus the number of points the
  3D scene drew.

## The 3D view

`ui.view3d {"on": true}` (or the palette's **3D** button) replaces the drawing canvas with a
perspective view of the visible clouds in the same rectangle; `{"on": false}` returns to 2D.

- **Drag** orbits, **Shift+drag** or **middle drag** pans, the **wheel** dollies and
  **double-click** fits the clouds. **Fit 3D** re-frames without touching the camera angles.
- The camera is an orbit around the cloud's centre with world Z up (the scanner's vertical), 50°
  vertical field of view. Angles survive switching to 2D and back.
- The same decimation pyramid is used: the level is picked for the on-screen density at the camera
  target, coarsened further if it would exceed 1.5 million points. Unlike 2D there is no view
  filter, so orbiting and panning need **no rebuild and no upload** — each frame only writes an
  80-byte view-projection uniform.
- `crates/ui-egui/src/gpu3d.rs` owns a second pipeline (target format and stencil attachment shared
  with the 2D canvas). Points are 6-vertex instanced screen-space squares because WGSL has no
  `gl_PointSize`; the shader converts sRGB to linear when the target is sRGB, as the 2D shader does.
- Without a wgpu render state (a wasm build without a GPU) the view projects a sampled 50k points
  on the CPU with egui shapes, so it still shows something.

Drawing in 3D is the next step: the camera and the scene are already independent of the 2D drawing,
and the 2D entities are all at Z=0, so the overlay and then 3D entity creation can reuse the same
view-projection.

## Measured on real data (Ap 1, Bucharest)

| Step | Result |
|---|---|
| File | 3,899,546 points, point format 3, RGB, 0.1 mm scale |
| Band 2.04–2.20 m | **153,034 points** — identical to the PDAL reference (`filters.range`) |
| Decimated at 2 cm | 8,712 points, 799 KB DXF |
| Slice + write DXF | 0.45 s |
| `LASOPEN` whole file at 2 mm | **1,394,540 points, 9 levels** (~38 MB of 16-byte vertices) |
| Mesh at default zoom | 44,259 points → **2.1 ms** |
| Mesh zoomed to 6 m | 551,870 points → **22 ms**, one draw call |
| 3D scene at fit (`ui.view3d`) | 46,319 points → **0.8 ms** to build and upload; orbiting then rebuilds nothing |
| Recolour (`LASCOLOR`) | instant; the mesh rebuilds on the next frame |

The LOD keeps the drawn count proportional to the screen, not to the file: ~550k is what a
1600×1000 canvas can show at ~2 px per point, and panning inside the built area only moves the
buffer.

## Design

- `crates/pointcloud` (L3) reads, slices and builds the pyramid; it knows nothing about egui or the
  drawing model.
- Points stream in batches of one million, so a 12-million-point scan never sits in memory whole.
- `Cloud::from_slice` keeps a chain of grid-decimated levels (cells ×1.6) until the coarsest fits
  20k points, capped at 16 levels. Each vertex is 16 bytes: `f32` x/y/z relative to the cloud origin
  plus RGB and intensity bytes. `level_for(px_per_m)` picks the level whose cell is nearest 2 screen
  pixels.
- The canvas clips the chosen level to the view plus half a screen, builds a `DisplayList` with one
  `Kind::Cloud` primitive, and hands it to the GPU instanced-quad pipeline on `LAYER_CLOUD` (5):
  one instance per point with its own colour, one draw call. `gpu::CLOUD_SIZE` is 2 px, matching
  the LOD spacing.
- The mesh is rebuilt only when the level, the colour mode or the ground covered changes
  (quantised view centre and zoom); plain panning just moves it.
- Clouds live in `Session::clouds`, never as drawing entities: snaps, picking, undo and save see
  only what the user drew. Plots, PDF/SVG export and the CPU raster path have `Kind::Cloud` arms so
  a list containing one still renders (plots never do).
- The CPU path (no GPU target) draws the same list as egui rectangles — slower, but clouds stay
  visible.

## Limits (M4)

- LAZ is read, not written; no COPC streaming yet — a file is opened in one pass.
- No classification filter, no ortho PNG/GeoTIFF export, no intensity histogram.
- The finest level is capped by `maxPoints`; above it the grid is coarsened silently (the returned
  `points` says so). Raise `maxPoints` for a dense survey workstation.
- One mesh rebuild per zoom step costs ~20 ms at half a million points; panning within the built
  area is free. The 3D view caps its scene at 1.5 million points per frame.
- The 3D view draws clouds only: the 2D drawing, the ViewCube and 3D drawing are not part of it yet.
  It also has no depth buffer, so points do not occlude each other (fine for translucent scans,
  wrong for solid surfaces later).

## Building this fork (Windows)

A full workspace build needs ~10 GB. If `C:` is short on space, point the target directory at
another drive first:

```powershell
. .\scripts\env.ps1      # sets PATH and CARGO_TARGET_DIR=D:\cargo-target\LASDraft
cargo xtask ci
```

`cargo xtask ci` runs fmt, clippy (`-D warnings`), the whole test suite, the asset attribution
check, the layering check and the wasm build. Do not push when it fails.

## Licence

`las` (MIT) and `laz` (Apache-2.0) are ordinary Cargo dependencies; nothing is vendored.
