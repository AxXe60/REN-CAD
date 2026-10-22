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
| **Snap inteligent**: strat frontal (fără snap prin pereți) + mod Smooth/Exact | gata |
| **Constrângeri 3D**: Z-lock (plan orizontal), Axis Ortho (Shift) | gata |
| **Măsurare 3D**: lungime, dXY, dZ, pantă — live și ca comandă | gata |
| **Teren sub nor**: filtru morfologic cu ajustare pe pantă (planul etajului) | gata |
| **Georeferențiere**: CRS din fișier (GeoTIFF keys / WKT) + `.prj` la export | gata |
| **Cadrul de șantier**: `LASORIGIN` (punctul din lume de la 0,0,0) + `LASCRS` | gata |
| **Nori de 50–100M puncte**: citire cu plafon, fără să țină fișierul în memorie | gata |
| **Claritate 3D fără depth buffer**: front wall cut + depth cue | gata |
| **Comparație nor–nor (C2C)**: distanțe, RMS/mean/σ/p95, scară deviații | gata |
| **Clasificare automată**: înălțime peste teren + forma suprafeței (vegetație, pereți, zgomot) | gata |
| **Clasificare cu învățare**: k-means pe (înălțime, intensitate, rugozitate, verticalitate) + omul în buclă | gata |
| **Norul în fișier**: `LASOPEN {attach}` / `LASATTACH` — punctele se scriu în DWG/DXF (entități POINT + XRECORD) și revin la deschidere | gata |
| **Transparență / x-ray**: `LASOPACITY` + mască deterministă în shader | gata |
| **Contrast robust**: rampele folosesc percentilele 2%–98% | gata |
| Layere pe stroke (DXF per layer) + închidere polilinie | gata |
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
- Rampele (`Elevation`, `Intensity`) folosesc fereastra **2%–98%**, calculată la încărcare dintr-o
  histogramă (65.536 bin-uri pentru Z, 256 pentru intensitate — fără sortare): câteva valori
  extreme nu mai întind intervalul și nu mai șterg contrastul restului.
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
  **orbitarea nu reconstruiește nimic**, doar scrie uniforma view-projection (144 bytes, `bytemuck`).
- Buget: 3M puncte **vizibile**; nivelul e înrobit până încape. Zoom în → bila scade → nivelul fin
  devine accesibil exact unde se vede detaliul.
- **Fără depth buffer** (pipeline-ul upstream folosește doar stencil). În locul lui:
  - **Front wall cut** (`Cut`): punctele dintre ochi și `cut × dist` sunt aruncate în vertex shader
    (vârful lor iese din volumul de clip, deci nu se desenează). Camera intră în cameră fără să
    tai pereții din `LASCLIP`.
  - **Depth cue** (`Cue`): adâncimea = `dot(p − ochi, forward)` (merge și în orto, unde `clip.w`
    e mereu 1). Punctele din spatele țintei se estompează lin spre fundal (max. 45%, rampă
    pătratică) și le scade pătratul → prim-planul iese în evidență.

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
2. Fiecare cadru proiectează cursorul: se caută toate punctele din norul încărcat care cad în
   raza de 14 px, se calculează adâncimea lor față de cameră și se păstrează doar **stratul frontal**
   (adâncime ≤ minim + 0.15 m). Asta împiedică snap-ul prin pereți, fără depth buffer.
3. Modul de snap: **Exact** (punctul din nor) sau **Smooth** (centroidul punctelor din sfera de
   3 cm din jurul celui câștigător → zgomotul LiDAR de 2-5 mm se anulează). Implicit: Smooth.
4. Click stânga = vârf; rubber-band de la ultimul vârf; cerc verde = snap pe nor.
5. Dublu-click / click dreapta = termină stroke-ul; `Esc` = anulează; `C` = închide polilinia.
6. La terminare UI-ul cheamă comanda `3DLINE`; stroke-ul ajunge în `Session::sketch`.
7. `draw_sketch()` proiectează stroke-urile cu aceeași matrice și le pictează peste nor (portocaliu).
8. `3DEXPORT {path}` scrie DXF cu entități `LINE` la XYZ real (și `POINT` pentru un vârf singur),
   fiecare stroke pe layer-ul lui.

### 3.6 Constrângeri de desenare 3D
- **Z-lock** (tasta `Z` sau checkbox): orice vârf nou ia X/Y din snap, dar Z = Z-ul primului vârf
  al stroke-ului → tot stroke-ul rămâne într-un plan orizontal.
- **Axis Ortho** (tasta `Shift` sau checkbox): segmentul curent se proiectează pe axa cardinală
  (X, Y sau Z) cea mai apropiată de direcția mișcării față de ultimul vârf.
- Ambele sunt funcții pure în `pointcloud/src/snap.rs` (`z_locked`, `axis_locked`), testate fără
  fereastră.

### 3.7 Măsurare 3D
- Comanda `3DMEASURE {p1, p2}` întoarce `dist_3d`, `dist_2d_xy`, `dx`, `dy`, `dz`, `slope_deg`,
  `slope_pct` (matematica stă în `pointcloud/src/measure.rs`, ca să fie aceleași numere ca în UI).
- Modul **Measure** din paletă: două clickuri măsoară un segment (care se snapuiează pe nor); pe
  ecran rămâne segmentul albastru cu eticheta `8.184 m · dXY 7.731 · dZ 2.683`.
- În timpul desenului, rubber-band-ul arată live lungimea, dXY și dZ segmentului curent.

### 3.8 Terenul sub nor (M10a) — `pointcloud/src/ground.rs`
- Filtrul morfologic, cu corecție de pantă: celula 0,5 m → cel mai jos punct ca sămânță → sămânțile
  ce stau cu mult peste vecinii (masa, mașina) se aruncă → **planul** ajustat pe 3×3 celule → punctele
  pe plan sunt teren (clasa 2), cele de sub el sunt zgomot (clasa 18).
- Clasele stau **lângă** vârful de 16 bytes (în `CloudLevel.classes`), nu în el — GPU-ul nu le
  are nevoie, iar layout-ul rămâne neatins.
- `LASGROUND` (și butonul **Find ground** din paletă) clasifică; **Ground only** desenează doar
  terenul — pe scanul real, planul etajului rămâne, mobila dispare (737.687 puncte de teren din
  3.899.545).

### 3.9 Georeferențiere (M11a) — `pointcloud/src/crs.rs`
- CRS-ul se citește din fișier: cheile GeoTIFF (codul EPSG) sau WKT-ul OGC.
- `LASINFO` și paleta spun ce sistem e (EPSG + nume); `LASEXPORT` scrie WKT-ul lângă desen ca
  `out.prj` (convenția GIS).
- `LASCRS {epsg, falseEasting?, falseNorthing?}` declară sistemul proiectului pentru fișierele
  care nu-l spun (un scan Faro înregistrat nu are VLR-uri) + **offset-ul grilei locale**
  (Stereo 70 cu northing fără cifra milioanelor: `EPSG:31700, falseNorthing: -4000000`).
- Fork-ul **nu reproiectează**: desenul e 2D în unități locale, iar norul rămâne unde îl pune
  fișierul. Un fișier fără CRS spune asta, nu inventează.

### 3.10 Cadrul de șantier (M11b) — `LASORIGIN`, `LASCRS`
- Un scan înregistrat e la sute de km de originea desenului (fabrica: E 498 162, N 290 290).
  **Cadrul de șantier** le leagă: punctul din lume care stă în (0,0,0)-ul desenului.
- `LASORIGIN` îl pune (implicit: mijlocul primului nor, solul la 0); overlay-ul 2D, vederea 3D,
  `LASFRAME` și filtrul de teren îl respectă.
- `LASCRS {epsg}` declară sistemul proiectului pentru fișierele care nu-l spun (un scan Faro
  înregistrat nu are niciun VLR: georeferențierea stă în proiectul de topografie) → `.prj` la export.

### 3.11 Nori de 50–100 de milioane de puncte
- Citirea nu ține niciodată fișierul în memorie: punctele sunt puse în celule în timp ce fișierul
  se citește, iar când cele păstrate depășesc plafonul, celula se dublează și se regrupează.
- Plafonul (`maxPoints`, 6 milioane implicit) decide cât de mult detaliu rămâne: nivelul cel mai
  fin al unui scan de 53M are primele `maxPoints` puncte, repartizate uniform, nu pereții în
  detaliu. LOD-ul de streaming (niveluri pe măsură ce se citesc) e milestone-ul următor.

Nu există depth buffer în pipeline-ul 3D (folosește doar stencil), deci liniile și punctele nu se
ocludează reciproc. E o alegere conștientă: nu atingem pipeline-urile upstream. În schimb, snap-ul
prioritizat pe adâncime compensează lipsa depth buffer-ului.

## 4. Comenzi

Toate sunt `CommandSpec`, deci merg din linia de comandă, CLI, control JSON și MCP.

| Comandă | Efect |
|---|---|
| `LASINFO {path}` | doar header: puncte, bounds, scale, format, culoare |
| `LASEXPORT {path,out,z0?,z1?,decimate?,layer?}` | bandă → DXF de `POINT` |
| `LASOPEN {path,z0?,z1?,decimate?,maxPoints?,color?,name?}` | deschide norul în sesiune |
| `LASCOLOR {mode,index?}` | `rgb`, `elevation`, `intensity`, `single:#rrggbb` |
| `LASPECTRUM {spectrum?,index?}` | scara de intensitate: `grey`, `bluered`, `viridis`, `turbo`, `magma`, `diverging` |
| `LASDECIMATE {cell,index?}` | re-citește la altă celulă; `0` = toate punctele |
| `LASCLIP {min,max,index?}` / `{reset:true}` | crop box (tavan/pereți/podea) |
| `LASVISIBLE {visible?,index?}` | arată/ascunde |
| `LASGROUND {cell?,tolerance?,bump?,sigma?,show?}` | găsește terenul / desenează doar terenul |
| `LASOPACITY {opacity?,index?}` | cât de opac e norul (x-ray: punctele se rarează) |
| `LASCMP {index?,reference?,cell?}` | compară două nori: distanțe, RMS/mean/σ/p95, scară deviații |
| `LASCLASS {mode?,index?}` | clasifică norul: `auto`, `height` (benzi de vegetație), `shape` (pereți) |
| `LASATTACH {attach?,index?}` | atașează norul la desen (se scrie în fișier) sau îl face doar overlay |
| `LASCLUSTER {k?,cluster?,class?,index?}` | grupează punctele după caracteristici (k-means) și numește grupele |
| `LASORIGIN {x?,y?,z?,reset?}` | cadrul de șantier: punctul din lume de la (0,0,0) |
| `LASCRS {epsg?,name?,reset?}` | sistemul proiectului, pentru fișierele care nu-l spun |
| `LASFRAME {index?}` | încadrează XY în 2D |
| `LASCLOSE {index?,all?}` · `LASCLOUDS {}` | închide / listează |
| `3DLINE {points:[[x,y,z],…], layer?, closed?}` | adaugă o polilinie 3D (implicit `3D_DRAFT`) |
| `3DCLEAR` · `3DUNDO` · `3DLIST` | golire / ultimul stroke / listare (layer + closed) |
| `3DEXPORT {path,layer?}` | stroke-urile → DXF `LINE` cu Z, fiecare pe layer-ul lui |
| `3DSNAP {mode}` | `smooth` (implicit) sau `exact` |
| `3DMEASURE {p1,p2}` | `dist_3d`, `dist_2d_xy`, `dx`, `dy`, `dz`, `slope_deg`, `slope_pct` |

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
- Secțiunea 3D:
  - **Draw 3D** / **Measure**, **Finish** / **Close** / **Cancel**, **Undo** / **Clear**.
  - **Smooth** / **Exact** (modul de snap), **Z lock**, **Ortho**, câmpul de layer.
  - **Persp/Ortho**, **Fit**, **Top/Front/Iso**, slider de **Lens** și mărimea punctului.
  - **Cut** (front wall cut) și **Cue** (depth cue).

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
| `crates/pointcloud/src/cloud.rs` | `Cloud`, `CloudLevel`, piramidă, culori, percentile 2%–98% |
| `crates/pointcloud/src/sketch.rs` | `Sketch3d`, `Stroke3` (puncte, layer, closed) |
| `crates/pointcloud/src/snap.rs` | strat frontal, centroid, Z-lock, axis-lock |
| `crates/pointcloud/src/ground.rs` | filtrul de teren (morfologic + plan) și zgomotul |
| `crates/pointcloud/src/crs.rs` | CRS-ul din fișier (GeoTIFF keys / WKT) |
| `crates/pointcloud/src/measure.rs` | lungime, dXY, dZ, pantă |
| `crates/pointcloud/src/dxf.rs` | export DXF puncte + linii 3D (per layer) |
| `crates/engine/src/lib.rs` | `Session::clouds`, `Session::sketch`, `Session::snap3d` |
| `crates/engine/src/cmd/pointcloud.rs` | toate comenzile `LAS*` și `3D*` |
| `crates/ui-egui/src/canvas.rs` | overlay-ul 2D și filtrarea la view |
| `crates/ui-egui/src/gpu.rs` | pipeline-ul instanced 2D |
| `crates/ui-egui/src/view3d.rs` | camera, culling, 3D drafting, cut + cue |
| `crates/ui-egui/src/gpu3d.rs` | pipeline-ul 3D (front wall cut + depth cue în shader) |
| `crates/ui-egui/src/palettes/pointcloud.rs` | paleta |
| `docs/pointcloud.md` | documentația detaliată |

## 8. Limite / ce nu e încă

- LAZ doar citire; fără streaming COPC (fișierul se deschide dintr-o singură trecere).
- Fără filtru pe clasificare, fără export ortho PNG/GeoTIFF, fără histogramă de intensitate.
- Fără depth buffer în 3D: punctele nu se oclud reciproc (bine pentru nori translucizi). Snap-ul
  prioritizează adâncimea, deci nu sare prin pereți, iar front wall cut + depth cue fac imaginea
  lizibilă — dar rămân aproximații ale unui depth test adevărat.
- Fereastra 2%–98% taie valorile extreme: un punct mult mai sus decât restul ia culoarea de sus,
  nu a lui. E prețul contrastului restului.
- Stroke-urile 3D trăiesc în sesiune; persistă doar prin `3DEXPORT`.
- `maxPoints` limitează nivelul cel mai fin; peste el grila e înrobită (răspunsul comenzii spune cât).
- Desenul 2D upstream e 2D; nu am împins Z prin entități, snap-uri și grip-uri.
- Smooth snap-ul e un centroid într-o sferă de 3 cm: la suprafețe cu detaliu sub 3 cm poate
  netezi mici detalii (Exact e pentru asta).

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
- M6 (cameră + 3D drafting): implementat și verificat live (perspectivă/orto, zoom ancorat,
  snap pe nor, export DXF).
- M7 (snap cu strat frontal + Smooth/Exact, Z-lock, Axis Ortho, layere per stroke, închidere
  polilinie, `3DMEASURE`): implementat, teste unitare în `crates/pointcloud` și
  `crates/engine`, verificat live pe fișierul real (Z constant pe stroke, segment pe axă,
  măsurare 8.184 m / dXY 7.731 / dZ 2.683, export cu layer).
- M9 (claritate fără depth buffer: front wall cut + depth cue în shader; contrast robust prin
  percentilele 2%–98%): implementat, teste unitare (uniformă `bytemuck`, shader WGSL validat cu
  naga, percentile pentru Z și intensitate) și verificat live (zoom 30 → 15 m, 1.134.102 puncte pe
  ecran, perechea față spate clar diferențiată, rampă elevation cu contrast plin).
- M10a/M11a (filtru de teren cu ajustare pe pantă; CRS din fișier + `.prj` la export): implementat
  fără dependențe noi, teste unitare (7 pentru CRS, 6 pentru filtrul de teren), verificat live pe
  scanul real (737.687 puncte de teren, planul etajului extras, export cu `.prj` când fișierul
  are CRS).
- Următorul: M10b (secțiune verticală prin nor → polilinie exportabilă) și M10c (citire COPC pe
  octree, cu query pe nivel + bounds — `las` are deja `CopcReader`).
