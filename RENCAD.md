# RENCAD — ce am construit și cum funcționează

Rezumat scurt al fork-ului. Documentația detaliată (engleză) e în `docs/pointcloud.md`.

## 1. Ce este

RENCAD este un fork subțire al CADCraft: păstrează tot CADCraft-ul de bază și adaugă **norul de
puncte LAS/LAZ** (2D + 3D) și **desen 3D peste nor**. Cratele și binarul păstrează numele upstream,
iar `main` = upstream + commit-uri de fork, ca rebase-ul săptămânal să rămână simplu.

Regulă de bază: norii **nu** sunt entități de desen. Stau în `Session::clouds`, deci snap-urile,
selecția, undo-ul și salvarea văd doar ce a desenat utilizatorul.

## 2. Ce funcționează azi

| Funcție | Stare |
|---|---|
| Deschidere LAS/LAZ (`OPEN scan.las`, `LASOPEN`, paletă, drag-and-drop) | gata |
| Bandă Z (slice orizontal pentru trasare) | gata |
| Piramidă LOD + desen 2D pe GPU (un singur draw call) | gata |
| Moduri de culoare: RGB, elevație, **intensitate ca spectru**, culoare unică | gata |
| Decimare pe grilă 3D (pereții nu se prăbușesc) + „All" (toate punctele) | gata |
| Crop: elimină tavan / podea / pereți, fără re-citirea fișierului | gata |
| Paletă dedicată **Point Cloud** | gata |
| Vedere 3D orbit, doar punctele vizibile pe ecran | gata |
| Cameră: lentilă 35°, Persp/Ortho, zoom ancorat pe cursor, Top/Front/Iso | gata |
| **3D drafting**: polilinii peste nor, snap pe puncte, export DXF cu Z | gata |
| Branding RENCAD (doar numele afișat) | gata |

## 3. Cum funcționează, pe straturi

```
LAS/LAZ ──► crates/pointcloud ──► Session.clouds ──► canvas 2D (GPU)
                    │                    └────────► view3d 3D (GPU + egui)
                    └─ piramidă LOD            │
LAS ──► 3DLINE ──► Session.sketch ─────────────► overlay în view3d ──► 3DEXPORT (DXF)
```

### 3.1 Citirea și piramida LOD — `crates/pointcloud`
- Fișierul e citit în flux, în bucăți de 1M puncte; se păstrează doar banda `z0..z1`.
- `Cloud::from_slice` construiește niveluri de decimare pe **cuburi 3D**: o celulă = un punct,
  celulele cresc ×1.6 până la ~20k puncte (max. 16 niveluri).
- Fiecare vârf are **16 bytes**: `f32 x/y/z` relative la originea norului + RGB + intensitate.
- `level_for(px_per_m)` alege nivelul cu celula de ~2 pixeli pe ecran.
- Intensitatea e întinsă pe intervalul benzii la construcție (nu `/257`), apoi colorată cu o rampă
  albastru→roșu.
- Fișierele > 8M puncte sunt subțiate la 5 mm la citire; `maxPoints` implicit 6M, `0` = toate
  punctele până la plafonul de 20M.

### 3.2 Desenul 2D — `crates/ui-egui/src/canvas.rs` + `gpu.rs`
- Nivelul ales e decupat la view + 25% marjă, împachetat într-un `Kind::Cloud` și trimis pipeline-ului
  instanced: un quad de 6 vârfuri per punct, un draw call.
- Mesh-ul se reconstruiește doar când se schimbă nivelul, culoarea, crop-ul sau zona acoperită
  (centru/zoom cuantizat); pan-ul simplu doar mută buffer-ul.
- Fără GPU (wasm) există fallback pe CPU cu dreptunghiuri egui.

### 3.3 Vederea 3D — `crates/ui-egui/src/view3d.rs` + `gpu3d.rs`
- Înlocuiește canvasul în același dreptunghi; camera orbitează în jurul lui `target`, cu Z în sus.
- Se încarcă doar bila din jurul țintei (rază `3 × dist`), pentru că e aceeași la orice unghi:
  **orbitarea nu reconstruiește nimic**, doar scrie uniforma view-projection (80 bytes).
- Buget: 3M puncte **vizibile**; nivelul e înrobit până încape. Zoom în → bila scade → nivelul fin
  devine accesibil exact unde se vede detaliul.

### 3.4 Camera
- `Projection::Perspective`: 35° vertical implicit (lentilă normală, fără fish-eye); slider 15–90°.
- `Projection::Ortho`: proiecție paralelă; pereții rămân paraleli, scara e aceeași la orice adâncime.
  Ambele proiecții folosesc aceeași înălțime la planul țintei, deci comutarea nu schimbă încadrarea.
- `view_height = 2 · dist · tan(fov/2)` — baza pentru pan și pentru pixeli/metru.
- Zoom cu roata **ancorat pe cursor**: se ia punctul de sub cursor, se schimbă `dist`, se recalculează
  și se mută `target` cu diferența.
- Preseturi `Top` / `Front` / `Iso`; dublu-click = Fit; MMB / Shift+drag / right-drag = pan.

### 3.5 3D drafting — `view3d.rs` + `cmd/pointcloud.rs` + `pointcloud/src/sketch.rs`
1. Paleta → **Draw 3D** activează modul.
2. Fiecare cadru proiectează cursorul: se caută cel mai apropiat punct din scena încărcată
   (eșantion ~250k puncte, rază 14 px). Dacă nu există, click-ul cade pe planul view prin `target`.
3. Click stânga = vârf; rubber-band de la ultimul vârf; cerc verde = snap pe nor.
4. Dublu-click / click dreapta = termină stroke-ul; Esc = anulează.
5. La terminare UI-ul cheamă comanda `3DLINE`; stroke-ul ajunge în `Session::sketch`.
6. `draw_sketch()` proiectează stroke-urile cu aceeași matrice și le pictează peste nor (portocaliu).
7. `3DEXPORT {path}` scrie DXF cu entități `LINE` la XYZ real (și `POINT` pentru un vârf singur).

Nu există depth buffer în pipeline-ul 3D (folosește doar stencil), deci liniile și punctele nu se
ocludează reciproc. E o alegere conștientă: nu atingem pipeline-urile upstream.

## 4. Comenzi

Toate sunt `CommandSpec`, deci merg din linia de comandă, CLI, control JSON și MCP.

| Comandă | Efect |
|---|---|
| `LASINFO {path}` | doar header: puncte, bounds, scale, format, culoare |
| `LASEXPORT {path,out,z0?,z1?,decimate?,layer?}` | bandă → DXF de `POINT` |
| `LASOPEN {path,z0?,z1?,decimate?,maxPoints?,color?,name?}` | deschide norul în sesiune |
| `LASCOLOR {mode,index?}` | `rgb`, `elevation`, `intensity`, `single:#rrggbb` |
| `LASDECIMATE {cell,index?}` | re-citește la altă celulă; `0` = toate punctele |
| `LASCLIP {min,max,index?}` / `{reset:true}` | crop box (tavan/pereți/podea) |
| `LASVISIBLE {visible?,index?}` | arată/ascunde |
| `LASFRAME {index?}` | încadrează XY în 2D |
| `LASCLOSE {index?,all?}` · `LASCLOUDS {}` | închide / listează |
| `3DLINE {points:[[x,y,z],…]}` | adaugă o polilinie 3D |
| `3DCLEAR` · `3DUNDO` · `3DLIST` | golire / ultimul stroke / listare |
| `3DEXPORT {path,layer?}` | stroke-urile → DXF `LINE` cu Z |

Exemplu:

```json
{"id":1,"method":"engine.execute","params":{"command":"3dline",
 "params":{"points":[[0,0,1],[4,0,1],[4,3,1]]}}}
```

## 5. Paleta Point Cloud

- Listă de nori (puncte + niveluri); click pe rând alege norul.
- **Open** (rezoluție completă) / **Grid + Apply** (decimare) / **All** (toate punctele).
- **Colour**: RGB, Elevation, Intensity, color picker.
- **Remove walls / ceiling**: crop box pe axele X/Y/Z, reset din checkbox.
- **3D / 2D**, **Fit**, **Show**, **Close**.
- Secțiunea 3D: **Persp/Ortho**, **Lens**, **Top/Front/Iso**, **Fit**, mărime punct, **Draw 3D**,
  **Finish/Cancel**, **Undo/Clear** și contorul de stroke-uri.

## 6. Performanță măsurată (Ap 1, București)

| Pas | Rezultat |
|---|---|
| Fișier | 3.899.546 puncte, format 3, RGB, scală 0.1 mm |
| Bandă 2.04–2.20 m | 153.034 puncte (identic cu referința PDAL) |
| Decimat 2 cm | 8.712 puncte, DXF 799 KB, 0.45 s |
| `LASOPEN` tot fișierul, 2 mm | 1.394.540 puncte, 9 niveluri (~38 MB) |
| Mesh 2D la zoom implicit | 44.259 puncte → **2.1 ms** |
| Mesh 2D la 6 m | 551.870 puncte → **22 ms**, un draw call |
| 3D la fit, tot fișierul | 214.871 puncte vizibile → **3.4 ms**; orbitarea nu reconstruiește |
| 3D zoom | 506k → 8 ms · 1.13M → 18 ms · 2.45M → 40 ms |
| Recolorare | instant; mesh-ul se reface la cadrul următor |

## 7. Fișiere-cheie

| Fișier | Rol |
|---|---|
| `crates/pointcloud/src/read.rs`, `slice.rs` | citire LAS/LAZ, bandă Z, flux |
| `crates/pointcloud/src/cloud.rs` | `Cloud`, `CloudLevel`, piramidă, culori |
| `crates/pointcloud/src/sketch.rs` | `Sketch3d` (stroke-uri 3D) |
| `crates/pointcloud/src/dxf.rs` | export DXF puncte + linii 3D |
| `crates/engine/src/lib.rs` | `Session::clouds`, `Session::sketch` |
| `crates/engine/src/cmd/pointcloud.rs` | toate comenzile `LAS*` și `3D*` |
| `crates/ui-egui/src/canvas.rs` | overlay-ul 2D și filtrarea la view |
| `crates/ui-egui/src/gpu.rs` | pipeline-ul instanced 2D |
| `crates/ui-egui/src/view3d.rs` | camera, culling, 3D drafting |
| `crates/ui-egui/src/gpu3d.rs` | pipeline-ul 3D |
| `crates/ui-egui/src/palettes/pointcloud.rs` | paleta |
| `docs/pointcloud.md` | documentația detaliată |

## 8. Limite / ce nu e încă

- LAZ doar citire; fără streaming COPC (fișierul se deschide dintr-o singură trecere).
- Fără filtru pe clasificare, fără export ortho PNG/GeoTIFF, fără histogramă de intensitate.
- Fără depth buffer în 3D: punctele nu se oclud reciproc (bine pentru nori translucizi).
- Stroke-urile 3D trăiesc în sesiune; persistă doar prin `3DEXPORT`.
- `maxPoints` limitează nivelul cel mai fin; peste el grila e înrobită (răspunsul comenzii spune cât).
- Desenul 2D upstream e 2D; nu am împins Z prin entități, snap-uri și grip-uri.

## 9. Rulare și build

```bash
# Windows, din rădăcina repo-ului
. .\scripts\env.ps1                 # setează PATH + CARGO_TARGET_DIR
cargo run --release -p cadcraft -- --sample --control 47902
cargo xtask ci                      # fmt, clippy, teste, assets, layering, wasm
```

Control (JSON lines pe `127.0.0.1:47902`): `engine.execute`, `cmdline.input`, `ui.click`,
`ui.drag`, `ui.wheel`, `ui.key`, `ui.screenshot`. Protocolul complet: `docs/control-protocol.md`.

## 10. Stadiu

- M1–M5 (citire, overlay, LOD, paletă, 3D, rezoluție completă, spectru, crop): verificate pe
  fișierul real, `cargo xtask ci` verde.
- M6 (cameră + 3D drafting): implementat; testele și verificarea live sunt pasul imediat următor
  înainte de commit.
