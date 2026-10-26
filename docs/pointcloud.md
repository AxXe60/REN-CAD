# Point clouds (LAS/LAZ) — RENCAD

> **Last reviewed:** 2026-10-25 · **Status:** M12 — the ground filter, the scan's coordinate system, the site frame, the colouring · **Fork addition**

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
readable while the view still had no depth buffer: a **front wall cut** drops the points between the
camera and the target, a **depth cue** fades the far ones into the background, and the colour ramps
run over the **2%–98% window** so a few outliers cannot flatten the contrast of the rest. M12 makes
the colouring and the depth honest: every point carries the colour of its **own** record however the
band or the grid thinned the file, the 3D points write the window's depth buffer so the wall nearest
the eye is the wall that is seen, and the picture the user set up — the two windows the ramps read
through, the two multipliers that lift a dark photo, the opacity — survives a re-read and repaints
the moment it moves.

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

A photo is not a ramp, and a scanner's own colours arrive dark: `exposure` (0.25..=4) multiplies
the RGB and `contrast` (0.25..=4) stretches it about mid grey. `auto: true` takes both from the
finest level: every point's luminance is put on a sorted list and the 2%–98% span of that list is
stretched towards four fifths of the scale (the contrast), and the mean is then lifted to 0.55 on
top of that stretch (the exposure). Both answers stay inside the same bounds as the sliders.

Two things the automatic answer does not do. A file whose colour has no spread — every point the
same value, which is what a scan without an RGB column reads as — gets `1.0, 1.0`: stretching a
flat field past its clamp paints the whole cloud white. And no multiplier gives back colour the
file never recorded; `auto` only re-scales what is already there.

```json
{"index":0,"auto":true,"color":"rgb","exposure":1.1153893,"contrast":0.8812240,
 "message":"Ap 6: colour rgb"}
```

The answer above is `Auto RGB` on the 41.7-million-point scan measured below: its own luminance band
is already wide, so the automatic contrast is *below* 1 and the picture barely moves. Auto is an
answer from the statistics, not a promise of a better picture.

The multipliers are applied where the point's colour is built, so they cost no re-read and no second
buffer. The file's own widths are settled once for the whole cloud, at load: if any channel of any
point exceeds 255 the file is read as 16-bit and every channel is divided by 257, because a cloud
drawn from two different widths at once would show half of it black.

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

### `LASRANGE {target, min, max, index?}` · `LASRANGE {target, reset: true, index?}`

Names the window the elevation or the intensity ramp reads, instead of the band's own 2%–98% slice.
The points keep their numbers; only the colours move, so the same wall can be read as a storey
(`0.90..3.75 m`) or as the ceiling detail (`6.19..8.12 m`) without a second pass over the file.
`target` takes `elevation` (`height`, `z`), or `intensity` (`return`, `i`). The intensity endpoints
are on the 0..255 scale the ramp is drawn on, whatever the file's own width; elevation is in metres.

```json
{"index":0,"target":"elevation","min":6.1936,"max":8.1222,
 "message":"Ap 6: the elevation ramp reads 6.19 m to 8.12 m","window":[6.1936,8.1222]}
```

```json
{"index":0,"target":"intensity","min":60,"max":180,
 "message":"Ap 6: the intensity ramp reads 60 to 180","window":[60,180]}
```

`reset` hands the window back to the band and the answer says what the band's own window is;
`window` is `null` then. An endpoint that is not a number, out of 0..255 for an intensity, or a
`max` below the `min` is refused before anything is touched, and the error says
`` `max` must not be below `min` ``. Two elevation endpoints more than 10⁹ m from the origin are
refused too: they are finite numbers, but a span that wide overflows to infinity and every point
reads the ramp's first colour. An equal pair is not refused — a window of no span simply draws the
ramp's middle colour.

The palette draws the two sliders under the ramp (Height window / Intensity window) with an **Auto**
button beside them, and the row of colour chips under the sliders — the legend — re-reads its ends
from the same window and takes every chip from the same `Spectrum::apply` the points are coloured
with, so a legend can never promise a colour the drawing does not produce.

### `LASDECIMATE {cell, index?}`

Re-reads the file at a new grid cell. What comes back is the file's points; the cloud's life in the
drawing is carried over — the band, the name, the colour mode and scale, the crop, the place and
the tie to the document, the two colour windows, the two RGB multipliers, the opacity and whether
the cloud is shown. What a re-read does lose is anything measured on the points it replaced: the
ground classification `LASGROUND` wrote, and the per-point deviations the `Deviation` colour reads
— after a grid change that mode is the legend's "no data" grey until `LASCMP` runs again. No
need to `LASCLOSE`/`LASOPEN`. `cell: 0` drops the grid and keeps every point of the band (the
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

### `AUTORLV {index?, params?}` · `AUTO3D {index?, params?}`

The scan-to-BIM model of the cloud: floor and ceiling slabs, the storeys between them, the walls and
the closed rooms. `AUTORLV` is the 2D releveu (wall axes on `RLV_WALLS`, room rings on `RLV_ZONES`,
slab outlines on `RLV_SLABS`, the room schedule on `RLV_SCHEDULE`); `AUTO3D` extrudes the same walls
and re-draws the same slabs as 3D faces (`RLV_3D_WALLS`, `RLV_3D_SLABS`). Both refuse when no cloud
is open — they model the scan on screen, never a made-up apartment. The `params` object is the
pipeline's knobs; see [Auto-RLV and Auto-3D](#auto-rlv-and-auto-3d-scan-to-bim).

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
- Under **RGB**, the two sliders that make a dark scan readable — **RGB exposure** and **RGB
  contrast** — and **Auto RGB**, which takes both from the finest level's sorted luminance: the
  2%–98% span sets the contrast, the mean sets the exposure. A cloud with no colour spread gets
  neutral values rather than a white field.
- Under **Elevation** or **Intensity**, the **Height window** / **Intensity window** pair of sliders
  and **Auto** (`LASRANGE`): the ends the ramp is read on. Under them the **legend** — one chip per
  colour of the ramp, the two ends named — or one chip per class the scan carries in class colours.
  The comparison's key adds a last chip for the grey of a point the reference has nothing for,
  because at the edge of a wall that grey is most of the picture. Every chip is the colour the
  points get, from the same function the drawing uses.
- **Scan info** carries the **Intensity profile**: the band's 64-bin return histogram with the window
  marked on it, so the sliders are set against what the scan returns.
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

### Clarity: a depth buffer and two cues

The 3D pipeline draws into the window's depth-stencil buffer. The 2D canvas owns the stencil half of
that buffer and writes no depth, so the clouds own the depth half: every square writes its own view
depth and is tested against it before another is drawn (`CompareFunction::Less`), which means a near
wall hides what is behind it instead of piling every point along the ray into one slab of pixels.
A target without a depth aspect — a `Stencil8` test target, or a web build that got no depth buffer —
compiles to the same pipeline with the depth test off, and the two cues below carry the picture alone.

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

Both live in the vertex shader of `crates/ui-egui/src/gpu3d.rs`. The depth is
`dot(point − eye, forward)`, which works in the orthographic projection too (where `clip.w` is
always 1), and it is what the cue fades on; the buffer's own depth is what hides a far point. The
uniform is a `repr(C)` struct of `f32` arrays, cast into the buffer with `bytemuck`, so the 20-byte
instance record (`[x, y, z]` and `size` as `f32`, then RGBA8 — `VERTEX3D_SIZE`) and the uniform
layout are both exactly what the shader expects.

### Colour ramps that survive outliers

`Elevation` and `Intensity` map the value to the blue→red ramp. A single spike through the
ceiling or one hot return would stretch the range and flatten the contrast of the other 99% of the
points, so the ramp runs over the **2%–98% window** by default, computed at load time from a
histogram over the points the level holds — 65 536 bins for the height, and 65 536 raw counts for
the return, which are folded into the 256 bins of the 0..255 scale the ramp is drawn on before the
percentiles are read (no sorting of four million points). The values outside the window are clamped
to its ends.

The window is a default, not a sentence: `LASRANGE` names the ends, the palette puts two sliders and
an **Auto** button under the ramp, and the legend row re-reads its ends from the same numbers. The
band's own profile is drawn too — the intensity histogram of 64 bins, folded from the 65 536 return
counts of that same walk, so a return lands in the bin the ramp reads it in — with the window
marked on it, so the choice of ends is made against what the scan actually returns, not by dragging
blind.

Both ramps and the class colours are drawn from the same functions the points use
(`Spectrum::apply`, `deviation_color`, `class_color`), so a legend cannot promise a colour the
drawing does not produce. A cloud whose points carry a class the classifier never writes — ASPRS 0,
the state of most cleaned exports — gets a chip named by its number, because the grey it draws still
needs a name.

### Seeing through the cloud

A dense cloud hides the drawing under it. The depth test makes that honest — a wall seen edge-on is
one solid surface, not a pile of every point along the ray — but an honest wall is still a wall
between the surveyor and the line drawn under it.

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

## Auto-RLV and Auto-3D (scan-to-BIM)

`AUTORLV` and `AUTO3D` turn a scan into building parts. The pipeline follows Zbirovský & Nezerka,
*Cloud2BIM: An open-source automatic pipeline for efficient conversion of large-scale point clouds
into IFC format* (arXiv:2503.11498v1) for the modelling stages; where the paper publishes no number,
the value is this fork's and is labelled so in `BimOptions`
(`crates/pointcloud/src/bim/mod.rs`). The commands model **slabs, storeys, walls and rooms**, detect
**openings** in every wall, measure the **cloud-to-model deviation** and route what the model cannot
explain to `RLV_FURNITURE`, and `AUTORLV` writes an **IFC 4 STEP** file when `ifc` names a path.
Automatic dimensions are the one stage still to come, and the reply says so in `notAvailable`.

### The stages

| Stage | Module | What it does |
|---|---|---|
| slabs | `bim/raster.rs`, `bim/slabs.rs` | A 5 cm vertical histogram of the band. A run of bins at least half as tall as the tallest is a slab candidate; a single below-cut bin with candidates on both sides joins the run, so the trough inside a rough floor is not read as two faces; neighbouring runs are paired into one slab a storey apart, and a lone face is thickened by `single_sided_slab_thickness`. The plan comes from a 5 cm density mask. |
| storeys | `bim/storeys.rs` | The band between one slab's top and the next slab's base, each face pulled back by `safety_margin` (0.10 m) so floor and ceiling points cannot enter a wall slice. |
| walls | `bim/walls.rs` | A horizontal slice at 0.9–1.0 of the storey's clear height, a 5 cm density mask at `threshold`, closing with radius `close_radius` cells, contours simplified by `epsilon` (never finer than one cell, because a 45° face cannot be placed more precisely than the lattice) and `angle_tolerance`, collinear pieces joined, then **parallel faces paired** 0.06–0.60 m apart into one wall axis with its thickness. A lone face on the outer contour becomes an exterior wall of `exterior_wall_thickness`, set half a thickness outside the face; a lone face inside the building (a courtyard) is never thickened. |
| zones | `bim/zones.rs` | Axis ends snapped onto other walls within `zone_gap`, each wall body laid out as the rectangle it occupies, probes 1 mm outside each face at seven stations, and the rectangle soup traced into closed rings; rings below `min_zone_area` are dropped. |
| openings | `bim/openings.rs` | Each wall body plus `opening_band` (0.05 m) on every side is read once from the LAS, thinned to `opening_decimate` (5 mm) and projected onto the wall's `(u, z)` elevation at `opening_cell` (2 cm). A cell counts as wall when its density reaches `opening_threshold` (0.5) of the rank-`opening_rank` (10th) densest cell, so nine density spikes cannot move the reference; a radius-1 closing bridges a 4 cm sampling crack. An empty region that touches neither wall end nor the top is kept when 0.60–1.60 m wide: a hole whose sill is at most `door_max_sill` (0.30 m) above the storey base and at least `door_min_height` (1.90 m) tall is a **door**; a hole whose sill is at least `window_min_sill` (0.50 m) up is a **window**. |
| deviation | `bim/deviate.rs` | Every cloud point is measured against the wall bodies and slabs: zero inside a body, otherwise the gap to the nearest face. The report carries the point count, RMSE, mean and max in mm and how many points lie above `deviation_threshold` (50 mm); off-model points are binned at `deviation_cell` (0.10 m) and connected clusters of at least `min_furniture_points` (50) become furniture footprints, at most `max_furniture_clusters` (4,096). |
| IFC | `bim/ifc.rs` | With `ifc` set to a path, `AUTORLV` writes ISO 10303-21 STEP (`FILE_SCHEMA(('IFC4'))`): project, site, building and storeys, walls, slabs, spaces, openings, doors and windows, with `IfcRelVoidsElement`/`IfcRelFillsElement` links whenever there are openings, IFC base64 GUIDs and `\X2\` escapes for non-ASCII names. The writer then reads its own text back — unique ids, every `#N` reference resolved, the project present and, when the building has openings, the void/fill relationships — and the reply cites the checked entity counts. |

`extract_auto_rlv` returns `AutoRlvResult`: per storey a `StoreyModel` with its walls and zones, the
slabs, and one `RoomScheduleRow` per closed room (code `E01`…, name `Spațiu`, usable area in m²,
perimeter in m, clear height). `AUTORLV` then detects the openings wall by wall, measures the cloud
against the model, and draws `RLV_WALLS` wall axes, `RLV_ZONES` room rings, `RLV_SLABS` slab plans,
`RLV_OPENINGS` two jamb lines per opening, `RLV_FURNITURE` one plan rectangle per deviation cluster,
and the schedule text on `RLV_SCHEDULE`. `AUTO3D` reads the same model and extrudes each wall axis
into two face planes; it adds no roof, because a roof is a slab the stage found above the top storey,
not an invention.

### Ours, and the paper's

Adopted from the paper's Table 6: `z_step` 0.05 m, a slab at 0.5 × the tallest bin,
`safety_margin` 0.1 m, the slice window [0.9, 1.0], a mask `threshold` of 0.01, the 5 × 5 closing
kernel (`close_radius` 2), `epsilon` 0.02 m, `angle_tolerance` 3°, and the rank-10 opening threshold
(`max10`). The paper's 1 m dilation/erosion and its `smoothing_factor` belong to its pixel-grid
pipeline; this one closes a metric mask, so neither has a counterpart here.

This fork's values, because the paper gives none or contradicts itself:

| Parameter | Default | Why |
|---|---|---|
| `cell` | 0.05 m | A metric raster cell; the paper's `grid_coefficient` contradicts its own figure by 50×. |
| `single_sided_slab_thickness` | 0.20 m | The paper's `bfs`/`tfs` are undefined; this names the one thing they are used for. |
| `min_wall_length` | 0.40 m | Table 5 publishes none. |
| `min_wall_thickness` / `max_wall_thickness` | 0.06 / 0.60 m | Table 5 publishes none. |
| `zone_gap` | 0.40 m | Snap reach for axis ends: closes the paper's own 35 cm corner failure without bridging a door (≥ 0.8 m). |
| `min_zone_area` | 0.5 m² | Table 5 publishes none. |
| `exterior_wall_thickness` | 0.30 m | Used only when the scan has no exterior face. |
| `opening_cell` | 0.02 m | The paper gives a rank and no cell; 2 cm is finer than the 5 cm plan lattice and resolves a jamb. |
| `opening_threshold` | 0.5 | The paper gives the rank and no percentage; half the rank-10 density keeps a sampled face and drops a hole. |
| `min_opening_width` / `max_opening_width` | 0.60 / 1.60 m | The paper names the width test and publishes no number: 0.60 m is an interior door, 1.60 m a wide window. |
| `door_min_height` / `door_max_sill` | 1.90 / 0.30 m | A door reaches head height and starts at the floor. |
| `window_min_sill` | 0.50 m | Above the door sill; the 0.30–0.50 m gap is refused rather than guessed at. |
| `opening_band` / `opening_decimate` / `max_wall_band_points` | 0.05 m / 5 mm / 200,000 | The band is the wall body plus reading margin, the points are thinned to the 5 mm the spec asks for, and one wall's stream is capped so a dense wall cannot exhaust memory. |
| `deviation_threshold` | 0.05 m | The spec names 50 mm; the paper gives no number. |
| `deviation_cell` / `min_furniture_points` / `max_furniture_clusters` | 0.10 m / 50 / 4,096 | A cell that bridges the sampling gaps of a thinned scan while separating two pieces of furniture; 50 points is an object rather than speckle; the cap bounds the report. |

`AUTORLV` and `AUTO3D` take these through `params`; `AUTORLV` also takes `ifc`, the path to write
(it is a command parameter beside `index`, not a `BimOptions` key). Without `ifc`, nothing is
written:

```json
{"index":0,"ifc":"C:/releveu.ifc",
 "params":{"cell":0.05,"z_step":0.05,"slab_ratio":0.5,"slice_from":0.9,"slice_to":1.0,
 "threshold":0.01,"close_radius":2,"epsilon":0.02,"exterior_wall_thickness":0.3,
 "exterior_scan":false,"zone_gap":0.4,"min_zone_area":0.5,
 "opening_cell":0.02,"opening_threshold":0.5,"opening_rank":10,
 "min_opening_width":0.6,"max_opening_width":1.6,"door_min_height":1.9,
 "door_max_sill":0.3,"window_min_sill":0.5,"opening_band":0.05,
 "opening_decimate":0.005,"max_wall_band_points":200000,
 "deviation_threshold":0.05,"deviation_cell":0.1,
 "min_furniture_points":50,"max_furniture_clusters":4096}}
```

Hostile values are refused with a message, not guessed at: a zero or NaN cell, a slice window
outside the storey, a threshold of zero, a minimum thickness above the maximum, an angle tolerance
over 90°, an exterior thickness of zero, an opening cell or decimation of zero, a rank of zero, a
width or height of zero, or a furniture budget of zero. `#![deny(clippy::unwrap_used, …)]` holds
throughout `bim/`, every index is `get()`, a raster is capped by a cell budget so a 100 km LAS
cannot ask for a 10⁹-cell grid, a wall band and the furniture clusters have their own budgets, and
loops are bounded.

### Measured on the Ap 6 scan (2026-10-25)

The 41.7-million-point section export used for the rendering work, driven over the control channel
on a release build (Windows, DX12):

| Step | Result |
|---|---|
| `LASOPEN` at the default 5 mm grid | **15.9 s this run** (17.0 s and 20.1 s on two earlier ones) — 5,826,743 points from 41,760,735, 7 levels |
| `AUTORLV` (defaults, with openings, deviation and IFC) | **46.2 s** — 2 slabs, 1 storey, 70 walls, 0 rooms, 0 openings, 41 furniture clusters, Su = 0.00 m²; the four modelling stages alone take **0.50 s**, the fine wall-band read **4.9 s** and the deviation **39.5 s** |
| Slabs | floor top 1.027 m and ceiling base 3.483 m, both single-sided at 0.20 m; 1,176 and 1,174 plan vertices |
| Storey | base 1.127 m, top 3.383 m, clear height 2.256 m |
| `AUTO3D` (defaults, with the opening read) | **5.5 s** — 70 wall planes (140 face quads in the 3D sketch), 2 slab faces, 0 openings, no roof; it was 0.52 s before the opening read was wired in |
| Entities in the drawing | `RLV_WALLS` **70** LINE, `RLV_SLABS` **2** LWPOLYLINE, `RLV_FURNITURE` **41** LWPOLYLINE, `RLV_OPENINGS` / `RLV_ZONES` / `RLV_SCHEDULE` empty; `RLV_3D_WALLS` 70 axis LINE plus 140 closed quads in the 3D sketch, `RLV_3D_SLABS` 2 closed quads (142 3D polygons in all) |

The slab histogram is where this scan first failed. Its ceiling's tallest 5 cm bin holds 948,085
points, so the 50 % cut sits at 474,042; the floor's tallest bin holds 429,129, 45 % of that, and
the old rule split the floor into a 0.93–0.98 m face and a 1.03–1.13 m face. Three faces then paired
into a phantom 2.4 m slab and the pipeline reported **zero storeys**. The one-bin bridge described
in the slab row restores the floor face and the ceiling face: **2 slabs, 1 storey**.

**0 rooms is the measured truth on this scan, not a failed command.** The wall stage finds 70 axes
at the paper's [0.9, 1.0] slice, but every room-sized loop is open at a doorway or passage: the
collinear gaps between wall ends run from 0.6 m to over 2 m, and `zone_gap` is 0.40 m precisely so a
door (≥ 0.8 m) is not bridged. The tracer does find loops where the walls really close — slices at
0.5–0.6 and 0.6–0.7 of the clear height return 1 and 3 zones — but they are 0.5–1.9 m² pockets
between wall and furniture fragments, not rooms. The openings stage can now find holes in the wall
bodies (none inside this scan's fragments at the default threshold, below), but the zone tracer
still does not use them: it would rather leave a room open than invent a wall across a door.

Plan B, measured on that same open file over the control channel:

- **Openings** at the default `opening_threshold` 0.5: **0** on 70 walls. The walls are fragments
  (median 0.70 m, 51 of them at least 0.60 m); on the walls whose densest 2 cm cell holds 100–600
  points, the 50 % cut leaves 0–1 % of the cells occupied, so the empty region percolates to the
  wall's top edge and is rejected. Lowering `opening_threshold` to 0.1–0.2 finds **one window**,
  1.08 × 0.58 m, sill 1.99 m, inside one wall body. **No doorway is found at any threshold**: this
  scan's doorways are full-height gaps between two wall fragments, with no lintel above, and the
  stage looks for holes inside a wall body. The default stays as the spec set it; a scan like this
  one answers through a `params` override.
- **Deviation**: the 5,826,743 points measure against 2 slabs and 70 wall axes at RMSE **291.6 mm**,
  mean 170.9 mm, max 4,751 mm, with 3,248,080 points above 50 mm and **41 furniture clusters**. The
  number is high because the model explains only the walls and the two slab faces: furniture,
  fittings, wall runs the wall stage did not model and points outside the slab plan are all
  off-model, which is exactly what `RLV_FURNITURE` draws.
- **IFC**: 175,885 bytes, **3,747 entities**, 4,116 references, 70 `IFCWALL`, 2 `IFCSLAB`, one each
  of `IFCPROJECT`, `IFCSITE`, `IFCBUILDING`, `IFCBUILDINGSTOREY`; 0 spaces, openings, doors and
  windows because the model has none. The writer's own reader closed the whole graph before the
  reply cited those counts.

The time moved with Plan B: `AUTORLV` is 46.2 s instead of the 0.50 s of the four modelling stages
— 4.9 s for the one streaming pass over 41.7 M points that reads 71 wall bands, and 39.5 s for the
deviation (measured in a scratch harness on 6.0 M points; the app holds 5,826,743). That is the
price of reading every wall's elevation and testing every point against the model; the modelling
stages alone still take 0.50 s.

### What the commands do not do yet

- **Automatic dimensions** are not placed; `RLV_DIMS` stays empty and the reply lists `dimensions`
  in `notAvailable`.

Everything else is computed, not promised. The IFC file is text this repository writes from the
public ISO 10303-21 encoding and the IFC 4 attribute tables, and its entity graph is checked by this
repository's own reader; it has not been round-tripped through a commercial BIM application, so how
a third-party application accepts it is untested. The fabricated six-wall box, the invented openings
and the fixed-fraction rooms the command used to draw are gone.

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

The plan view paints the points it keeps **from the bottom up** (`canvas::plan_order`). The 2D
pipeline blends premultiplied squares in list order and has no depth buffer, so the last square
drawn in a pixel is the one seen; file order is scan order, which puts a floor point after a
ceiling point as often as before it, and the plan then mixes every surface in the building into a
salt-and-pepper. Sorting the kept points by height, lowest first, leaves the highest point in each
pixel — the surface a top view looks at. The sort walks the screen-sized set the view keeps, not
the file.

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

### A second scan (Ap 6), and the colour window measured on it

A 41.7-million-point section export, driven over the control channel on a release build (Windows,
DX12, a 1500×950 window, the camera set once to `iso` + 35° perspective + fit and never touched
again, so the only thing that can move between two captures is colour).

| Step | Result |
|---|---|
| File | **41,760,735 points**, point format 3, RGB and intensity, 0.1 mm scale, no CRS, 9.2 × 14.3 × 9.6 m |
| `LASOPEN` at the default 5 mm grid | 5,826,743 points, 7 levels — **17.5 s** |
| On screen at fit | 464,954 points |
| `LASRANGE` elevation, intensity, reset | 4 ms, 13 ms, 19 ms; the mesh rebuilds on the next frame |
| `LASCOLOR rgb` / `Auto RGB` | 20 ms / 131 ms (the auto answer walks and sorts one level) |
| Elevation band 0.90–3.75 m → window 6.19–8.12 m | 114,020 of 1,425,000 px changed (8.0%), **the same 145,464 lit px** |
| Intensity band 0–255 → window 60–180 | 101,937 px changed (7.2%), the same lit px |
| Class colours | one chip, `class 0` — the file carries ASPRS 0 on every point |

The unchanged lit-pixel count is the point of the last two rows: a window is a colour change and
nothing else. The same camera, the same points, the same buffer size — only the ramp moved.

`Auto RGB` on this scan answers exposure 1.12× and contrast 0.88×, and the picture is barely
different (mean channel change 1.3 of 255). That is the algorithm working as designed, not failing:
the band's own 2%–98% luminance span is already 0.91 of the scale, so there is no contrast to win,
and a small lift is all the statistics justify. The scan looks flat in its own colours because it is
concrete and dust — its mean saturation is 24 of 255 against the elevation ramp's 139 — and the way
to read it is the ramp and the window, not a stronger multiplier on a photograph of grey.

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
