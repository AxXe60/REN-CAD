# Point clouds (LAS/LAZ) — RENCAD

> **Last reviewed:** 2026-10-21 · **Status:** M11 — the ground filter, the scan's coordinate system, the site frame · **Fork addition**

RENCAD is the working name of this fork of CADCraft (the crates and the binary keep their upstream
names so the weekly rebase stays a fast-forward). The fork adds point cloud support: M1 read a
LAS/LAZ file, kept the points inside a Z band and wrote them as ordinary DXF POINT entities; M2
kept the band in the session and drew it in the canvas; M3 made that overlay fast and controllable
(a decimation pyramid, one GPU draw call, colour modes and live decimation); M4 adds the **Point
Cloud** palette and a **3D view** with an orbit camera over the same pyramid; M5 makes it practical
on real scans: the finest level can be every point of the file, intensity is a colour spectrum, the
grid is a 3D cube so walls keep their height, and a crop box removes the ceiling or the walls.
M6 makes the 3D view a working surface: a long-lens perspective or true orthographic camera, zoom
anchored at the cursor and named views, plus **3D drafting** — polylines traced over the cloud that
snap to its points and export as DXF `LINE` entities. M7 makes that tracing behave like drafting:
the snap keeps only the **front layer** under the cursor (so it cannot snap through a wall) and can
average the scan noise away, the stroke can be held in a horizontal plane (**Z lock**) or on one
cardinal axis (**ortho**), every stroke carries a **layer**, a polyline can be **closed**, and a
**measure** tool reports the length, the run in plan and the rise of a segment. M9 makes the picture
readable without a depth buffer: a **front wall cut** drops the points between the camera and the
target, a **depth cue** fades the far ones into the background, and the colour ramps run over the
**2%–98% window** so a few outliers cannot flatten the contrast of the rest.

## Commands

All are plain `CommandSpec`s, so they work from the command line, the CLI, the JSON control channel
and MCP without extra wiring.

### `LASINFO {path}`

Reads the header only. Returns the point count, bounds, scale/offset, point format, whether the
file carries colour and the coordinate reference system it names (`crs` is `null` when the file
names none).

```bash
cadcraft-cli run --cmd 'lasinfo {"path":"D:/LIdar/Ap 1.las"}'
```

```json
{"points":3899546,"bounds":{"min":[-9.238,-9.742,-1.693],"max":[4.048,8.034,5.392]},
 "hasColor":true,"pointFormat":3,"scale":[0.0001,0.0001,0.0001],"crs":null}
```

### `LASEXPORT {path, out, z0?, z1?, decimate?, layer?}`

Streams the file, keeps `z0 <= z <= z1` and writes a DXF of POINT entities (R2000, ASCII) on
`layer` (default `PC-SLICE`). `decimate` is a grid cell in metres — 0.01–0.05 m keeps wall outlines
and drops scanner noise. Without `z0`/`z1` it uses the file bounds. When the file names a
coordinate reference system, its WKT is written next to the drawing as `<out>.prj`, so the slice
keeps its place on the globe.

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

- `decimate` — optional grid cell in metres for the finest level. Without it the finest level is
  the source points themselves: every point of the band (up to `maxPoints`). `0.02` is the drafting
  setting for tracing, `0.002` for measuring walls. The grid is a 3D cube, so a wall keeps its
  points up its height instead of collapsing to one point per column.
- `maxPoints` — memory cap on the finest level, default 6,000,000. If the band is bigger the grid
  is coarsened until it fits, so a 100-million-point file cannot exhaust RAM. `0` keeps every band
  point, up to a 20,000,000 safety ceiling. Files over 8,000,000 points are also thinned to a 5 mm
  cube while reading (before the cap applies), so the first read cannot hold 3 GB of points.
- `color` — `rgb` (default), `elevation`, `intensity`, `single:#rrggbb`. `intensity` is a spectrum
  across the band's returns (blue = darkest, red = brightest), not a flat grey.

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

With no `decimate` the same call reports every band point:

```json
{"name":"Ap 1","points":3899545,"levels":10,"cell":0,"inBand":3899545,
 "message":"Ap 1: 3899545 points (all band points) from 3899545 in the band (10 levels)"}
```

### `LASCOLOR {mode, index?}`

Recolours an open cloud without re-reading the file. `rgb` is the scanner's true colour,
`elevation` a blue→cyan→green→yellow→red ramp across the band's Z, `intensity` the same kind of
spectrum across the band's own return range (blue = the dimmest return, red = the brightest),
and `single:#rrggbb` (or just `#rrggbb`) one flat colour.

```json
{"index":0,"color":"intensity","message":"Ap 1: colour intensity"}
```

### `LASPECTRUM {spectrum?, index?}`

Chooses the colour scale the intensity (and the elevation) band is read on. Six scales, all drawn
in code — nothing imported, nothing licensed:

| Scale | What it is for |
|---|---|
| `grey` | the black-and-white of the printed report |
| `bluered` | the thermal scale of the screen (the default) |
| `viridis` | even steps of light: a weak difference is still a difference |
| `turbo` | the rainbow, ordered so it does not repeat itself |
| `magma` | the scale of a relief |
| `diverging` | the scale of the comparison |

```json
{"index":0,"spectrum":"viridis","message":"Ap 1: the intensity is read on the Viridis scale"}
```

Without a name the command walks to the next scale — the palette's row of buttons does the same.

### `LASDECIMATE {cell, index?}`

Re-reads the file at a new grid cell, keeping the band, the name, the colour mode and the crop —
no need to `LASCLOSE`/`LASOPEN`. `cell: 0` drops the grid and keeps every point of the band (the
palette's **All** button), so the finest level is the LAS itself.

```json
{"cell":0.01,"points":15488,"levels":1,
 "message":"Ap 1: 15488 points on a 0.01 m grid (1 levels)"}
```

```json
{"cell":0,"points":3899545,"levels":10,
 "message":"Ap 1: all 3899545 band points (10 levels)"}
```

### `LASCLIP {min, max, index?}` · `LASCLIP {reset: true, index?}`

Crops the cloud to a box in world metres without re-reading the file. Lower `max[2]` to remove the
ceiling, raise `min[2]` to remove the floor, and pull the X/Y bounds in to cut the four walls; the
points stay in memory, so the box can be moved or reset at any time.

```json
{"index":0,"clip":{"min":[-9.24,-9.75,-1.66],"max":[4.05,8.04,1.2]},
 "message":"Ap 1: cropped to the box"}
```

```json
{"index":0,"clip":null,"message":"Ap 1: crop off"}
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
- **Open** (the path field at the top, when no cloud is open) reads the file at full resolution —
  every point of the band — and the **Grid** slider thins it afterwards.
- **Fit** frames it in 2D, **3D** switches the canvas to the 3D view (**2D** switches back),
  **Close** drops the cloud and **Show** hides it without closing.
- **Colour**: RGB, Elevation, Intensity, or the colour picker for a flat colour (`single:#rrggbb`).
  Intensity shows the band's return range next to the point count.
- **Grid** and **Apply** re-read the file at a new decimation cell; **All** keeps every point of
  the LAS band (no grid). The count line says whether the finest level is the source points.
- **Remove walls / ceiling** enables a crop box: one min/max pair per axis. Lower the Z maximum to
  drop the ceiling, raise the Z minimum for the floor, pull X and Y in for the walls; untick it to
  restore every point (the file is not re-read).
- **Find ground** classifies the floor (see below) and **Ground only** draws just that.
- **Opacity** makes the cloud transparent; the 3D view thins the points (the x-ray), so the drawing
  under the cloud stays readable.
- The count line says whether the finest level is the source points, and the line under it names
  the coordinate reference system the file carries (`CRS: EPSG:32610 · WGS 84 / UTM zone 10N`), or
  says that the file names none. The next line is the **site origin**: the world point that sits at
  the drawing's (0, 0, 0).
- When the 3D view is on, the camera controls appear: **Persp/Ortho**, **Fit**, **Top/Front/Iso**,
  the **Lens** slider and the point size.
- Below them, the clarity controls: **Cut** (the front wall cut) and **Cue** (the depth cue).
- Above them, the drafting tools: **Draw 3D** and **Measure**, then **Finish**/**Close**/**Cancel**
  for the stroke being drawn, **Undo**/**Clear** for the committed ones, the snap mode
  (**Smooth**/**Exact**), the **Z lock** and **Ortho** checkboxes and the layer field.

## The 3D view

`ui.view3d {"on": true}` (or the palette's **3D** button) replaces the drawing canvas with a
perspective view of the visible clouds in the same rectangle; `{"on": false}` returns to 2D.

- **Drag** orbits, **Shift+drag**, **middle drag** or **right drag** pans, the **wheel** zooms
  towards the cursor and **double-click** fits the clouds. **Fit 3D** re-frames without touching the
  camera angles.
- The camera is an orbit around the cloud's centre with world Z up (the scanner's vertical). It has
  two projections: **Perspective** with a 35° vertical field of view by default (a normal lens, not
  the 50° wide angle that bent near walls) and an adjustable **Lens** slider from 15° to 90°, and
  **Ortho**, a parallel projection where walls stay parallel and equal world lengths stay equal on
  screen at any depth. Both frame the same world height at the target, so toggling keeps the view.
  **Top**, **Front** and **Iso** restore the standard angles. Camera angles and projection survive
  switching to 2D and back.
- Zooming is anchored at the cursor: the world point under the pointer stays under it while `dist`
  changes, instead of the view growing towards the middle of the screen. The anchor moves the orbit
  target, so the target is clamped to the visible clouds' box after every zoom and pan: a wheel over
  the empty sky (or a pan past the edge) otherwise walked the camera off the scan and left an empty
  viewport while the HUD still reported points. With no visible cloud there is no box and the camera
  moves freely.
- The same decimation pyramid is used: the level is picked for the on-screen density at the camera
  target, then the scene keeps only the points inside a ball around the target (three times the
  camera distance, which covers the screen corners at every orbit angle). The uploaded set is
  capped at 3 million points; because the ball shrinks as you zoom in, the full-resolution level
  fits exactly where its detail is visible. Orbiting changes neither the ball nor the level, so it
  still needs **no rebuild and no upload** — each frame only writes an 80-byte view-projection
  uniform; zooming and panning rebuild in coarse steps (a quarter of the ball, half an e-fold of
  its radius).
- `crates/ui-egui/src/gpu3d.rs` owns a second pipeline (target format and stencil attachment shared
  with the 2D canvas). Points are 6-vertex instanced screen-space squares because WGSL has no
  `gl_PointSize`; the shader converts sRGB to linear when the target is sRGB, as the 2D shader does.
- Without a wgpu render state (a wasm build without a GPU) the view projects a sampled 50k points
  on the CPU with egui shapes, so it still shows something.

### Comparing two clouds (C2C)

The measurement a survey office makes after a second visit: how far is the new scan from the
reference? `LASCMP {index?, reference?, cell?}` measures the distance from every point of the tested
cloud to the nearest point of the reference and reports the statistics of those distances — the
mean, the standard deviation, the **RMS** (the number a survey report quotes), the extremes and the
95th percentile — and draws the cloud on a blue-to-red scale.

Two rules of the practice (Zhang et al. 2015; the cloud-to-cloud comparison every survey tool
does): the **reference must be denser** than the tested cloud, and it must **cover** it. A point that
finds no neighbour is counted as missing and stays grey — the report says how many, instead of
inventing a distance.

```json
{"command":"lascmp","params":{"index":0,"reference":1}}
```

```json
{"message":"Ap 1 against Curte ulei: 3899545 points: mean 0.0121 m, σ 0.0090 m, RMS 0.0150 m, min 0.0000, max 0.2130, p95 0.0243 m"}
```

The search is a uniform grid over the reference — one flat array of buckets, the cell four times
the reference's point spacing — so a comparison of two four-million-point scans takes seconds, not
minutes: the measured run (two 3 899 545-point scans of the same file, read and compared) took
28.7 s, the comparison itself about 15 s of it.

### The viewport HUD and the Z band

The 3D view carries its own bar, over the canvas, below the app's menus: **2D / 3D**, the standard
views (**Top**, **Front**, **Right**, **Iso**), **Perspective / Orthographic**, **Fit** and the cloud's
**opacity**. Every button is a command or a view change — the bar is thin, it owns no state.

The palette's **Z Band** panel draws the scan's height profile (64 bins, computed when the file is
read) with the cut marked on it, and the band can be walked up and down with the jog buttons while
its thickness stays locked — the way a floor slab is found in a storey scan.

### Clarity without a depth buffer

The 3D pipeline has no depth attachment (the 2D canvas owns the pass), so the near and the far
geometry are mixed. Two things stand in for the depth test:

- **The front wall cut** (`Cut` in the palette, `{"cut": 0.3}` in the control channel): the points
  between the eye and that fraction of the distance to the target are dropped in the vertex shader
  (their vertex goes outside the clip volume, so nothing is drawn for them). The camera can look
  into a room without cropping the walls out of the cloud in `LASCLIP`, and the cut moves with the
  camera.
- **The depth cue** (`Cue` in the palette, `{"cue": 1}`): every point knows its depth along the
  view direction, so the ones behind the orbit target fade towards the background (up to 45%, with
  a quadratic ramp so only the far ones recede). The squares keep their size: the level's spacing
  still sets the side, and the cue works through colour alone. Shrinking the far squares as well took
  them down to a single pixel, which left one-pixel gaps between them wherever the level drew its
  points close together, so the depth is told by the fade and not by a smaller point.

Both live in the vertex shader of `crates/ui-egui/src/gpu3d.rs` and need no extra attachment: the
depth is `dot(point − eye, forward)`, which works in the orthographic projection too (where
`clip.w` is always 1). The uniform is a `repr(C)` struct of `f32` arrays, cast into the buffer with
`bytemuck`, so the 16-byte vertex layout and the uniform layout are both exactly what the shader
expects.

### Colour ramps that survive outliers

`Elevation` and `Intensity` map the value to the blue→red ramp. A single spike through the
ceiling or one hot return would stretch the range and flatten the contrast of the other 99% of the
points, so the ramp runs over the **2%–98% window** instead, computed at load time from a
histogram (65 536 bins for Z, 256 for the intensity — no sorting of four million points). The
values outside the window are clamped to its ends.

### Seeing through the cloud

A dense cloud hides the drawing under it, and without a depth buffer the points in front and
behind pile up: a wall seen edge-on becomes a solid slab of pixels, and the line drawn over it
disappears.

`LASOPACITY {opacity?, index?}` makes the cloud transparent (5%..=100%; without a value it
toggles between opaque and a third). Two things happen at once:

- the points are **thinned** by a deterministic mask on their index, so the wall keeps its shape
  but stops being a solid slab — the x-ray, and the reason the drawing under it stays readable;
- the colour and its alpha are scaled together, so the cloud fades into the background instead of
  turning into a flat grey.

The 2D overlay uses the same opacity (the render list's transparency), the 3D view the shader's
alpha. The mask lives in the vertex shader, so it costs nothing per frame.

```json
{"command":"lasopacity","params":{"opacity":0.35}}
```

```json
{"message":"Curte ulei: the cloud is drawn at 35%","index":0,"opacity":0.35}
```

### The ground under the cloud

A survey scan is mostly furniture, vegetation and noise; the floor is what a drawing is measured
from, and the LAS classification codes of the file are often missing or wrong. `LASGROUND` finds
the ground the way every GIS tool does, with the slope correction a bare earth surface needs:

1. the points are binned into 0.5 m cells and the lowest point of every cell seeds a surface;
2. a seed far above its neighbours (a table, a car) is dropped, so it cannot drag the surface up;
3. the surface of every cell is a **plane** fitted through the seeds of its 3 × 3 neighbourhood,
   so a sloped floor stays a sloped floor instead of a staircase;
4. a point on that plane is the ground (LAS class 2), a point under it is noise (class 18), and a
   point far above its neighbours is noise too.

Every level of the decimation pyramid is classified on its own, so the answer is as sharp as the
level it belongs to. **Ground only** in the palette then draws just the floor — the apartment's
floor plan, with the furniture gone — in both the 2D overlay and the 3D view.

The classes live beside the 16-byte vertex, not inside it: the GPU never needs them.

### Where the scan is

A LAS file may name its coordinate reference system — the EPSG code in the GeoTIFF keys, or the
full OGC WKT. RENCAD reads both, shows the system in the palette and in `LASINFO`, and writes the
WKT next to an exported drawing as a `.prj` sidecar (the convention every GIS tool reads), so an
exported slice keeps its place on the globe.

A registered Faro scan carries no CRS record at all (the georeference lives in the survey
project), so the project declares it: `LASCRS {epsg, name?, falseEasting?, falseNorthing?}`.
The offsets are the local grid's own: a survey office that writes the Stereo 70 northing without
its leading millions needs `falseNorthing`, or the export lands kilometres from the truth.

`LASGEO {lat, lon}` ties the project to a point on the map: the app projects that point into
the project's grid (Stereo 70, an UTM zone — the standard Transverse Mercator series on GRS 80)
and works out the local grid's offsets as *the file's coordinates minus the standard grid of that
point*. The offsets go into the WKT, and the drawing's (0, 0, 0) moves to the declared point.

```json
{"command":"lascrs","params":{"epsg":31700}}
{"command":"lasgeo","params":{"lat":44.1124385745297,"lon":24.974564167361805}}
```

```json
{"message":"the project is at 44.1124386, 24.9745642 — EPSG:31700 497964.36, 7884400.12; the local grid is E +39.40 N -7594110.45",
 "grid":[497964.36,7884400.12],"reference":[498003.76,290289.67,101.311]}
```

The fork does not reproject the points: the drawing is 2D in local units, and a scan is placed
where its file puts it. A file that names no system says so plainly instead of pretending.

### `LASORIGIN {x?, y?, z?, reset?}` · `LASCRS {epsg?, name?, reset?}`

`LASORIGIN` sets the site frame — the world point that sits at the drawing's (0, 0, 0). Without
arguments it is the middle of the first cloud with its lowest point at zero; with `x`, `y`, `z` it
is the point the survey office gives. `LASCRS` declares the coordinate system of the project for
the files that name none (a registered Faro scan has no CRS record at all).

```json
{"command":"lasorigin","params":{}}
```

```json
{"message":"the site origin is at 498003.760, 290188.238, 101.311: the drawing's (0, 0, 0) is that point in the world",
 "origin":[498003.76029999997,290188.2384,101.311]}
```

### `LASGROUND {cell?, tolerance?, bump?, sigma?, show?, index?}`

`cell` is the size of the cells the surface is built on (0.5 m), `tolerance` how far above the
surface a point may sit and still be the ground (0.15 m), `bump` how far above its neighbours a
seed may sit before it is dropped (0.3 m), and `sigma` how many standard deviations above the
local heights a point may sit before it is noise (3; 0 turns the cut off). `show` switches the
drawing between everything and the ground alone.

```json
{"command":"lasground","params":{"cell":0.5,"show":true}}
```

```json
{"message":"Ap 1: 737687 ground points, 43078 noise, 3118780 unclassified (showing the ground only)",
 "ground":737687,"noise":43078,"unclassified":3118780,"groundOnly":true}
```

### `LASCLASS {mode?, cell?, radius?, sample?, index?}`

Classifies the cloud the way a report does: the ground, the vegetation in three bands, the walls,
the noise. The heights are measured from the ground the filter found, so the ground filter runs
first if it has not run yet.

| Mode | What it reads |
|---|---|
| `auto` (the default) | the height bands and the shape |
| `height` | only the height above the ground |
| `shape` | only the shape of the surface: the walls and the flat surfaces |

The rules, all of them explainable in a report:

* under **20 cm** above the ground: the ground;
* **50 cm**, **2 m** and **5 m**: the three bands of the vegetation — low, medium, high;
* above **5 m**: a building (a yard, a courtyard, a facade);
* a surface that **stands up** (its normal is horizontal) above the vegetation: a facade, so a
  building;
* a point with **no neighbours** at all: noise.

The features are read on a sample (60 000 points by default, `sample` changes it) and given to the
whole level through the nearest neighbour, so a four-million-point scan classifies in seconds.

```json
{"command":"lasclass","params":{"mode":"auto"}}
```

Measured on the Ap 1 scan (3 899 545 points, 14.7 s including the reading of the file):

```json
{"message":"Ap 1 (auto): 1096838 ground, 261829 low vegetation, 1359446 medium vegetation, 137387 high vegetation, 1043592 building",
 "classes":{"ground":1096838,"low vegetation":261829,"medium vegetation":1359446,"high vegetation":137387,"building":1043592}}
```

What the rules cannot tell: a piece of furniture at a metre's height is called *medium vegetation*,
because height and shape alone do not separate a chair from a bush. The clustering below is the way
out — the user names the classes.

The class colours are the ones the LAS viewers use: the ground brown, the vegetation green, the
buildings red — `LASCOLOR {"mode":"class"}` or the palette's **Class colours** button.

### `LASCLUSTER {k?, cluster?, class?, index?}`

The classification with learning: the points are described by four things — the height above the
ground, the intensity of the return, the roughness of the surface, how much it stands up — and the
ones that look alike are **grouped** together. The groups come back with a class proposed for each
one; the user names them, and the classes are given again.

The grouping is the k-means of the features with a fixed seed, so the same scan always gives the
same groups and a report can be repeated. Nothing is downloaded, no model is trained, no weight
file is shipped — it is a few hundred lines of arithmetic.

```json
{"command":"lascluster","params":{"k":5}}
```

Measured on the Ap 1 scan (3 899 545 points, 15.7 s including the reading of the file), the groups
and the class proposed for each one:

```json
{"message":"Ap 1: 5 groups, named by the rules — rename one with `cluster` and `class`",
 "groups":[
  {"index":0,"count":11086,"class":"ground","centre":{"height":-0.32,"intensity":0.22,"roughness":0.021,"verticality":0.03}},
  {"index":1,"count":14878,"class":"building","centre":{"height":0.85,"intensity":0.19,"roughness":0.055,"verticality":0.83}},
  {"index":2,"count":14644,"class":"building","centre":{"height":0.96,"intensity":0.70,"roughness":0.017,"verticality":0.96}},
  {"index":3,"count":11079,"class":"high vegetation","centre":{"height":2.05,"intensity":0.67,"roughness":0.062,"verticality":0.31}},
  {"index":4,"count":9244,"class":"building","centre":{"height":2.28,"intensity":0.56,"roughness":0.009,"verticality":0.02}}]}
```

The reading of an apartment scan: the floor, two walls (one of them much brighter — a window or a
lamp), the clutter and the ceiling. The rules propose those classes; the user is the one who
decides, and one click renames a group.

The user in the loop:

```json
{"command":"lascluster","params":{"cluster":1,"class":"building"}}
```

```json
{"message":"Ap 1: group 1 is now building","cluster":1,"class":"building"}
```

The palette's **AI** button runs the grouping; every group gets a row with a list of the classes,
and choosing one names the group.

### The cloud belongs to the drawing, not over it

A cloud opened with `LASOPEN` is an **overlay**: it is looked at, and it is not written. A cloud
**attached** to the drawing is *referred to* by the drawing — and the file stays small.

```json
{"command":"lasopen","params":{"path":"D:/scan/Ap 1.las","decimate":0.05,"attach":true}}
```

```json
{"command":"lasattach","params":{"index":0,"attach":true}}
```

The palette's **Attach to the drawing** box does the same, and it can be turned off again.

**What the file carries is the reference, not the points.** A scan of a building has millions of
points; a file that holds them all is a file nobody can send. So the drawing says:

* the **file the scan is in** (`D:/scan/Ap 1.las`), the **band** of Z it holds, the **grid cell** it
  is read at, the **colour mode** and the **colour scale**;
* the **coordinate system** the file declares (the EPSG code);
* **where the scan sits in the drawing**: the world point that is the drawing's (0, 0, 0) — the
  same tie `LASORIGIN` makes, saved with the drawing.

That is written in the `XRECORD` `CADCRAFT_CLOUDS` of the named object dictionary. An `XRECORD`
and not a header variable, because the DWG keeps the records and drops the variables it does not
know (the first try lost the cloud in the DWG, and the test says so now).

**When the file is opened, the scan is loaded from the disk**, at the position the file says: the
same band, the same grid, the same colour mode, the same place in the drawing. If the scan is not
on that computer, the drawing opens without it and says which file it was looking for — a drawing
that travels to a colleague carries the reference, and the colleague points it at the scan.

Measured: the Ap 1 scan decimated to a 5 cm grid (184 499 points) is a **DWG of 22 kB** — the
drawing and the reference, nothing else — and opening it loads the scan from
`D:/…/Ap 1_….las` with its position:

```json
{"clouds":[{"name":"Ap 1_2026-10-06-13-44-44.section_in1","path":"D:/…/Ap 1_….las",
            "points":184499,"levels":4,"bounds":{"min":[-9.2382,-9.7418,-1.6544],"max":[4.0479,8.0337,5.3914]}}],
 "count":1}
```

And when the scan is not there, the drawing still opens:

```text
D:/tmp/scan_temp.las: the scan could not be loaded: The system cannot find the file specified.
```

> The reference does not carry the points, so the DWG holds no cloud points at all: any program
> that opens the file sees the drawing, and RENCAD sees the drawing plus the scan it points at.

### The site frame
A registered survey sits hundreds of kilometres from the drawing's origin — the factory at
E 498 162, N 290 290 is 500 km from (0, 0) — so a scan in a projected system and a drawing in
local metres have to meet somewhere. The **site origin** is that meeting point: the world point
that sits at the drawing's (0, 0, 0). The clouds and the drawing share the local frame, and the
origin is the tie between them (the same idea as AutoCAD's georeferencing point).

`LASORIGIN` sets it: without arguments the middle of the first cloud, with its lowest point at
zero; with `x`, `y`, `z` the point is placed where the survey office says it is. The 2D overlay,
the 3D view, `LASFRAME` and the ground filter all follow it.

`LASCRS {epsg}` declares the system's code for the files that name none — a registered Faro scan
carries no CRS record at all, the georeference lives in the survey project. The code is shown in
the palette, and `LASEXPORT` writes the matching WKT next to the exported drawing as a `.prj`.

### Reading a 50-million-point survey

A registered scan is 1.8–3.3 GB and 53–98 million points. The read never holds them: the points
are binned into cells while the file streams, and when the kept points outgrow the cap the cell
doubles and the points kept so far are re-binned. The memory stays flat, and the levels built
from the result are the ones the whole file would have given.

The cap (`maxPoints`, 6 million by default) decides how much detail survives: the finest level of
a 53-million-point survey holds the first `maxPoints` points of the read, spread evenly, not the
walls in full detail. A survey that wants the structures sharp needs the streaming LOD of the
next milestone.

### 3D drafting

The palette's **Draw 3D** turns the 3D view into a drafting surface (the `3DLINE` command is the
same path for scripts):

- A click places a vertex. It **snaps to the cloud** within 14 screen points (the uploaded scene is
  searched on a ~250k-point sample, which is a millisecond per frame); with no point nearby it lands
  on the view plane through the orbit target, so free vertices are always at the depth you are
  looking at.
- The next vertex is previewed as a rubber band; a green ring and dot mark a snapped point, an
  orange ring a free one.
- **Double-click** or **right-click** ends the stroke, **Esc** or **Cancel** drops it, **C** closes
  the polyline (the last vertex joins the first). Ending a stroke calls `3DLINE`, so it lands in
  `Session::sketch` exactly as it would from a script.
- Committed strokes are painted over the points in orange (the clouds' pipeline has no depth
  buffer, so the sketch is deliberately always visible). `3DUNDO` drops the last stroke, `3DCLEAR`
  all of them, and `3DEXPORT {path}` writes them as DXF `LINE` entities with their Z (a lone vertex
  becomes a `POINT`), each on its own layer, ready to open as an ordinary drawing file.

### Smart snapping

The 3D pipeline has no depth buffer, so the snap cannot ask the GPU which point is in front. It
ranks the candidates itself:

1. Every visible point that projects within 14 px of the cursor is a candidate, with its distance
   to the camera along the view direction.
2. The **front layer** is the shallowest candidate plus everything within 0.15 m behind it. A point
   on the far wall, seen through a gap in the near one, is deeper and stays out of the layer.
3. The winner is the candidate of the front layer closest to the cursor.
4. `Smooth` (the default) then averages the cloud points within a 3 cm sphere around the winner:
   the 2-5 mm scan noise of a wall cancels out. `Exact` keeps the cloud point itself.

`3DSNAP {mode}` switches the mode; the palette shows it as **Smooth** / **Exact**. The rules live
in `crates/pointcloud/src/snap.rs` (`front_layer`, `nearest`, `smooth_centroid`) and are unit
tested there.

### Drawing constraints

- **Z lock** (`Z` in the view, or a checkbox): a new vertex takes the X and Y of the snap but the Z
  of the stroke's first vertex, so the whole stroke stays in one horizontal plane.
- **Axis ortho** (`Shift` for one move, or a checkbox): the new vertex leaves the last one along
  the cardinal axis the movement is closest to, so a segment is never accidentally diagonal.

Both are pure functions in `crates/pointcloud/src/snap.rs` (`z_locked`, `axis_locked`).

### Measuring 3D

`3DMEASURE {p1, p2}` returns the numbers of a segment: `dist_3d`, `dist_2d_xy`, `dx`, `dy`, `dz`,
`slope_deg` and `slope_pct` (null for a vertical segment, which has no horizontal run to compare).
The palette's **Measure** mode measures with two clicks that snap to the cloud; the segment stays
on screen in blue with its numbers, and the rubber band shows the length, the run and the rise live
while a stroke is being drawn. The arithmetic lives in `crates/pointcloud/src/measure.rs`, so the
command and the screen always agree.

### `3DLINE`, `3DCLEAR`, `3DUNDO`, `3DLIST`, `3DEXPORT`, `3DSNAP`, `3DMEASURE`

`3DLINE {points: [[x,y,z], …], layer?, closed?}` appends one polyline (at most 100,000 vertices;
non-finite coordinates are refused). The layer defaults to `3D_DRAFT`; `closed: true` joins the
last vertex back to the first. `3DLIST` reports each stroke's vertex count, endpoints, layer and
closed flag. `3DEXPORT {path, layer?}` writes the strokes, each on its own layer (an explicit
`layer` puts them all on one instead), and refuses an empty sketch instead of writing an empty
file.

```json
{"command":"3dline","params":{"points":[[0,0,1.2],[4,0,1.2],[4,3,1.2]],"layer":"WALLS"}}
```

```json
{"message":"3D stroke: 3 vertices on layer WALLS (1 strokes, 2 segments)","vertices":3,"strokes":1,"segments":2}
```

```json
{"command":"3dmeasure","params":{"p1":[0,0,0],"p2":[3,4,12]}}
```

```json
{"dist_3d":13.0,"dist_2d_xy":5.0,"dx":3.0,"dy":4.0,"dz":12.0,"slope_deg":67.38,"slope_pct":240.0}
```

The strokes live beside the clouds in `Session::sketch`, not in the drawing model: upstream entities
are 2D (Z=0), so 3D geometry that must not disturb snaps, grips, undo or save stays a fork field
until the 3D kernel can carry it.

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
| 3D scene at fit, whole file open | 214,871 visible points → **3.4 ms** to build and upload; orbiting then rebuilds nothing |
| 3D zoom steps | 506k → 8 ms, 1.13M → 18 ms, 2.45M → 40 ms; each upload holds only the ball around the target |
| Recolour (`LASCOLOR`) | instant; the mesh rebuilds on the next frame |

The LOD keeps the drawn count proportional to the screen, not to the file: ~550k is what a
1600×1000 canvas can show at ~2 px per point, and panning inside the built area only moves the
buffer.

### The commands, measured (the whole 3 899 545-point scan, release build)

| Command | Before | Now |
|---|---|---|
| `LASOPEN` (read and pyramid) | 6.1 s | **3.0 s** |
| `LASGROUND` (the ground filter) | +0.4 s | +0.0 s |
| `LASCLASS` (the classification) | +7.5 s | +5.4 s |
| `LASCLUSTER` (the grouping) | +6.8 s | +4.8 s |
| `LASCMP` (two such clouds) | +10.7 s | +6.0 s |

What made the difference:

* the pyramid is built **level by level** (each level from the one below it), not by walking the
  whole band once per level: the cells double, so every cube of a level sits inside one cube of
  the level above, and the decimation is then exactly the one a fresh pass would give;
* the read uses the reusable buffer of the `las` crate and takes the coordinates from the column
  accessors, instead of decoding every record into an owned struct and copying it again;
* the ground filter works on one flat cell array over the band's own box, with no allocation per
  cell (and keeps the old path for a band spread over kilometres, so the memory stays bounded);
* the classification computes the heights above the ground **for the sample it uses**, not for the
  whole level;
* the comparison builds the reference's grid **once** for all the levels of the tested cloud, and
  the grid itself is an open-addressed table instead of a hashed map;
* the statistics of a comparison find their percentile from a histogram, without copying and
  sorting three million distances.

What is still slow, and why: the classification gives every point of every level the class of its
nearest sampled point, and that is a nearest-neighbour search over the whole pyramid. The next
step is for the coarse levels to inherit from the fine one.

## Design

- `crates/pointcloud` (L3) reads, slices and builds the pyramid; it knows nothing about egui or the
  drawing model.
- Points stream in batches of one million, so a 12-million-point scan never sits in memory whole.
- `Cloud::from_slice` keeps a chain of grid-decimated levels (the cells double, so every cube of a
  level sits inside one cube of the level above: the level is built from the level below it, and
  the decimation is then exactly the one a fresh pass over the band would give) until the coarsest
  fits 20k points, capped at 16 levels. Each vertex is 16 bytes: `f32` x/y/z relative to the cloud origin
  plus RGB and intensity bytes. `level_for(px_per_m)` picks the level whose cell is nearest 2 screen
  pixels.
- The canvas clips the chosen level to the view plus a quarter screen, builds a `DisplayList` with
  one `Kind::Cloud` primitive, and hands it to the GPU instanced-quad pipeline on `LAYER_CLOUD`
  (5): one instance per point with its own colour, one draw call. Each cloud asks for a square as
  wide as the spacing of its level on screen (through the primitive's `lw`), so the points tile the
  cloud instead of leaving it a dotted haze; `gpu::CLOUD_SIZE` is the fallback.
- **The level does not flicker at a zoom boundary.** `level_for_sticky` keeps the level drawn last
  while its spacing on screen is between 1.25 and 3.2 pixels, so a zoom that hovers where the
  nearest-level choice sits between two levels keeps the density it had instead of flipping it from
  one frame to the next. The 3D view remembers the level it really drew, the budget coarsening
  included, so a count hovering at the scene budget is steady too.
- **The squares sit on the pixel grid.** A square that is a fraction of a pixel, at a fractional
  position, covers a different set of pixels for every sub-pixel of view motion: that is what made
  the cloud crawl while panning and orbiting. Both renderers now round the square to a whole number
  of device pixels and put its centre in the middle of a pixel (the 2D quad shader through the
  `c.z` flag only the cloud layer sets, the 3D shader in its vertex stage), so a point covers the
  same pixels until it really moves by one. The 3D square also had its half-extent divided by the
  viewport once instead of twice, which drew every point at half the requested size: the points are
  now the width they ask for, which is what makes them read as clear.
- The mesh is rebuilt only when the level, the colour mode or the ground covered changes
  (quantised view centre and zoom); plain panning just moves it. The overlay list is kept with
  its key, so a frame that changes nothing walks no points at all.
- The 3D scene counts its visible points only when the scene changes, and reserves what it really
  holds, not the whole level.
- Clouds live in `Session::clouds`, never as drawing entities: snaps, picking, undo and save see
  only what the user drew. Plots, PDF/SVG export and the CPU raster path have `Kind::Cloud` arms so
  a list containing one still renders (plots never do).
- The CPU path (no GPU target) draws the same list as egui rectangles — slower, but clouds stay
  visible.

## Limits (M11)

- LAZ is read, not written; no COPC streaming yet — a file is opened in one pass.
- No classification filter: the ground is found by the fork's own filter, but the file's own
  class codes are not read back into the drawing.
- No reprojection: a scan is placed where its file puts it, in local units.
- A huge survey thins evenly: the finest level of a 50-million-point scan holds the first
  `maxPoints` points spread evenly, so the walls lose their detail before the open yard does
  (the streaming LOD of the next milestone fixes it).
- No ortho PNG/GeoTIFF export, no intensity histogram.
- The finest level is capped by `maxPoints`; above it the grid is coarsened silently (the returned
  `points` says so). Raise `maxPoints` for a dense survey workstation.
- One mesh rebuild per zoom step costs ~20 ms at half a million points; panning within the built
  area is free. The 3D view uploads at most 3 million points: the visible ball is counted first and
  the level is coarsened until it fits, so the cap applies to the points on screen, not the cloud.
- The 3D view draws clouds and 3D strokes only; the 2D drawing and the ViewCube are not part of it.
  It has no depth buffer, so points and strokes do not occlude each other (fine for translucent
  scans, wrong for solid surfaces later). The snap compensates by ranking the candidates by depth
  and keeping only the front layer, and the front wall cut and the depth cue make the picture
  readable — but both are approximations of a real depth test, not a substitute for one.
- The `Smooth` snap averages a 3 cm sphere, so details smaller than that are flattened; use `Exact`
  when the surface detail matters.
- The 2%–98% window clips the extreme values: a point far above the others takes the top colour
  instead of its own. That is the price of keeping the contrast of the rest.
- The ground filter needs a floor it can see: a scan of nothing but furniture has no ground to
  find, and a mezzanine is classified as ground.
- 3D strokes live in the session: they survive closing the palette and switching to 2D, but not a
  restart. `3DEXPORT` is the way to keep them; the wrap-up into drawing entities waits for the 3D
  kernel.

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
