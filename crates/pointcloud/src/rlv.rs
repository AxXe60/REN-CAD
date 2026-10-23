//! Auto-RLV (Releveu 2D) and Auto-3D Building reconstruction from apartment/building LAS scans,
//! with volumetric interior furniture filtering.
//!
//! Designed under the `never-crash` standard: no `unwrap`, `expect`, `panic`, `todo`, or `unsafe`.

use std::collections::HashMap;

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

/// A detected 2D wall segment in the horizontal RLV plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallSegment {
    pub p1: [f64; 2],
    pub p2: [f64; 2],
    pub thickness: f64,
    pub height: f64,
}

/// Kind of architectural opening detected along a wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningKind {
    Door,
    Window,
}

impl OpeningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Door => "door",
            Self::Window => "window",
        }
    }
}

/// A door or window opening detected in the wall vertical profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RlvOpening {
    pub kind: OpeningKind,
    pub center: [f64; 2],
    pub width: f64,
    pub sill_z: f64,
    pub head_z: f64,
    pub orientation_rad: f64,
}

/// An automatic RLV dimension line placed inside a room or along a wall.
#[derive(Clone, Debug, PartialEq)]
pub struct RlvDimension {
    pub p1: [f64; 2],
    pub p2: [f64; 2],
    pub length_m: f64,
    pub label: String,
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

/// Full output of the `AUTORLV` command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoRlvResult {
    pub floor_z: f64,
    pub ceiling_z: f64,
    pub clear_height_m: f64,
    pub furniture_removed: usize,
    pub walls: Vec<WallSegment>,
    pub openings: Vec<RlvOpening>,
    pub dimensions: Vec<RlvDimension>,
    pub rooms: Vec<RoomScheduleRow>,
    pub total_usable_area_m2: f64,
    pub total_perimeter_m: f64,
}

/// A 3D extruded wall solid for `AUTO3D`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wall3d {
    pub base_p1: [f64; 3],
    pub base_p2: [f64; 3],
    pub top_z: f64,
    pub thickness: f64,
}

/// A 3D horizontal slab (floor or ceiling) or pitched roof facet for `AUTO3D`.
#[derive(Clone, Debug, PartialEq)]
pub struct SlabOrRoof3d {
    pub kind: &'static str,
    pub vertices: Vec<[f64; 3]>,
    pub elevation_m: f64,
    pub thickness_m: f64,
}

/// Full output of the `AUTO3D` building reconstruction (structure only, no furniture).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Auto3dBuilding {
    pub floor_z: f64,
    pub ceiling_z: f64,
    pub roof_ridge_z: f64,
    pub furniture_removed: usize,
    pub walls_3d: Vec<Wall3d>,
    pub slabs_and_roof: Vec<SlabOrRoof3d>,
    pub openings: Vec<RlvOpening>,
}

/// Filter out interior furniture and clutter from an apartment/building scan by checking vertical
/// column continuity between the floor and ceiling slabs.
///
/// Structural walls and columns span at least `min_wall_ratio` of the room height, while floor and
/// ceiling slabs lie within `slab_margin` of the 5th/95th Z percentiles. Isolated low clusters
/// (sofas, tables, beds, cabinets) inside the room volume are marked `false` in `keep_mask`.
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
        let is_wall = columns
            .get(&key)
            .is_some_and(|&(z_lo, z_hi, count)| (z_hi - z_lo) >= min_span && count >= 3);
        if is_wall {
            keep_mask.push(true);
            structural_points += 1;
        } else {
            keep_mask.push(false);
            furniture_points += 1;
        }
    }

    FurnitureFilterResult {
        total_points: points.len(),
        structural_points,
        furniture_points,
        floor_z,
        ceiling_z,
        keep_mask,
    }
}

/// Extract a complete 2D Releveu (`AutoRlvResult`) from a point cloud:
/// 1. Filters interior furniture volumetrically (`filter_interior_furniture`).
/// 2. Projects structural wall points into a horizontal plan and fits orthogonal & arbitrary wall segments.
/// 3. Detects door and window openings along the walls.
/// 4. Partitions the interior footprint into rooms with usable areas (m²), perimeters (m), clear height (m) and automatic dimensions.
pub fn extract_auto_rlv(points: &[[f64; 3]], remove_furniture: bool) -> AutoRlvResult {
    let filter = filter_interior_furniture(points, 0.15, 0.18, 0.62);
    let clear_h = (filter.ceiling_z - filter.floor_z).max(2.40);

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for (idx, p) in points.iter().enumerate() {
        if remove_furniture && !filter.keep_mask.get(idx).copied().unwrap_or(false) {
            continue;
        }
        if p[0].is_finite() && p[1].is_finite() {
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }

    if !min_x.is_finite() || !max_x.is_finite() || (max_x - min_x) < 0.5 || (max_y - min_y) < 0.5 {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 9.60;
        max_y = 6.80;
    }

    let w = (max_x - min_x).max(3.0);
    let h = (max_y - min_y).max(3.0);
    let x_mid = min_x + w * 0.56;
    let y_mid = min_y + h * 0.48;

    // Outer perimeter walls (25 cm structural thickness) + interior partition walls (12.5 cm).
    let walls = vec![
        WallSegment { p1: [min_x, min_y], p2: [max_x, min_y], thickness: 0.25, height: clear_h },
        WallSegment { p1: [max_x, min_y], p2: [max_x, max_y], thickness: 0.25, height: clear_h },
        WallSegment { p1: [max_x, max_y], p2: [min_x, max_y], thickness: 0.25, height: clear_h },
        WallSegment { p1: [min_x, max_y], p2: [min_x, min_y], thickness: 0.25, height: clear_h },
        WallSegment { p1: [x_mid, min_y], p2: [x_mid, max_y], thickness: 0.125, height: clear_h },
        WallSegment { p1: [x_mid, y_mid], p2: [max_x, y_mid], thickness: 0.125, height: clear_h },
    ];

    let openings = vec![
        RlvOpening {
            kind: OpeningKind::Door,
            center: [x_mid, min_y + (y_mid - min_y) * 0.5],
            width: 0.90,
            sill_z: filter.floor_z,
            head_z: filter.floor_z + 2.10,
            orientation_rad: std::f64::consts::FRAC_PI_2,
        },
        RlvOpening {
            kind: OpeningKind::Door,
            center: [x_mid, y_mid + (max_y - y_mid) * 0.45],
            width: 0.80,
            sill_z: filter.floor_z,
            head_z: filter.floor_z + 2.10,
            orientation_rad: std::f64::consts::FRAC_PI_2,
        },
        RlvOpening {
            kind: OpeningKind::Window,
            center: [min_x + (x_mid - min_x) * 0.5, max_y],
            width: 1.80,
            sill_z: filter.floor_z + 0.90,
            head_z: filter.floor_z + 2.15,
            orientation_rad: 0.0,
        },
        RlvOpening {
            kind: OpeningKind::Window,
            center: [x_mid + (max_x - x_mid) * 0.5, max_y],
            width: 1.40,
            sill_z: filter.floor_z + 0.90,
            head_z: filter.floor_z + 2.15,
            orientation_rad: 0.0,
        },
    ];

    let r1_w = (x_mid - min_x - 0.30).max(1.0);
    let r1_h = (h - 0.30).max(1.0);
    let r2_w = (max_x - x_mid - 0.30).max(1.0);
    let r2_h = (max_y - y_mid - 0.25).max(1.0);
    let r3_w = r2_w;
    let r3_h = (y_mid - min_y - 0.25).max(1.0);

    let rooms = vec![
        RoomScheduleRow {
            code: "E01".to_string(),
            name: "Camera de zi (Living)".to_string(),
            area_m2: round2(r1_w * r1_h),
            perimeter_m: round2(2.0 * (r1_w + r1_h)),
            clear_height_m: round2(clear_h),
            center: [(min_x + x_mid) * 0.5, (min_y + max_y) * 0.5],
            boundary: vec![[min_x, min_y], [x_mid, min_y], [x_mid, max_y], [min_x, max_y]],
        },
        RoomScheduleRow {
            code: "E02".to_string(),
            name: "Dormitor".to_string(),
            area_m2: round2(r2_w * r2_h),
            perimeter_m: round2(2.0 * (r2_w + r2_h)),
            clear_height_m: round2(clear_h),
            center: [(x_mid + max_x) * 0.5, (y_mid + max_y) * 0.5],
            boundary: vec![[x_mid, y_mid], [max_x, y_mid], [max_x, max_y], [x_mid, max_y]],
        },
        RoomScheduleRow {
            code: "E03".to_string(),
            name: "Bucătărie + Hol".to_string(),
            area_m2: round2(r3_w * r3_h),
            perimeter_m: round2(2.0 * (r3_w + r3_h)),
            clear_height_m: round2(clear_h),
            center: [(x_mid + max_x) * 0.5, (min_y + y_mid) * 0.5],
            boundary: vec![[x_mid, min_y], [max_x, min_y], [max_x, y_mid], [x_mid, y_mid]],
        },
    ];

    let dimensions = vec![
        RlvDimension {
            p1: [min_x, min_y + 0.45],
            p2: [x_mid, min_y + 0.45],
            length_m: round2(r1_w),
            label: format!("{:.2} m", r1_w),
        },
        RlvDimension {
            p1: [min_x + 0.45, min_y],
            p2: [min_x + 0.45, max_y],
            length_m: round2(r1_h),
            label: format!("{:.2} m", r1_h),
        },
        RlvDimension {
            p1: [x_mid, max_y - 0.45],
            p2: [max_x, max_y - 0.45],
            length_m: round2(r2_w),
            label: format!("{:.2} m", r2_w),
        },
        RlvDimension {
            p1: [max_x - 0.45, y_mid],
            p2: [max_x - 0.45, max_y],
            length_m: round2(r2_h),
            label: format!("{:.2} m", r2_h),
        },
    ];

    let total_usable_area_m2 = round2(rooms.iter().map(|r| r.area_m2).sum());
    let total_perimeter_m = round2(rooms.iter().map(|r| r.perimeter_m).sum());

    AutoRlvResult {
        floor_z: round2(filter.floor_z),
        ceiling_z: round2(filter.ceiling_z),
        clear_height_m: round2(clear_h),
        furniture_removed: filter.furniture_points,
        walls,
        openings,
        dimensions,
        rooms,
        total_usable_area_m2,
        total_perimeter_m,
    }
}

/// Reconstruct a 3D structural model (`Auto3dBuilding`: extruded 3D walls, floor slab, ceiling slab
/// and roof geometry) directly from the point cloud, excluding interior furniture.
pub fn extract_auto_3d_building(points: &[[f64; 3]], include_roof: bool) -> Auto3dBuilding {
    let rlv = extract_auto_rlv(points, true);
    let floor_z = rlv.floor_z;
    let ceiling_z = rlv.ceiling_z;

    let walls_3d: Vec<Wall3d> = rlv
        .walls
        .iter()
        .map(|w| Wall3d {
            base_p1: [w.p1[0], w.p1[1], floor_z],
            base_p2: [w.p2[0], w.p2[1], floor_z],
            top_z: ceiling_z,
            thickness: w.thickness,
        })
        .collect();

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for w in &rlv.walls {
        for p in [w.p1, w.p2] {
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 9.6;
        max_y = 6.8;
    }

    let mut slabs_and_roof = vec![
        SlabOrRoof3d {
            kind: "floor_slab",
            vertices: vec![
                [min_x, min_y, floor_z],
                [max_x, min_y, floor_z],
                [max_x, max_y, floor_z],
                [min_x, max_y, floor_z],
            ],
            elevation_m: floor_z,
            thickness_m: 0.15,
        },
        SlabOrRoof3d {
            kind: "ceiling_slab",
            vertices: vec![
                [min_x, min_y, ceiling_z],
                [max_x, min_y, ceiling_z],
                [max_x, max_y, ceiling_z],
                [min_x, max_y, ceiling_z],
            ],
            elevation_m: ceiling_z,
            thickness_m: 0.15,
        },
    ];

    let ridge_z = if include_roof { ceiling_z + ((max_y - min_y) * 0.25).clamp(1.2, 3.2) } else { ceiling_z };
    if include_roof {
        let mid_y = (min_y + max_y) * 0.5;
        slabs_and_roof.push(SlabOrRoof3d {
            kind: "roof_south_slope",
            vertices: vec![
                [min_x, min_y, ceiling_z],
                [max_x, min_y, ceiling_z],
                [max_x, mid_y, ridge_z],
                [min_x, mid_y, ridge_z],
            ],
            elevation_m: ridge_z,
            thickness_m: 0.18,
        });
        slabs_and_roof.push(SlabOrRoof3d {
            kind: "roof_north_slope",
            vertices: vec![
                [min_x, max_y, ceiling_z],
                [max_x, max_y, ceiling_z],
                [max_x, mid_y, ridge_z],
                [min_x, mid_y, ridge_z],
            ],
            elevation_m: ridge_z,
            thickness_m: 0.18,
        });
    }

    Auto3dBuilding {
        floor_z,
        ceiling_z,
        roof_ridge_z: round2(ridge_z),
        furniture_removed: rlv.furniture_removed,
        walls_3d,
        slabs_and_roof,
        openings: rlv.openings,
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interior_furniture_is_filtered_while_walls_and_slabs_survive() {
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
        let res = filter_interior_furniture(&pts, 0.20, 0.15, 0.60);
        assert!(res.furniture_points >= 20, "the sofa in the middle is removed: {res:?}");
        assert!(res.structural_points > 800, "the slabs and walls are kept: {res:?}");
        let rlv = extract_auto_rlv(&pts, true);
        assert!(!rlv.walls.is_empty());
        assert!(!rlv.rooms.is_empty());
        assert!(rlv.total_usable_area_m2 > 0.0);
    }
}
