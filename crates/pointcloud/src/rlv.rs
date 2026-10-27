//! Auto-RLV (Releveu 2D) and Auto-3D building reconstruction from LAS scans.
//!
//! The modelling is [`crate::bim`]'s: the vertical histogram that finds the slabs, the storey
//! split, the walls of the ceiling slice and the zones the wall axes close into. This module
//! gathers that output into the shapes `AUTORLV`, `AUTO3D` and the room schedule draw; it invents
//! no geometry of its own.
//!
//! What is not here: openings, the cloud-to-model deviation and the IFC writer. They are Plan B of
//! `docs/superpowers/plans/2026-10-25-cloud2bim-scan-to-bim.md`; a caller asking for them is
//! answered [`BimError::NotAvailable`], never with a guessed opening or a made-up room.
//!
//! Designed under the `never-crash` standard: no `unwrap`, `expect`, `panic`, `todo`, or `unsafe`.

use std::collections::HashMap;

use crate::bim::{self, BimError, BimOptions, Column, Slab, Stair, Storey, Wall, Zone};

/// Class code assigned to interior furniture / clutter points removed before RLV & 3D extraction.
pub const CLASS_FURNITURE: u8 = 22;

/// Summary of the volumetric interior furniture filter (`LASFURNITURE`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FurnitureFilterResult {
    /// Total input points inspected.
    pub total_points: usize,
    /// Structural points kept (walls, columns, floor slab, ceiling slab).
    pub structural_points: usize,
    /// Furniture and interior clutter points filtered out.
    pub furniture_points: usize,
    /// Detected finished floor elevation (Z min of slab), in metres.
    pub floor_z: f64,
    /// Detected ceiling elevation (Z max of slab), in metres.
    pub ceiling_z: f64,
    /// Masks parallel to input points (`true` = structural keep, `false` = furniture/clutter).
    pub keep_mask: Vec<bool>,
}

/// One row of the automatic RLV room schedule table (`RLV_SCHEDULE`).
#[derive(Clone, Debug, PartialEq)]
pub struct RoomScheduleRow {
    pub code: String,
    pub name: String,
    pub area_m2: f64,
    pub perimeter_m: f64,
    pub clear_height_m: f64,
    pub center: [f64; 2],
    pub boundary: Vec<[f64; 2]>,
}

/// One storey of the model: the walls standing in it and the rooms they close.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreyModel {
    /// The void between two slabs, with [`BimOptions::safety_margin`] already taken off its ends.
    pub storey: Storey,
    /// The wall axes, measured where both faces were scanned and synthesised where only the room
    /// side was.
    pub walls: Vec<Wall>,
    /// The closed room polygons the axes make, inside the wall faces.
    pub zones: Vec<Zone>,
    /// [`Self::zones`] as the schedule reads them, in the same order.
    pub rooms: Vec<RoomScheduleRow>,
    /// The structural columns standing in the storey.
    pub columns: Vec<Column>,
    /// The detected wall openings (doors and windows).
    pub openings: Vec<crate::bim::openings::Opening>,
    /// The stairs detected within this storey.
    pub stairs: Vec<Stair>,
}

/// Full output of the `AUTORLV` command: the slabs the scan has, and each storey's walls and rooms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoRlvResult {
    /// The walking surface of the lowest slab.
    pub floor_z: f64,
    /// The underside of the highest slab.
    pub ceiling_z: f64,
    /// Every horizontal surface the histogram found, ordered by `base_z`.
    pub slabs: Vec<Slab>,
    /// The storeys between them, in the same order.
    pub storeys: Vec<StoreyModel>,
    pub total_usable_area_m2: f64,
    pub total_perimeter_m: f64,
    /// All stairs detected across storeys.
    pub stairs: Vec<Stair>,
}

impl AutoRlvResult {
    /// How many wall axes the pipeline found, across every storey.
    pub fn walls(&self) -> usize {
        self.storeys.iter().map(|s| s.walls.len()).sum()
    }

    /// How many closed rooms it found.
    pub fn zones(&self) -> usize {
        self.storeys.iter().map(|s| s.zones.len()).sum()
    }

    /// How many rows the schedule holds: one per zone.
    pub fn rooms(&self) -> usize {
        self.storeys.iter().map(|s| s.rooms.len()).sum()
    }

    /// How many structural columns were detected across every storey.
    pub fn columns(&self) -> usize {
        self.storeys.iter().map(|s| s.columns.len()).sum()
    }

    /// How many wall openings (doors and windows) were detected across every storey.
    pub fn openings(&self) -> usize {
        self.storeys.iter().map(|s| s.openings.len()).sum()
    }

    /// How many partial-height walls / parapets were detected across every storey.
    pub fn parapets(&self) -> usize {
        self.storeys.iter().flat_map(|s| &s.walls).filter(|w| w.is_parapet).count()
    }

    /// How many stairs were detected across every storey.
    pub fn stairs(&self) -> usize {
        self.stairs.len()
    }
}

/// A 3D wall for `AUTO3D`, extruded from the axis between the storey's floor and ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wall3d {
    pub base_p1: [f64; 3],
    pub base_p2: [f64; 3],
    pub top_z: f64,
    pub thickness: f64,
}

/// A horizontal slab face for `AUTO3D`, at the elevation a person sees from the room.
#[derive(Clone, Debug, PartialEq)]
pub struct Slab3d {
    /// `floor_slab`, `ceiling_slab` or `slab`.
    pub kind: &'static str,
    pub vertices: Vec<[f64; 3]>,
    pub holes: Vec<Vec<[f64; 3]>>,
    pub elevation_m: f64,
    pub thickness_m: f64,
}

/// Full output of the `AUTO3D` building reconstruction: the walls and slabs the scan has. The wall
/// slice sits above furniture height, so what stands in the rooms below it cannot become a wall,
/// and nothing is invented for what the scan did not show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Auto3dBuilding {
    pub floor_z: f64,
    pub ceiling_z: f64,
    pub walls_3d: Vec<Wall3d>,
    pub slabs: Vec<Slab3d>,
    pub columns: Vec<Column>,
}

/// Filter out interior furniture and clutter from an apartment/building scan by checking vertical
/// column continuity between the floor and ceiling slabs.
///
/// Structural walls and columns span at least `min_wall_ratio` of the room height, while floor and
/// ceiling slabs lie within `slab_margin` of the 5th/95th Z percentiles. Isolated low clusters
/// (sofas, tables, beds, cabinets) inside the room volume are marked `false` in `keep_mask`.
///
/// This is the `LASFURNITURE` filter, not part of `AUTORLV`: the Cloud2BIM pipeline defends the
/// walls with its own ceiling slice, and deleting a point before measuring would hide it from the
/// deviation stage Plan B adds.
pub fn filter_interior_furniture(points: &[[f64; 3]], cell_xy: f64, slab_margin: f64, min_wall_ratio: f64) -> FurnitureFilterResult {
    if points.is_empty() {
        return FurnitureFilterResult::default();
    }
    let cell = if cell_xy.is_finite() && cell_xy >= 0.05 { cell_xy } else { 0.15 };
    let margin = if slab_margin.is_finite() && slab_margin >= 0.02 { slab_margin } else { 0.18 };
    let ratio = if min_wall_ratio.is_finite() { min_wall_ratio.clamp(0.35, 0.95) } else { 0.65 };

    let mut zs: Vec<f64> = points.iter().map(|p| p[2]).filter(|z| z.is_finite()).collect();
    if zs.is_empty() {
        return FurnitureFilterResult::default();
    }
    zs.sort_by(f64::total_cmp);
    let floor_z = zs[(zs.len() * 3) / 100];
    let ceiling_z = zs[(zs.len() * 97) / 100];
    let room_h = (ceiling_z - floor_z).max(0.5);

    // Measure vertical span and verticality in each XY column cell.
    let inv = 1.0 / cell;
    let mut columns: HashMap<(i64, i64), (f64, f64, usize)> = HashMap::new();
    for p in points {
        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            continue;
        }
        if p[2] <= floor_z + margin || p[2] >= ceiling_z - margin {
            continue;
        }
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        let entry = columns.entry(key).or_insert((p[2], p[2], 0));
        if p[2] < entry.0 {
            entry.0 = p[2];
        }
        if p[2] > entry.1 {
            entry.1 = p[2];
        }
        entry.2 = entry.2.saturating_add(1);
    }

    let min_span = (room_h - 2.0 * margin).max(0.4) * ratio;
    let mut keep_mask = Vec::with_capacity(points.len());
    let mut structural_points = 0usize;
    let mut furniture_points = 0usize;

    for p in points {
        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            keep_mask.push(false);
            continue;
        }
        // Keep floor and ceiling slabs.
        if p[2] <= floor_z + margin || p[2] >= ceiling_z - margin {
            keep_mask.push(true);
            structural_points += 1;
            continue;
        }
        let key = ((p[0] * inv).floor() as i64, (p[1] * inv).floor() as i64);
        let is_wall = columns.get(&key).is_some_and(|&(z_lo, z_hi, count)| (z_hi - z_lo) >= min_span && count >= 3);
        if is_wall {
            keep_mask.push(true);
            structural_points += 1;
        } else {
            keep_mask.push(false);
            furniture_points += 1;
        }
    }

    FurnitureFilterResult { total_points: points.len(), structural_points, furniture_points, floor_z, ceiling_z, keep_mask }
}

/// The whole Cloud2BIM model of a scan: [`bim::slabs::find`], [`bim::storeys::split`], and then
/// [`bim::walls::extract`] and [`bim::zones::build`] in every storey.
///
/// The options are the pipeline's own ([`BimOptions`]); nothing here reads the file, the selection
/// or the UI. A cloud the histogram finds no surface in is [`BimError::Empty`] — the command says
/// so rather than drawing an empty frame — and a scan whose slabs stand too close for a room keeps
/// its slabs and reports no storey, because that is what the scan shows.
///
/// # Errors
///
/// Whatever the stages refuse: [`BimError::InvalidCell`] for an unusable option,
/// [`BimError::TooLarge`] for a budget, [`BimError::Empty`] when nothing can be built.
pub fn extract_auto_rlv(points: &[[f64; 3]], opts: &BimOptions) -> Result<AutoRlvResult, BimError> {
    let mut slabs = bim::slabs::find(points, opts)?;
    for slab in &mut slabs {
        let regions = bim::slabs::detect_split_levels(points, slab, opts);
        slab.regions = regions;
    }
    let storeys = bim::storeys::split(&slabs, opts);
    let mut models = Vec::with_capacity(storeys.len());
    let mut all_stairs = Vec::new();
    let mut code = 1usize;
    for storey in &storeys {
        let walls = bim::walls::extract(points, storey, opts)?;
        let storey_pts = bim::storeys::points_in(points, storey, opts);
        let mut openings = Vec::new();
        for (i, wall) in walls.iter().enumerate() {
            if let Ok(mut found) = bim::openings::detect(&storey_pts, wall, i, opts) {
                openings.append(&mut found);
            }
        }
        let zones = bim::zones::build(&walls, opts)?;
        let columns = bim::detect_columns(points, storey, &walls, opts)?;
        let stairs = bim::slabs::detect_stairs(points, &slabs, storey.index, storey.base_z, storey.top_z);
        all_stairs.extend(stairs.clone());
        let clear_height = round2(storey.top_z - storey.base_z);
        let mut rooms = Vec::with_capacity(zones.len());
        for zone in &zones {
            rooms.push(RoomScheduleRow {
                code: format!("E{code:02}"),
                name: "Spațiu".to_string(),
                area_m2: round2(zone.area),
                perimeter_m: round2(zone.perimeter),
                clear_height_m: clear_height,
                center: ring_centroid(&zone.ring),
                boundary: zone.ring.clone(),
            });
            code = code.saturating_add(1);
        }
        models.push(StoreyModel { storey: *storey, walls, zones, rooms, columns, openings, stairs });
    }
    let floor_z = slabs.first().map(|s| round2(s.top_z)).unwrap_or(0.0);
    let ceiling_z = slabs.last().map(|s| round2(s.base_z)).unwrap_or(floor_z);
    let total_usable_area_m2 = round2(models.iter().flat_map(|m| m.rooms.iter()).map(|r| r.area_m2).sum());
    let total_perimeter_m = round2(models.iter().flat_map(|m| m.rooms.iter()).map(|r| r.perimeter_m).sum());
    Ok(AutoRlvResult { floor_z, ceiling_z, slabs, storeys: models, total_usable_area_m2, total_perimeter_m, stairs: all_stairs })
}

/// Reconstruct a 3D structural model (`Auto3dBuilding`: the wall planes and the slab faces) from
/// the same model `AUTORLV` draws, without furniture and without a guessed roof.
///
/// A wall is extruded from its axis between the base of its storey and the storey's top. A slab
/// face sits where a person sees it from the room — the walking surface for the floor, the
/// underside for everything above it — and carries the slab's measured or named thickness.
///
/// # Errors
///
/// The same refusals as [`extract_auto_rlv`], because this is that model read in 3D.
pub fn extract_auto_3d_building(points: &[[f64; 3]], opts: &BimOptions) -> Result<Auto3dBuilding, BimError> {
    let rlv = extract_auto_rlv(points, opts)?;
    let walls_3d: Vec<Wall3d> = rlv
        .storeys
        .iter()
        .flat_map(|storey| storey.walls.iter())
        .map(|w| Wall3d {
            base_p1: [w.from[0], w.from[1], w.base_z],
            base_p2: [w.to[0], w.to[1], w.base_z],
            top_z: w.base_z + w.height,
            thickness: w.thickness,
        })
        .collect();
    let last = rlv.slabs.len().saturating_sub(1);
    let slabs: Vec<Slab3d> = rlv
        .slabs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let kind = if i == 0 {
                "floor_slab"
            } else if i == last {
                "ceiling_slab"
            } else {
                "slab"
            };
            let z = if i == 0 { s.top_z } else { s.base_z };
            let holes = s.holes.iter().map(|h| h.iter().map(|p| [p[0], p[1], z]).collect()).collect();
            Slab3d { kind, vertices: s.polygon.iter().map(|p| [p[0], p[1], z]).collect(), holes, elevation_m: z, thickness_m: s.thickness }
        })
        .collect();
    let columns: Vec<Column> = rlv.storeys.iter().flat_map(|s| s.columns.iter().cloned()).collect();
    Ok(Auto3dBuilding { floor_z: rlv.floor_z, ceiling_z: rlv.ceiling_z, walls_3d, slabs, columns })
}

/// The area centroid of a closed ring, in world metres.
///
/// A degenerate ring (zero area, for instance two points repeated) falls back to the mean of its
/// vertices, so a label always has a place to sit; an empty ring is the origin, which the caller
/// never draws.
fn ring_centroid(ring: &[[f64; 2]]) -> [f64; 2] {
    let n = ring.len();
    let mut twice_area = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else {
            continue;
        };
        let cross = a[0] * b[1] - b[0] * a[1];
        twice_area += cross;
        cx += (a[0] + b[0]) * cross;
        cy += (a[1] + b[1]) * cross;
    }
    if twice_area.abs() > f64::EPSILON {
        return [cx / (3.0 * twice_area), cy / (3.0 * twice_area)];
    }
    if n == 0 {
        return [0.0, 0.0];
    }
    let count = n as f64;
    [ring.iter().map(|p| p[0]).sum::<f64>() / count, ring.iter().map(|p| p[1]).sum::<f64>() / count]
}

fn round2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    // `f64::sum` starts at -0.0, so an empty model formats its total as "-0.00 m²"; a total is a
    // magnitude, and zero is zero whichever sign the addend carried.
    if r == 0.0 { 0.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interior_furniture_fixture() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        // Floor slab at Z=0 and ceiling slab at Z=2.70
        for i in 0..20 {
            for j in 0..20 {
                let x = f64::from(i) * 0.25;
                let y = f64::from(j) * 0.25;
                pts.push([x, y, 0.0]);
                pts.push([x, y, 2.70]);
            }
        }
        // Continuous wall along Y=0 spanning Z=0..2.70
        for i in 0..20 {
            for k in 1..27 {
                pts.push([f64::from(i) * 0.25, 0.0, f64::from(k) * 0.10]);
            }
        }
        // A low sofa / table in the middle of the room (Z=0.45..0.85)
        for i in 6..12 {
            for j in 6..10 {
                pts.push([f64::from(i) * 0.25, f64::from(j) * 0.25, 0.65]);
            }
        }
        pts
    }

    #[test]
    fn a_total_with_no_addends_is_positive_zero() {
        // `f64::sum` starts at -0.0: an empty model reported "Su total = -0.00 m²", and the
        // reply's `totalUsableAreaM2` serialised as -0.0. A magnitude cannot be negative zero.
        let sum: f64 = std::iter::empty::<f64>().sum();
        assert!(sum.is_sign_negative(), "the reason the rounding has to normalise: {sum}");
        assert!(round2(sum).is_sign_positive(), "an empty total formats as 0.00, not -0.00");
        assert!(round2(-0.001).is_sign_positive(), "and anything that rounds to zero is still zero");
        assert_eq!(round2(1.234), 1.23, "non-zero totals keep their value");
    }

    #[test]
    fn interior_furniture_is_filtered_while_walls_and_slabs_survive() {
        let pts = interior_furniture_fixture();
        let res = filter_interior_furniture(&pts, 0.20, 0.15, 0.60);
        assert!(res.furniture_points >= 20, "the sofa in the middle is removed: {res:?}");
        assert!(res.structural_points > 800, "the slabs and walls are kept: {res:?}");
    }

    /// A 6 x 4 m room split by a partition at x = 3: 0.20 m walls, floor at z = 0, ceiling at
    /// z = 2.80, scanned from inside. See the engine test of the same shape for why the points are
    /// where they are: the floor and ceiling planes are the histogram's tallest bins, the four
    /// outer faces are single-sided, and the partition shows both of its faces.
    fn room_scan() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
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
        let levels = [0.1, 0.5, 0.9, 1.3, 1.7, 2.1, 2.45, 2.55, 2.65];
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
        pts
    }

    #[test]
    fn the_model_is_the_rooms_the_scan_has_and_not_a_bounding_box() {
        let rlv = extract_auto_rlv(&room_scan(), &BimOptions::default()).expect("the room has a model");
        assert_eq!(rlv.slabs.len(), 2, "the floor and the ceiling");
        assert_eq!(rlv.storeys.len(), 1, "one void between them");
        assert_eq!(rlv.walls(), 5, "four outer faces, synthesised, and one measured partition");
        assert_eq!(rlv.zones(), 2, "the partition splits the room");
        assert_eq!(rlv.openings(), 0, "no openings in solid room scan");
        assert_eq!(rlv.rooms(), 2);
        assert!((rlv.total_usable_area_m2 - 19.44).abs() <= 0.2, "two 2.7 x 3.6 rooms, got {}", rlv.total_usable_area_m2);
        let codes: Vec<&str> = rlv.storeys[0].rooms.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, ["E01", "E02"]);
        for room in &rlv.storeys[0].rooms {
            assert!((room.clear_height_m - 2.6).abs() < 1e-9, "the storey's margined height, got {}", room.clear_height_m);
            assert!(room.boundary.len() >= 3, "a room has an outline");
        }
    }

    #[test]
    fn auto3d_uses_the_same_model_and_says_no_roof_it_did_not_see() {
        let bld = extract_auto_3d_building(&room_scan(), &BimOptions::default()).expect("the room has a model");
        assert_eq!(bld.walls_3d.len(), 5);
        assert_eq!(bld.slabs.len(), 2, "the floor and the ceiling, and nothing invented above them");
        assert_eq!(bld.slabs.iter().map(|s| s.kind).collect::<Vec<_>>(), ["floor_slab", "ceiling_slab"]);
        assert!(bld.slabs.iter().all(|s| !s.vertices.is_empty()), "each slab is drawn from its own plan");
    }

    #[test]
    fn a_cloud_with_no_surfaces_is_an_error_and_not_an_empty_frame() {
        let r = extract_auto_rlv(&[], &BimOptions::default());
        assert!(matches!(r, Err(BimError::Empty)), "{r:?}");
    }
}
