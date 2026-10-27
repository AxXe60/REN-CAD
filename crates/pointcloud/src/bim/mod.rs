//! The scan-to-BIM pipeline: cloud → rasters → slabs, storeys, walls and room polygons.
//!
//! Every stage here takes plain arrays and owned bitmaps. Nothing in `bim` opens a file, reads
//! the environment or talks to the network, so the crate keeps compiling for wasm; the reading of
//! the cloud belongs to the caller, which hands the stages the points it can afford to hold.
//!
//! The numbers come from the design doc §5: Table 6 of the paper is adopted verbatim, and where
//! the paper is silent or contradicts itself the choice is ours and is labelled as ours.

pub mod columns;
pub mod deviate;
pub mod dimensions;
pub mod ifc;
pub mod openings;
pub mod raster;
pub mod slabs;
pub mod storeys;
pub mod walls;
pub mod zones;

pub use columns::{Column, ColumnShape, detect_columns};
pub use deviate::{DeviationReport, FurnitureCluster, measure as measure_deviation};
pub use dimensions::{AutoDimension, generate_dimensions};
pub use ifc::{Building as IfcBuilding, BuildingStorey as IfcStorey, IfcCheck, check as check_ifc, write as write_ifc};
pub use openings::{Opening, OpeningKind, band as opening_band, detect as detect_openings, detect_all as detect_all_openings};
pub use raster::{Contour, MAX_RASTER_CELLS, Raster, close, contours, mask};
pub use slabs::{MAX_Z_BINS, Slab};
pub use storeys::Storey;
pub use walls::Wall;
pub use zones::Zone;

/// The largest morphological close radius the pipeline will run.
///
/// A close costs `O(w·h·k)` per pass, so an unbounded radius is an unbounded loop over a grid
/// that is already at its cell budget; `64` cells is far past any wall a scanner resolves.
pub const MAX_CLOSE_RADIUS: usize = 64;

/// The most walls one opening raster is allowed to reference through its rank. A rank is a
/// position in a sorted list of cells, and no wall a scanner resolves has this many dense cells.
pub const MAX_OPENING_RANK: usize = 4_096;

/// The most furniture clusters an option may ask a report to hold.
pub const MAX_FURNITURE_CLUSTERS: usize = 65_536;

/// The most fine points one wall band may keep before the stream coarsens it.
pub const MAX_BAND_POINTS: usize = 10_000_000;

/// The knobs of the pipeline. The values marked *(paper)* are Table 6 of arXiv:2503.11498; the
/// rest are ours, because Table 5 publishes no defaults and the source contradicts itself on the
/// raster resolution (see the design doc §5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BimOptions {
    /// *(paper)* bin size of the vertical histogram that finds the slabs.
    pub z_step: f64,
    /// *(paper)* a slab candidate is a bin at least this times the tallest bin.
    pub slab_ratio: f64,
    /// *(paper)* the horizontal slice, as a fraction of the storey height.
    pub slice_from: f64,
    /// *(paper)* the top of the same window, as a fraction of the storey height.
    pub slice_to: f64,
    /// *(paper)* a mask cell is set at this times the tallest bin.
    pub threshold: f64,
    /// *(paper)* the morphological close radius, in cells: Table 6's 5×5 kernel is radius 2.
    pub close_radius: usize,
    /// *(paper)* the Douglas-Peucker tolerance for wall polylines, in metres.
    pub epsilon: f64,
    /// *(paper)* how far apart two segments may be to join, in degrees.
    pub angle_tolerance_deg: f64,
    /// *(paper)* the overlap margin when the cloud is split into storeys.
    pub safety_margin: f64,
    /// *(ours)* the metric cell size of the rasters: the paper's `grid coefficient` of 5 px/mm
    /// contradicts its own figure, so the grid is named in metres instead. The source is silent
    /// here, so this number is ours.
    pub cell: f64,
    /// *(ours)* the thickness given to a slab whose second face was never scanned; the paper
    /// calls this `bfs`/`tfs` thickness and never says what either is. The source is silent here,
    /// so this number is ours.
    pub single_sided_slab_thickness: f64,
    /// *(ours)* the shortest wall axis kept after pairing; Table 5 publishes no default, so this
    /// number is ours.
    pub min_wall_length: f64,
    /// *(ours)* the thinnest wall accepted, in metres; the source is silent here, so this number
    /// is ours.
    pub min_wall_thickness: f64,
    /// *(ours)* the thickest wall accepted, in metres; the source is silent here, so this number
    /// is ours.
    pub max_wall_thickness: f64,
    /// *(ours)* how far a wall's end may miss another wall's line and still be snapped onto it
    /// when the room polygons are closed, in metres. The paper's own corner failure is 35 cm —
    /// its Kladno scan merged two rooms — while a door is at least 0.8 m wide, so 0.40 m closes a
    /// bad corner without bridging an opening. The source gives no value, so this number is ours.
    pub zone_gap: f64,
    /// *(ours)* the smallest room, in square metres, that becomes a zone; the source is silent
    /// here, so this number is ours.
    pub min_zone_area: f64,
    /// *(ours)* the thickness synthesised for a perimeter wall when the scan has no exterior
    /// face, in metres; the source is silent here, so this number is ours.
    pub exterior_wall_thickness: f64,
    /// True when the scan covers the exterior face of the perimeter walls. When it is false the
    /// scan is interior-only and the outer face of a perimeter wall is synthesised at
    /// `exterior_wall_thickness`. The source is silent here, so the default — interior-only, the
    /// case the paper's own scans are — is ours.
    pub exterior_scan: bool,
    // --- Plan B: openings ---
    /// *(ours)* the metric cell of a wall's elevation raster. The paper gives a rank and no cell;
    /// 2 cm resolves the 25 mm acceptance bound and is finer than the plan lattice.
    pub opening_cell: f64,
    /// *(ours)* a cell is open when its density is under this fraction of the rank-th densest
    /// cell. The paper gives the rank (`max10`) and no percentage; this choice is ours.
    pub opening_threshold: f64,
    /// *(paper)* the rank of the reference cell, Table 6's `max10`: nine density spikes cannot
    /// move it.
    pub opening_rank: usize,
    /// *(ours)* the narrowest opening accepted, in metres; the paper names the test and no number.
    pub min_opening_width: f64,
    /// *(ours)* the widest opening accepted, in metres.
    pub max_opening_width: f64,
    /// *(ours)* the shortest door accepted, in metres.
    pub door_min_height: f64,
    /// *(ours)* the highest sill a door may have, in metres above the wall's base.
    pub door_max_sill: f64,
    /// *(ours)* the lowest sill a window may have, in metres above the wall's base.
    pub window_min_sill: f64,
    /// *(ours)* how far off a wall's faces a point may lie and still count as that wall's, in
    /// metres.
    pub opening_band: f64,
    /// *(ours)* the cube the fine wall bands are thinned to before the opening raster, in metres;
    /// the spec asks for ~5 mm detail.
    pub opening_decimate: f64,
    /// *(ours)* the most points one wall's fine band may hold; the stream coarsens the cell past
    /// it.
    pub max_wall_band_points: usize,
    // --- Plan B: deviation ---
    /// *(ours)* the distance above which a point is not on the model, in metres. The spec names
    /// 50 mm; the paper gives no number.
    pub deviation_threshold: f64,
    /// *(ours)* the 3D cell of the furniture clustering, in metres.
    pub deviation_cell: f64,
    /// *(ours)* how many off-model points a cluster needs before it is reported as furniture.
    pub min_furniture_points: usize,
    /// *(ours)* the most furniture clusters one report may hold; past it the stage refuses.
    pub max_furniture_clusters: usize,
    // --- Plan B: columns ---
    /// *(ours)* the smallest structural column cross-section dimension accepted, in metres.
    pub min_column_size: f64,
    /// *(ours)* the largest structural column cross-section dimension accepted, in metres.
    pub max_column_size: f64,
    /// *(ours)* the maximum cross-section aspect ratio (width / depth) accepted for columns.
    pub max_column_aspect_ratio: f64,
}

impl Default for BimOptions {
    fn default() -> Self {
        BimOptions {
            z_step: 0.05,
            slab_ratio: 0.5,
            slice_from: 0.9,
            slice_to: 1.0,
            threshold: 0.01,
            close_radius: 2,
            epsilon: 0.02,
            angle_tolerance_deg: 3.0,
            safety_margin: 0.1,
            cell: 0.05,
            single_sided_slab_thickness: 0.20,
            min_wall_length: 0.40,
            min_wall_thickness: 0.06,
            max_wall_thickness: 0.60,
            zone_gap: 0.40,
            min_zone_area: 0.5,
            exterior_wall_thickness: 0.30,
            exterior_scan: false,
            opening_cell: 0.02,
            opening_threshold: 0.5,
            opening_rank: 10,
            min_opening_width: 0.60,
            max_opening_width: 1.60,
            door_min_height: 1.90,
            door_max_sill: 0.30,
            window_min_sill: 0.50,
            opening_band: 0.05,
            opening_decimate: 0.005,
            max_wall_band_points: 200_000,
            deviation_threshold: 0.05,
            deviation_cell: 0.10,
            min_furniture_points: 50,
            max_furniture_clusters: 4_096,
            min_column_size: 0.15,
            max_column_size: 1.00,
            max_column_aspect_ratio: 2.50,
        }
    }
}

impl BimOptions {
    /// The options with every key `params` names applied over the defaults.
    ///
    /// A key that is absent keeps its default; a key whose value cannot be used is an error
    /// rather than a guess. Hostile JSON — a string where a number belongs, a negative cell, a
    /// cell of zero, `null`, an array, a non-finite number — never panics: it returns
    /// [`BimError::InvalidCell`] or [`BimError::TooLarge`], or leaves the default in place when
    /// the value is a bool of the wrong type. Ordering between keys (a `slice_from` above
    /// `slice_to`) is the stages' business, not this function's.
    pub fn from_params(params: &serde_json::Value) -> Result<Self, BimError> {
        let mut o = BimOptions::default();
        // Lengths that have to be above zero.
        if let Some(v) = positive(params, "z_step")? {
            o.z_step = v;
        }
        if let Some(v) = positive(params, "cell")? {
            o.cell = v;
        }
        if let Some(v) = positive(params, "single_sided_slab_thickness")? {
            o.single_sided_slab_thickness = v;
        }
        if let Some(v) = positive(params, "exterior_wall_thickness")? {
            o.exterior_wall_thickness = v;
        }
        // The paper's ratios, which are fractions of the tallest bin or of the storey height.
        if let Some(v) = unit_ratio(params, "slab_ratio")? {
            o.slab_ratio = v;
        }
        if let Some(v) = unit_ratio(params, "threshold")? {
            o.threshold = v;
        }
        if let Some(v) = unit_ratio(params, "slice_from")? {
            o.slice_from = v;
        }
        if let Some(v) = unit_ratio(params, "slice_to")? {
            o.slice_to = v;
        }
        // Tolerances and minimum sizes, where zero legitimately means "no filter".
        if let Some(v) = non_negative(params, "epsilon")? {
            o.epsilon = v;
        }
        if let Some(v) = non_negative(params, "angle_tolerance_deg")? {
            o.angle_tolerance_deg = v;
        }
        if let Some(v) = non_negative(params, "safety_margin")? {
            o.safety_margin = v;
        }
        if let Some(v) = non_negative(params, "min_wall_length")? {
            o.min_wall_length = v;
        }
        if let Some(v) = non_negative(params, "min_wall_thickness")? {
            o.min_wall_thickness = v;
        }
        if let Some(v) = non_negative(params, "max_wall_thickness")? {
            o.max_wall_thickness = v;
        }
        if let Some(v) = non_negative(params, "zone_gap")? {
            o.zone_gap = v;
        }
        if let Some(v) = non_negative(params, "min_zone_area")? {
            o.min_zone_area = v;
        }
        if let Some(v) = integer(params, "close_radius", "close radius")? {
            o.close_radius = v;
        }
        if let Some(v) = params.get("exterior_scan").and_then(serde_json::Value::as_bool) {
            o.exterior_scan = v;
        }
        // Plan B: the openings' elevation raster and its tests.
        if let Some(v) = positive(params, "opening_cell")? {
            o.opening_cell = v;
        }
        if let Some(v) = positive(params, "opening_decimate")? {
            o.opening_decimate = v;
        }
        if let Some(v) = positive(params, "min_opening_width")? {
            o.min_opening_width = v;
        }
        if let Some(v) = positive(params, "max_opening_width")? {
            o.max_opening_width = v;
        }
        if let Some(v) = positive(params, "door_min_height")? {
            o.door_min_height = v;
        }
        if let Some(v) = positive(params, "opening_band")? {
            o.opening_band = v;
        }
        if let Some(v) = unit_ratio(params, "opening_threshold")? {
            o.opening_threshold = v;
        }
        if let Some(v) = non_negative(params, "door_max_sill")? {
            o.door_max_sill = v;
        }
        if let Some(v) = non_negative(params, "window_min_sill")? {
            o.window_min_sill = v;
        }
        if let Some(v) = count_up_to(params, "opening_rank", "opening rank", MAX_OPENING_RANK)? {
            o.opening_rank = v;
        }
        if let Some(v) = count_up_to(params, "max_wall_band_points", "wall band points", MAX_BAND_POINTS)? {
            o.max_wall_band_points = v;
        }
        // Plan B: the deviation report and its furniture clusters.
        if let Some(v) = positive(params, "deviation_cell")? {
            o.deviation_cell = v;
        }
        if let Some(v) = non_negative(params, "deviation_threshold")? {
            o.deviation_threshold = v;
        }
        if let Some(v) = count_up_to(params, "min_furniture_points", "furniture cluster minimum", MAX_BAND_POINTS)? {
            o.min_furniture_points = v;
        }
        if let Some(v) = count_up_to(params, "max_furniture_clusters", "furniture cluster budget", MAX_FURNITURE_CLUSTERS)? {
            o.max_furniture_clusters = v;
        }
        // Plan B: columns.
        if let Some(v) = positive(params, "min_column_size")? {
            o.min_column_size = v;
        }
        if let Some(v) = positive(params, "max_column_size")? {
            o.max_column_size = v;
        }
        if let Some(v) = positive(params, "max_column_aspect_ratio")? {
            o.max_column_aspect_ratio = v;
        }
        Ok(o)
    }
}

/// A number that must be finite and above zero, or the parameter is refused.
fn positive(params: &serde_json::Value, key: &str) -> Result<Option<f64>, BimError> {
    match number(params, key)? {
        Some(v) if v > 0.0 => Ok(Some(v)),
        Some(_) => Err(BimError::InvalidCell),
        None => Ok(None),
    }
}

/// A number that must be a finite ratio in `(0, 1]`: the paper's knobs are all fractions of the
/// tallest bin or of the storey height.
fn unit_ratio(params: &serde_json::Value, key: &str) -> Result<Option<f64>, BimError> {
    match number(params, key)? {
        Some(v) if v > 0.0 && v <= 1.0 => Ok(Some(v)),
        Some(_) => Err(BimError::InvalidCell),
        None => Ok(None),
    }
}

/// A number that must be finite and not negative; zero means "no filter" and is allowed.
fn non_negative(params: &serde_json::Value, key: &str) -> Result<Option<f64>, BimError> {
    match number(params, key)? {
        Some(v) if v >= 0.0 => Ok(Some(v)),
        Some(_) => Err(BimError::InvalidCell),
        None => Ok(None),
    }
}

/// A whole number within [`MAX_CLOSE_RADIUS`]; too big is refused, not clamped. `what` names the
/// budget in the error, which is a `&'static str` so an error can be held past the call.
///
/// The value comes through [`number`], so a float that holds a whole number (`2.0`, the form a
/// JS-authored or metre-computing caller naturally writes) is applied rather than ignored, and a
/// string, a bool or a fraction is refused rather than silently leaving the default. Every
/// unusable number is `InvalidCell`, the same as the sibling readers answer: the variant is the
/// crate's word for "a parameter that cannot be used", not only for a cell size.
fn integer(params: &serde_json::Value, key: &str, what: &'static str) -> Result<Option<usize>, BimError> {
    integer_up_to(params, key, what, MAX_CLOSE_RADIUS)
}

/// A whole number in `1..=max`: a count, a rank or a budget of zero is not something a caller can
/// mean, and is refused rather than applied.
fn count_up_to(params: &serde_json::Value, key: &str, what: &'static str, max: usize) -> Result<Option<usize>, BimError> {
    match integer_up_to(params, key, what, max)? {
        Some(0) => Err(BimError::InvalidCell),
        other => Ok(other),
    }
}

/// The shared body of every whole-number reader, with its own ceiling. See [`integer`] for why the
/// value goes through [`number`] and why every unusable number is `InvalidCell`.
fn integer_up_to(params: &serde_json::Value, key: &str, what: &'static str, max: usize) -> Result<Option<usize>, BimError> {
    let Some(v) = number(params, key)? else {
        return Ok(None);
    };
    if v < 0.0 || v.fract() != 0.0 {
        return Err(BimError::InvalidCell);
    }
    if v > max as f64 {
        return Err(BimError::TooLarge { what, limit: max });
    }
    Ok(Some(v as usize))
}

/// The number behind a key: `Ok(None)` when the key is absent or explicitly `null` (there is
/// nothing to read), `Err` when a value is there but is not a usable finite number — a string, a
/// bool, an array, a number that is not finite — because a wrong value the caller can see beats a
/// default that quietly won. Note that `json!` renders an infinite literal as `Value::Null`, so
/// such a value arrives through the `null` arm, not this one. `as_f64` is the only float reader
/// here, so no cast of hostile text is involved.
fn number(params: &serde_json::Value, key: &str) -> Result<Option<f64>, BimError> {
    let Some(raw) = params.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    match raw.as_f64() {
        Some(v) if v.is_finite() => Ok(Some(v)),
        _ => Err(BimError::InvalidCell),
    }
}

/// What went wrong on the way from a cloud to a model.
#[derive(Debug, thiserror::Error)]
pub enum BimError {
    /// A cell size or a numeric parameter that cannot be used: not a number, zero where a
    /// positive length is needed, or a negative distance.
    #[error("a parameter or raster cell is not usable: it must be a finite, non-negative number")]
    InvalidCell,
    /// Something asked for more than the pipeline is willing to hold.
    #[error("{what} exceeds its budget of {limit}")]
    TooLarge {
        /// Which budget was hit.
        what: &'static str,
        /// The limit itself, so the caller can say what to ask for instead.
        limit: usize,
    },
    /// There is nothing to build from: no points, no cells, no slab face found.
    #[error("the scan holds nothing to build from in this band")]
    Empty,
    /// A stage that is designed but not written yet, so a caller can say so in words.
    #[error("{0} is not available yet")]
    NotAvailable(&'static str),
    /// A STEP file our own reader cannot close: a missing header, a duplicate id, a reference to
    /// an entity that is not there. The writer must never advertise a file that fails this check.
    #[error("the STEP file is not well formed: {0}")]
    Malformed(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_spec_section_5_numbers() {
        let o = BimOptions::default();
        // Table 6 of the paper, adopted verbatim.
        assert_eq!(o.z_step, 0.05);
        assert_eq!(o.slab_ratio, 0.5);
        assert_eq!(o.slice_from, 0.9);
        assert_eq!(o.slice_to, 1.0);
        assert_eq!(o.threshold, 0.01);
        assert_eq!(o.close_radius, 2, "a 5x5 close kernel is radius 2");
        assert_eq!(o.epsilon, 0.02);
        assert_eq!(o.angle_tolerance_deg, 3.0);
        assert_eq!(o.safety_margin, 0.1);
        // Ours, because the source publishes no default or contradicts itself.
        assert_eq!(o.cell, 0.05);
        assert_eq!(o.single_sided_slab_thickness, 0.20);
        assert_eq!((o.min_wall_length, o.min_wall_thickness, o.max_wall_thickness), (0.40, 0.06, 0.60));
        assert_eq!(o.zone_gap, 0.40);
        assert_eq!(o.min_zone_area, 0.5);
        assert_eq!(o.exterior_wall_thickness, 0.30);
        assert!(!o.exterior_scan);
        // Plan B: openings, deviation and their budgets.
        assert_eq!(o.opening_cell, 0.02);
        assert_eq!(o.opening_threshold, 0.5);
        assert_eq!(o.opening_rank, 10, "the paper's max10");
        assert_eq!((o.min_opening_width, o.max_opening_width), (0.60, 1.60));
        assert_eq!((o.door_min_height, o.door_max_sill, o.window_min_sill), (1.90, 0.30, 0.50));
        assert_eq!((o.opening_band, o.opening_decimate), (0.05, 0.005));
        assert_eq!(o.max_wall_band_points, 200_000);
        assert_eq!(o.deviation_threshold, 0.05);
        assert_eq!(o.deviation_cell, 0.10);
        assert_eq!((o.min_furniture_points, o.max_furniture_clusters), (50, 4_096));
        assert_eq!((o.min_column_size, o.max_column_size, o.max_column_aspect_ratio), (0.15, 1.00, 2.50));
    }

    #[test]
    fn from_params_reads_the_keys_it_knows_and_leaves_the_rest_alone() {
        let o = BimOptions::from_params(&serde_json::json!({})).expect("no params is the default");
        assert_eq!(o, BimOptions::default());
        let o = BimOptions::from_params(&serde_json::json!({
            "cell": 0.1, "z_step": 0.02, "close_radius": 3, "exterior_scan": true, "unknown_knob": 7
        }))
        .expect("a small params object parses");
        assert_eq!(o.cell, 0.1, "the named key wins");
        assert_eq!(o.z_step, 0.02);
        assert_eq!(o.close_radius, 3);
        assert!(o.exterior_scan);
        assert_eq!(o.threshold, BimOptions::default().threshold, "an absent key keeps its default");
    }

    #[test]
    fn a_whole_number_that_arrives_as_a_float_is_applied_not_ignored() {
        // `as_i64` reads an integer-typed JSON number and nothing else, so the natural
        // `json!({"close_radius": 2.0})` from a JS-authored or f64-computing caller was neither
        // applied nor refused: `Ok(None)` means "absent" and the default quietly won. A value
        // that is there must be applied or refused, never ignored (the rule `number` states).
        let o = BimOptions::from_params(&serde_json::json!({"close_radius": 4.0})).expect("4.0 is a whole number");
        assert_eq!(o.close_radius, 4, "a float holding a whole number is applied, not dropped");
        let o = BimOptions::from_params(&serde_json::json!({"close_radius": 0.0})).expect("0.0 is a whole number");
        assert_eq!(o.close_radius, 0, "and a radius of zero means the identity close, not the default");
        let o = BimOptions::from_params(&serde_json::json!({"close_radius": 2.0})).expect("the common form");
        assert_eq!(o.close_radius, 2, "2.0 is radius 2 (which is also the default, so the line above is the one that bites)");
        let o = BimOptions::from_params(&serde_json::json!({"close_radius": 3})).expect("the integer form still parses");
        assert_eq!(o.close_radius, 3);
        // What cannot be a radius is refused loudly rather than defaulted away.
        for refused in [
            serde_json::json!({"close_radius": 2.5}),
            serde_json::json!({"close_radius": "2"}),
            serde_json::json!({"close_radius": true}),
            serde_json::json!({"close_radius": -1}),
            serde_json::json!({"close_radius": -0.5}),
        ] {
            assert!(
                matches!(BimOptions::from_params(&refused), Err(BimError::InvalidCell)),
                "{refused} is not a radius and must not quietly become one"
            );
        }
        // The budget is still a refusal, not a clamp.
        assert!(
            matches!(
                BimOptions::from_params(&serde_json::json!({"close_radius": MAX_CLOSE_RADIUS + 1})),
                Err(BimError::TooLarge { limit, .. }) if limit == MAX_CLOSE_RADIUS
            ),
            "a radius past the budget says which budget it hit"
        );
    }

    #[test]
    fn hostile_params_are_refused_and_never_panic() {
        for hostile in [
            serde_json::json!(null),
            serde_json::json!("cell"),
            serde_json::json!([1.0, 2.0]),
            serde_json::json!({"cell": "wide"}),
            serde_json::json!({"cell": null}),
            serde_json::json!({"cell": -1.0}),
            serde_json::json!({"cell": 0.0}),
            // `json!` cannot hold an infinite number: it renders `1e308 * 10.0` as `Value::Null`,
            // so this entry pins the explicit-null path (an infinite literal is *ignored*, which
            // is what serde_json gives us). The strict block below is what proves a non-number is
            // refused.
            serde_json::json!({"cell": 1e308 * 10.0}),
            serde_json::json!({"z_step": -0.05}),
            serde_json::json!({"threshold": -1.0}),
            serde_json::json!({"close_radius": -5}),
            serde_json::json!({"close_radius": 4_000_000_000_u64}),
            serde_json::json!({"exterior_scan": "yes"}),
        ] {
            let read = BimOptions::from_params(&hostile);
            match read {
                // A value that cannot be used is reported, never guessed at, and never a panic.
                Ok(o) => assert!(o.cell.is_finite() && o.cell > 0.0 && o.z_step > 0.0 && o.close_radius <= crate::bim::MAX_CLOSE_RADIUS),
                Err(e) => assert!(matches!(e, BimError::InvalidCell | BimError::TooLarge { .. })),
            }
        }
        // Values that really do reach `number`'s error arm rather than its `null` arm: a bool, a
        // numeric-looking string, an array and a whole number in a string all get `None` from
        // `as_f64` while sitting under a known key, so each must be an error the caller can see.
        for junk in [
            serde_json::json!({"cell": true}),
            serde_json::json!({"cell": "0.05"}),
            serde_json::json!({"cell": [0.05]}),
            serde_json::json!({"close_radius": "2"}),
            serde_json::json!({"threshold": false}),
            serde_json::json!({"epsilon": ""}),
        ] {
            assert!(
                matches!(BimOptions::from_params(&junk), Err(BimError::InvalidCell)),
                "{junk} is a value under a known key that is not a number: refused, not defaulted"
            );
        }
    }

    #[test]
    fn the_plan_b_options_are_read_and_their_budgets_enforced() {
        let o = BimOptions::from_params(&serde_json::json!({
            "opening_cell": 0.01, "opening_threshold": 0.4, "opening_rank": 12,
            "min_opening_width": 0.7, "max_opening_width": 1.5, "door_min_height": 1.95,
            "door_max_sill": 0.1, "window_min_sill": 0.6, "opening_band": 0.04,
            "opening_decimate": 0.004, "max_wall_band_points": 150_000,
            "deviation_threshold": 0.06, "deviation_cell": 0.08,
            "min_furniture_points": 40, "max_furniture_clusters": 128
        }))
        .expect("every Plan B key parses");
        assert_eq!(o.opening_cell, 0.01);
        assert_eq!(o.opening_threshold, 0.4);
        assert_eq!(o.opening_rank, 12);
        assert_eq!((o.min_opening_width, o.max_opening_width), (0.7, 1.5));
        assert_eq!(o.door_min_height, 1.95);
        assert_eq!(o.door_max_sill, 0.1);
        assert_eq!(o.window_min_sill, 0.6);
        assert_eq!(o.opening_band, 0.04);
        assert_eq!(o.opening_decimate, 0.004);
        assert_eq!(o.max_wall_band_points, 150_000);
        assert_eq!(o.deviation_threshold, 0.06);
        assert_eq!(o.deviation_cell, 0.08);
        assert_eq!(o.min_furniture_points, 40);
        assert_eq!(o.max_furniture_clusters, 128);
        // What cannot be a count is refused, and what cannot fit says which budget it hit.
        for refused in [
            serde_json::json!({"opening_rank": 0}),
            serde_json::json!({"min_furniture_points": 0}),
            serde_json::json!({"max_furniture_clusters": 0}),
            serde_json::json!({"max_wall_band_points": 0}),
            serde_json::json!({"deviation_cell": -0.1}),
            serde_json::json!({"opening_threshold": 0.0}),
            serde_json::json!({"opening_rank": 2.5}),
        ] {
            assert!(matches!(BimOptions::from_params(&refused), Err(BimError::InvalidCell)), "{refused} must be refused");
        }
        assert!(
            matches!(
                BimOptions::from_params(&serde_json::json!({"max_wall_band_points": MAX_BAND_POINTS + 1})),
                Err(BimError::TooLarge { limit, .. }) if limit == MAX_BAND_POINTS
            ),
            "the wall band budget is a refusal, not a clamp"
        );
        assert!(
            matches!(
                BimOptions::from_params(&serde_json::json!({"opening_rank": MAX_OPENING_RANK + 1})),
                Err(BimError::TooLarge { limit, .. }) if limit == MAX_OPENING_RANK
            ),
            "the opening rank names its own budget"
        );
        assert!(
            matches!(
                BimOptions::from_params(&serde_json::json!({"max_furniture_clusters": MAX_FURNITURE_CLUSTERS + 1})),
                Err(BimError::TooLarge { limit, .. }) if limit == MAX_FURNITURE_CLUSTERS
            ),
            "the cluster budget names its own budget"
        );
    }
}
