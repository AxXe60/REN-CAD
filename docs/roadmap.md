# CADCraft roadmap detail

> **Last reviewed:** 2026-10-21 · **Last updated:** 2026-10-21 · **Change:** minor (RENCAD fork: point-cloud milestones) · **Target:** Autodesk AutoCAD 2027

Forward-looking plan: milestones with status, the current focus, and what comes next with
estimates. The summary is [ROADMAP.md](../ROADMAP.md); the work items behind every line are in
[gaps.md](gaps.md); numbers come from [target-app-parity.md](target-app-parity.md). Hours are
Opus 5.5 agent-hours.

## Current focus

1. **Gate and land.** (`cargo fmt` on main is fixed.) Add a PR workflow that runs
   `cargo xtask ci`, then review and land the 47 open PRs, most of them community bug fixes
   (S1–S3). ≈ 15–25 h.
2. **Stop losing data on save** (FF1, FF2, FF7) and save DWG/DXF at a chosen version (FF3).
   ≈ 30–45 h.
3. **A real-file corpus** and a check of our output in AutoCAD (FF5, needs the owner).
4. **Precision input:** object snap tracking, extension/parallel snaps, FROM/M2P/TK, typed snap
   overrides, editable dynamic input (U1–U4). ≈ 25–35 h.
5. **Unstub the prompt options** of the most-used commands (F1). ≈ 15–25 h.
6. **Autosave, recovery, AUDIT, RECOVER** (F4). ≈ 10–15 h.
7. **The rest of the alpha gate:** block editor and REFEDIT, xref attach (F2, F3), and plot styles
   with a plot dialog and preview (F14). ≈ 50–80 h.

Items 2–5 and 7 are the [alpha gate](#alpha-gate) rows.

## Point clouds (RENCAD fork)

The fork's own track, beside the upstream plan. AutoCAD has no native point-cloud editor, so the
bar is the survey workflow: open the scan, cut it, look at it, trace it, export it. Detail in
[pointcloud.md](pointcloud.md); the summary is in [ROADMAP.md](../ROADMAP.md).

| Milestone | What it delivers | State |
|---|---|---|
| M1–M2 | Read LAS/LAZ, cut a Z band, write it as DXF `POINT`s, keep the band in the session and draw it in the canvas | done |
| M3–M4 | Decimation pyramid and one GPU draw call, colour modes and live decimation; **Point Cloud** palette, **3D view** | done |
| M5 | Full resolution, intensity as a spectrum, a 3D-cube grid (walls keep their height), crop box for the walls and the ceiling | done |
| M6 | 3D camera (35° lens or true ortho, zoom anchored at the cursor, Top/Front/Iso); 3D drafting over the cloud, exported as DXF `LINE`s with Z | done |
| M7 | Smart snapping (front layer only, `Exact`/`Smooth`), Z lock, axis ortho, per-stroke layers, closed polylines, `3DMEASURE` and the measure tool | done |
| M9 | Front wall cut and depth cue in the vertex shader; colour ramps over the 2%–98% window | done |
| M9b | Reading the picture: the clouds draw into the window's depth buffer, so a near point hides a far one; `LASRANGE` names the window a ramp reads, and the palette's sliders, colour legend and intensity profile show exactly what it reads; `LASCOLOR` carries the RGB exposure and contrast, with `auto` from the cloud's own luminance histogram | done |
| M10a | The ground under the cloud: a morphological filter with a slope correction, classes beside the 16-byte vertex, `LASGROUND`, **Find ground** / **Ground only** | done |
| M11a | Georeferencing: the CRS the file names (GeoTIFF keys or WKT), shown in the palette and `LASINFO`, written as a `.prj` sidecar next to an exported drawing | done |
| M11b | The site frame: `LASORIGIN` ties the drawing's (0, 0, 0) to a world point; `LASCRS` declares the system's code for the files that name none | done |
| M11c | Big surveys: the read keeps the memory flat — the cell grows while the file streams — so a 53-million-point scan opens without holding the file in memory | done |
| M10b | A vertical section through the cloud: a plane cut, a profile polyline, exported as a drawing (`LASSECTION`) | done |
| M10c | COPC on the octree (read only the levels and bounds in view) and LAZ writing (≈ 10–16 h) | planned |
| M11d | Reprojection between CRSs, so a scan and a georeferenced drawing can share a frame (≈ 12–20 h) | planned |
| M12 | A streaming LOD: the levels built while the file is read, so a huge survey keeps its walls sharp (≈ 12–20 h) | planned |
| M13 | **Seeing through the cloud**: `LASOPACITY` plus a deterministic mask in the vertex shader, so a wall seen edge-on stops hiding the drawing (done) | done |
| M14 | **The camera**: the orbit target follows the point under the cursor (the front-layer snap), the presets (Top/Front/Iso) animate instead of jumping, and a walk mode locks the eye's height for interiors (≈ 10–16 h) | planned |
| M15 | **The palette**: a floating camera bar over the 3D view, collapsible sections, a mini Z-histogram behind the crop slider (≈ 8–14 h) | planned |
| M16 | **Pick to pivot**: the orbit turns around the point under the cursor (the eye and the target both rotate around it, so the picture never jumps) and `Ctrl+Scroll` drives the front wall cut on its own (≈ 6–10 h) | planned |
| M17 | **The focus lens**: near the cursor the cloud is whole again (the opacity and the thinning ramp back), so the vertex lands on a detail even with the cloud at 25% (≈ 4–8 h) | planned |
| M18 | **A rotated crop**: the crop box turns with a wall (`Align Crop to Line` / `LASCROPALIGN`), so a facade at 37° can be cut | done |
| M19 | **Romanian** in the interface catalog (`ro`), the fifteenth language (≈ 4–6 h) | planned |
| M20 | **Compare two clouds (C2C)**: the distance from every point to the nearest point of a reference, the survey statistics (mean, σ, RMS, p95) and the deviation scale (done) | done |
| M21 | **Classify the cloud**: the height above the ground in the three vegetation bands, the shape of the surface (the walls), the noise — `LASCLASS` with the class colours (done) | done |
| M22 | **The classification with learning**: the features of every point (the height, the roughness, the planarity, the intensity) clustered without supervision, the clusters named by the user — the human in the loop (done: `LASCLUSTER`) | done |
| M25 | **The cloud belongs to the drawing**: `LASOPEN {attach}` / `LASATTACH` — the drawing *refers* to the scan (the LAS file, the band, the grid, the colour mode and where the scan sits in the drawing) in an `XRECORD` of the named object dictionary; the file keeps no points and the scan is loaded from the disk when the file is opened (done) | done |
| M26 | The scan that is not on this computer: a relative path beside the drawing, the search in the folders the survey uses, and the choice to look for it by hand (≈ 4–6 h) | planned |
| M23 | The comparison grown up: a coarse-to-fine registration (ICP) before the comparison, and the least-square local plane instead of the plain nearest point — the two refinements the comparison literature measures (≈ 8–14 h) | planned |
| M24 | The scan-to-BIM parts that need no learning — `AUTORLV` / `AUTO3D` run the full Cloud2BIM pipeline: slabs with inner hole/void detection, storeys, walls from parallel-face slice, closed room zones with schedule, wall openings (doors/windows), cloud-to-model deviation with furniture clusters, automatic aligned room dimensions on `RLV_DIMS`, and self-checked IFC 4 STEP export | done |
| M11 | The 3D strokes as real drawing entities — waits for the 3D kernel, so the upstream's own 3D milestone is the blocker | planned |
| M13 | A height/DSM grid over the band, with contours (≈ 12–18 h) | planned |
| M14 | A local cache of the decimated pyramid next to the scan, so a 1.8 GB survey opens in a second the second time (≈ 6–10 h) | planned |

M10b and M18 landed; M10c (COPC/LAZ) and M19 (Romanian) are the next ones that can land without the upstream's 3D kernel.

## Alpha gate

The core workflows a typical AutoCAD professional does every day (craftrules progress-docs
standard). CADCraft is **pre-alpha** because four of the five fail the gate; the failing rows are
the alpha checklist. Main platform: macOS.

| Core workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---|
| Draw and modify precisely with object snaps and tracking | **partial, blocking** | Running snaps, polar, ortho and coordinate entry work; object snap tracking, extension and parallel snaps are toggles with no behaviour; no FROM/M2P/TK or typed snap overrides; TRIM/EXTEND standard mode, OFFSET Erase/Layer/Multiple, ROTATE Reference print "not available yet"; ellipse/spline intersections approximate ([ui-parity.md](ui-parity.md), [gaps.md](gaps.md) U1, U3, U4, F1) | 35–50 |
| Dimension and annotate | partial, not blocking | All core DIM types, associativity, DIMSTYLE variables (43 of ~80), TEXT/MTEXT, MLEADER, tables work; no DIMSTYLE dialog, MTEXT editor not in place, MLEADER thin (F7, F9, F10). A drafter can dimension a sheet end to end | 0 for the gate (beta work) |
| Use blocks (insert library blocks, edit them, attributes, xrefs) | **partial, blocking** | Static BLOCK/INSERT/attributes, ATTSYNC, BATTMAN editing, xrefs (#525) and the Block Editor (BEDIT/BSAVE/BCLOSE: every reference updates) work; no REFEDIT, so a block can't be edited in place in the drawing; no XCLIP; dynamic blocks from other drawings lose their behaviour (F2, F3) | 35–55 |
| Layers, layouts and plotting a sheet | **partial, blocking** | Layers, layouts, viewports, page setups, PDF output, plot styles (CTB/STB, #410), the Plot dialog (#160), Plot Preview and PUBLISH (#186) and plotter devices (#564) work; printers only via PDF/PNG/SVG files (F14, H3) | 15–25 |
| Save DWG that AutoCAD opens without loss, and reopen others' DWGs | **no** | DXF keeps what we don't model (#528) and writes native MULTILEADERs (#459, not yet tested through DWG); through DWG, acadrust drops custom-class entities and objects and some foreign data; never checked in AutoCAD; no real-file tests; a 10 MB DWG failed after ~2 min (#57) (FF1–FF6) | 40–60 + owner (real-file corpus, AutoCAD check) |
| **To alpha** | | | **≈ 125–190** |

Ready for real work is ≈ 38%, also under the ~40% alpha bar; closing the gate rows lifts it past
40%.

## Beta checklist

Beta means: close to feature complete for mainstream 2D work, opens and saves DWG reliably,
remaining gaps are known bugs and edge features (≈ 75% ready for real work).

| Item | State | Est. |
|---|---|---|
| DWG/DXF: nothing lost on save; MULTILEADER native; version choice; code pages; real-file corpus green; verified in AutoCAD | not started | 50–75 |
| 2D geometry exact for ellipses and splines; polyline offset correct; relative tolerances | not started | 50–80 |
| Precision input at AutoCAD level (tracking, dynamic input, overrides, point modifiers, grips, selection keywords, context menus) | partial | 90–140 |
| Every Draw/Modify prompt option works; associative arrays | partial | 40–60 |
| Annotation depth: DIMSTYLE manager and variables, missing DIM commands, in-place MTEXT, fields, MLEADER editing, annotative scaling, tables | partial | 90–130 |
| Xrefs, block editor, dynamic blocks, groups, attribute tools | not started | 90–135 |
| Plot styles, plot preview, PUBLISH, non-rectangular viewports | partial | 30–45 |
| Autosave, recovery, AUDIT/RECOVER, Options dialog, templates, SHX, user `.lin`/`.pat` | partial | 40–60 |
| AutoLISP core subset and SCRIPT | done (AutoLISP #758, SCRIPT #693); Visual LISP, DCL and reactors to come | 30–50 |
| Catalog plus AutoCAD for Mac's languages | not started | 30–45 |
| Stability: CI gate, issue backlog, command smoke sweep, geometry fuzzing | partial | 30–50 |
| Platforms: web open/save, Windows/Linux runtime checks; performance at 1M entities and large DWG | partial | 20–35 |
| **Total to beta** | | **≈ 600–930** |

About 70% of this parallelises across 4–6 agents (one per row), so elapsed time is roughly
150–250 hours.

## Milestones

Status re-scored on 2026-10-10 by depth, not presence (the 2026-10-07 percentages, reported by the
agents that built each milestone, are in the progress log of [ROADMAP.md](../ROADMAP.md)).

| # | Milestone | Status | Remaining (h) |
|---|---|---|---|
| M0 | Skeleton and vertical slice: workspace, AutoCAD-style UI, command line, draw/modify basics, DXF, CLI, MCP, web | done | — |
| M1 | Drafting core: every 2D Draw command and option, dynamic input, object snap tracking, snap overrides | ≈ 55% (Draw breadth strong; tracking, dynamic input, overrides missing) | 35–55 |
| M2 | Modify: grips, PEDIT, SPLINEDIT, LENGTHEN, BLEND, ALIGN, associative arrays, trim/extend for all curves, MATCHPROP UI | ≈ 50% | 45–70 |
| M3 | Layers and properties | ≈ 70% (open layer bugs #71–#75, #87) | 10–15 |
| M4 | Annotation: DIM*, DIMSTYLE manager, MLEADER, in-place MTEXT, fields, tables, annotative scaling, SHX | ≈ 40% | 90–130 |
| M5 | Hatch and blocks: hatch depth, block editor, dynamic blocks, xrefs, groups, attributes | ≈ 30% | 105–160 |
| M6 | Layouts and plotting: viewports, page setups, plot styles, preview, PUBLISH | ≈ 35% | 35–50 |
| M7 | Files: DWG/DXF fidelity and versions, RECOVER/AUDIT, templates, autosave, ETRANSMIT, underlays | ≈ 30% | 170–280 (incl. other formats) |
| M8 | Inquiry and utilities, OPTIONS, customisation | ≈ 25% | 45–70 |
| M9 | Parametric | ≈ 50% | 20–30 |
| M10 | Performance: 1M entities at 60 fps, large DWG loads | ≈ 60% (internal numbers only) | 20–40 |
| M11 | 3D: UCS, orbit, visual styles, solids, surfaces, meshes, sections, rendering | ≈ 2% | 300–450 |
| M12 | Automation: AutoLISP, SCRIPT, action recorder, sheet sets, Express Tools | ≈ 15% (MCP, CLI, control channel done) | 130–220 |
| M13 | 1.0 polish: preferences, workspaces, localisation, accessibility, signed packages, docs | ≈ 15% (signed packages for every platform exist; themes done; string catalog with Ukrainian and German, live language switching, #36) | 110–170 |
| — | Geometry precision (cross-cutting, see [geometry-parity.md](geometry-parity.md)) | ≈ 40% | 70–110 |
| — | AI features | ≈ 0% in-app | 40–80 |
| | **Remaining to full parity** | | **≈ 1,300–2,100** |

Milestone rows overlap a little with the cross-cutting rows; the total is the dimension sum in
[target-app-parity.md](target-app-parity.md), not the column sum.

## After beta

3D foundations (UCS, 3D views, orbit, then solids per `plan/adr/0001`), sheet sets, the full
AutoLISP surface, AI features with openly licensed local models, and every remaining language.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-26 | minor | M24: scan-to-BIM Plan C landed — P12 slab holes / voids and P13 automatic room dimensions on `RLV_DIMS`; `notAvailable` now empty; M24 complete |
| 2026-10-25 | minor | M24: scan-to-BIM Plan B landed — openings from each wall's elevation raster, cloud-to-model deviation with furniture clusters on `RLV_FURNITURE`, and a self-checked IFC 4 STEP export through `ifc`; automatic dimensions remain |
| 2026-10-25 | minor | M24: scan-to-BIM Plan A landed — `AUTORLV` and `AUTO3D` now run the Cloud2BIM stages (slabs, storeys, walls, zones) instead of a bounding-box fake; openings, deviation and IFC are Plan B |
| 2026-10-12 | minor | Block Editor for static blocks landed: "Use blocks" evidence updated; status and hours unchanged (REFEDIT and dynamic blocks remain) |
| 2026-10-11 | minor | Alpha-gate evidence refreshed for merged work (#459, #525, #528, #160, #186, #564); statuses and hours unchanged |
| 2026-10-10 | minor | Merged main: fmt fixed, Ukrainian catalog noted in M13, ready for real work 38% |
| 2026-10-10 | minor | Alpha gate added (5 core workflows, 4 failing); stage pre-alpha |
| 2026-10-10 | major | Created from the old ROADMAP's milestone table and current focus; milestones re-scored by depth; beta checklist added |
