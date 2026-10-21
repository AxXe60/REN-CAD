# Point clouds (LAS/LAZ)

> **Last reviewed:** 2026-10-21 · **Status:** M1 — slice to DXF · **Fork addition**

This fork adds point cloud support to CADCraft. M1 is the smallest useful step: read a LAS/LAZ
file, keep the points inside a Z band, and write them as ordinary DXF POINT entities the drawing
database already understands. The in-canvas overlay (M2) builds on the same crate.

## Commands

Both are plain `CommandSpec`s, so they work from the command line, the CLI, the JSON control
channel and MCP without extra wiring.

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

## Measured on real data (Ap 1, Bucharest)

| Step | Result |
|---|---|
| File | 3,899,546 points, point format 3, RGB, 0.1 mm scale |
| Band 2.04–2.20 m | **153,034 points** — identical to the PDAL reference (`filters.range`) |
| Decimated at 2 cm | 8,712 points, 799 KB DXF |
| Time (slice + write) | 0.45 s |
| CADCraft reads it back | `cadcraft-cli info`, `convert` → PNG |

## Design

- `crates/pointcloud` (L3) reads and slices; it knows nothing about egui or the drawing model.
- Points stream in batches of one million, so a 12-million-point scan never sits in memory whole.
- The DXF carries true colour (group 420) when the cloud has RGB, so the slice arrives looking
  like the scan.
- M2 will keep the cloud as a session resource (not DXF entities) and draw it through a new
  `Kind::Cloud` primitive with per-vertex colour.

## Limits in M1

- No in-canvas overlay yet: the slice is a DXF you open (or a TRACE over).
- No LAZ writing, no COPC streaming, no classification, no ortho PNG export yet.
- The export is ASCII DXF; a 10 cm slice of a large scan can reach tens of MB. Decimate first.

## Licence

`las` (MIT) and `laz` (Apache-2.0) are ordinary Cargo dependencies; nothing is vendored.
