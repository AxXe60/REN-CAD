//! The storey split: the voids between the slabs, and the points that stand in them.
//!
//! A storey is not a slab: it is the space *between* two of them, and the paper takes its points
//! from "the base of one slab and the top of the slab below it while maintaining by a default
//! setting 10 cm clearance from both the ceiling and the floor" (§3.2). The margin is what keeps a
//! wall's bottom row of points, and the furniture standing on the floor, out of the slice that
//! later looks for walls.
//!
//! Like the rest of `bim`, this module owns plain data: it reads [`Slab`] bounds and hands back
//! points in the world coordinates it was given.

use crate::bim::BimOptions;
use crate::bim::slabs::Slab;

/// One floor of the building: the void between two consecutive slabs, shrunk inward by
/// [`BimOptions::safety_margin`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Storey {
    /// Its place in the list [`split`] returned, counting from zero. A void the margin erased is
    /// not listed, so this is the storey number a user would read, not an index into `slabs`.
    pub index: usize,
    /// The bottom of the room: the top of the slab below it, plus the margin.
    pub base_z: f64,
    /// The top of the room: the underside of the slab above it, less the margin.
    pub top_z: f64,
}

/// The voids between consecutive slabs, in the order the slabs were given — which is
/// [`find`](crate::bim::slabs::find)'s ascending `base_z`.
///
/// The margin is applied **here and once**: the bounds that come back are already shrunk, so a
/// caller must not shrink them again ([`points_in`] does not). A void that the margin closes up —
/// two slabs less than `2 × safety_margin` apart, or one whose bounds are not numbers — is not a
/// room and is not returned, and the [`Storey::index`] numbering does not gap over it.
///
/// `split` returns a list rather than a `Result`, so a hostile `safety_margin` (not a number, or
/// negative, which would *widen* the rooms) is read as `0.0`: the rooms then touch the slabs,
/// which is what the paper's own "clearance from both the ceiling and the floor" degrades to.
pub fn split(slabs: &[Slab], opts: &BimOptions) -> Vec<Storey> {
    let margin = if opts.safety_margin.is_finite() && opts.safety_margin > 0.0 { opts.safety_margin } else { 0.0 };
    let mut out: Vec<Storey> = Vec::new();
    for pair in slabs.windows(2) {
        // `windows` hands two, but read them with a pattern: nothing here indexes a slice by hand.
        let [below, above] = pair else {
            continue;
        };
        let (base_z, top_z) = (below.top_z + margin, above.base_z - margin);
        if !base_z.is_finite() || !top_z.is_finite() || top_z <= base_z {
            continue;
        }
        out.push(Storey { index: out.len(), base_z, top_z });
    }
    out
}

/// The points standing in one storey, in world coordinates and in the order they came.
///
/// The bounds are the margined ones [`split`] already applied, so a point exactly on `base_z` or
/// exactly on `top_z` is inside and both ends are inclusive. Nothing is reordered, re-centred or
/// normalised: whatever comes back is what the wall stage rasterises, and it has to still be the
/// scan. A point with a coordinate that is not a number is skipped and never reaches the model.
///
/// The options are part of this stage's signature so every stage reads alike; the storey split
/// consults nothing in them here, because the only number it has — the margin — is already spent.
pub fn points_in(points: &[[f64; 3]], storey: &Storey, _opts: &BimOptions) -> Vec<[f64; 3]> {
    // The non-finite check is what this guard adds: an inverted storey already selects nothing
    // through the filter below, but a NaN bound would make every comparison false in a way that is
    // worth saying out loud rather than leaving to IEEE.
    if !storey.base_z.is_finite() || !storey.top_z.is_finite() || storey.top_z < storey.base_z {
        return Vec::new();
    }
    points.iter().filter(|p| p.iter().all(|v| v.is_finite()) && p[2] >= storey.base_z && p[2] <= storey.top_z).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::{Storey, points_in, split};
    use crate::bim::BimOptions;
    use crate::bim::slabs::Slab;
    use crate::bim::slabs::find;
    use crate::bim::slabs::fixture::four_face_box;

    // Two single-sided slabs, a floor at `floor_z` and a ceiling at `ceiling_z`, exactly as the
    // slab stage would pair nothing of them: the floor's body hangs below its face and the
    // ceiling's stands above it. Their plans are empty because the split reads only the Z bounds.
    fn two_slabs_at(floor_z: f64, ceiling_z: f64) -> Vec<Slab> {
        let t = BimOptions::default().single_sided_slab_thickness;
        vec![
            Slab { base_z: floor_z - t, top_z: floor_z, thickness: t, polygon: Vec::new(), holes: Vec::new(), single_sided: true },
            Slab { base_z: ceiling_z, top_z: ceiling_z + t, thickness: t, polygon: Vec::new(), holes: Vec::new(), single_sided: true },
        ]
    }

    #[test]
    fn a_storey_takes_the_points_between_the_slabs_and_drops_the_margin() {
        let slabs = two_slabs_at(0.0, 3.0);
        let storeys = split(&slabs, &BimOptions::default());
        assert_eq!(storeys.len(), 1);
        assert!((storeys[0].base_z - 0.1).abs() < 1e-9 && (storeys[0].top_z - 2.9).abs() < 1e-9, "0.1 m off the floor and 0.1 m under the ceiling");
        let pts = vec![[0.0, 0.0, 0.05], [0.0, 0.0, 1.5], [0.0, 0.0, 2.95]];
        assert_eq!(points_in(&pts, &storeys[0], &BimOptions::default()).len(), 1, "only the middle point");
    }

    #[test]
    fn the_margin_is_applied_once_by_split_and_never_again_by_points_in() {
        // B4: `Storey` already carries the margined bounds, so a point standing exactly on a bound
        // is inside the storey. Step 3's fixture cannot tell the two behaviours apart; this can.
        let storeys = split(&two_slabs_at(0.0, 3.0), &BimOptions::default());
        let storey = storeys.first().expect("the void between two slabs is a storey");
        let on_bounds = vec![[0.0, 0.0, storey.base_z], [0.0, 0.0, storey.top_z]];
        assert_eq!(points_in(&on_bounds, storey, &BimOptions::default()).len(), 2, "both bounds are inside the storey");
        let just_outside = vec![[0.0, 0.0, storey.base_z - 0.001], [0.0, 0.0, storey.top_z + 0.001]];
        assert!(points_in(&just_outside, storey, &BimOptions::default()).is_empty(), "and a millimetre past either bound is not");
    }

    #[test]
    fn points_in_hands_back_the_same_points_in_the_same_order() {
        // Task 3 rasterises whatever this returns, so the points must still be world points: no
        // reordering, no re-centring, no normalising.
        let storeys = split(&two_slabs_at(0.0, 3.0), &BimOptions::default());
        let storey = storeys.first().expect("one storey");
        let pts = vec![[10.5, -3.25, 1.0], [0.0, 0.0, 2.0], [7.0, 7.0, 0.5], [1.0, 1.0, 9.0]];
        assert_eq!(points_in(&pts, storey, &BimOptions::default()), vec![[10.5, -3.25, 1.0], [0.0, 0.0, 2.0], [7.0, 7.0, 0.5]]);
    }

    #[test]
    fn a_void_thinner_than_the_margin_twice_is_no_storey_and_the_indices_stay_contiguous() {
        let mut slabs = two_slabs_at(0.0, 3.0);
        // A third slab whose underside touches the ceiling slab's top: the void between them is
        // 3.2 .. 3.2 before the margin and 3.3 .. 3.1 after it, i.e. inside itself.
        slabs.push(Slab { base_z: 3.2, top_z: 3.35, thickness: 0.15, polygon: Vec::new(), holes: Vec::new(), single_sided: false });
        slabs.push(Slab { base_z: 6.0, top_z: 6.3, thickness: 0.3, polygon: Vec::new(), holes: Vec::new(), single_sided: false });
        let storeys = split(&slabs, &BimOptions::default());
        assert_eq!(storeys.len(), 2, "the squeezed void is not a room and is not listed");
        assert_eq!(storeys.iter().map(|s| s.index).collect::<Vec<_>>(), vec![0, 1], "and the numbering does not gap over it");
        // The room that survives is the next one up: from the third slab's top, margined, to the
        // fourth slab's underside, margined.
        assert!(
            (storeys[1].base_z - 3.45).abs() < 1e-9 && (storeys[1].top_z - 5.9).abs() < 1e-9,
            "got {} .. {}",
            storeys[1].base_z,
            storeys[1].top_z
        );
    }

    #[test]
    fn no_slabs_and_one_slab_divide_into_nothing() {
        let o = BimOptions::default();
        assert!(split(&[], &o).is_empty(), "an empty model has no storeys");
        let one = two_slabs_at(0.0, 3.0).into_iter().take(1).collect::<Vec<_>>();
        assert!(split(&one, &o).is_empty(), "one slab is a floor with no ceiling over it");
    }

    #[test]
    fn a_margin_that_is_not_a_usable_number_widens_nothing_and_never_panics() {
        // `split` promises a list rather than a `Result`, so the degradation has to be the harmless
        // one: a margin that is not a number, or negative — which would *widen* the rooms past the
        // slabs — reads as no margin at all.
        let slabs = two_slabs_at(0.0, 3.0);
        for hostile in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
            let o = BimOptions { safety_margin: hostile, ..BimOptions::default() };
            let storeys = split(&slabs, &o);
            let storey = storeys.first().expect("the void is still a storey");
            assert!(
                storey.base_z.abs() < 1e-9 && (storey.top_z - 3.0).abs() < 1e-9,
                "{hostile} must read as no margin, got {} .. {}",
                storey.base_z,
                storey.top_z
            );
        }
        // A storey whose bounds are not numbers holds nothing, and says so by returning nothing.
        let pts = [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        for junk in [Storey { index: 0, base_z: f64::NAN, top_z: 3.0 }, Storey { index: 0, base_z: 0.0, top_z: f64::INFINITY }] {
            assert!(points_in(&pts, &junk, &BimOptions::default()).is_empty(), "{junk:?} is not a room and takes no points");
        }
    }

    #[test]
    fn an_inside_out_storey_holds_no_points() {
        // M-4: the guard's real content is the non-finite check — an inverted storey selects
        // nothing through its filter anyway — but a caller-built `Storey` must not be trusted to be
        // sane, and this pins that it answers "nothing" rather than "everything".
        let junk = Storey { index: 0, base_z: 3.0, top_z: 0.0 };
        assert!(points_in(&[[0.0, 0.0, 1.0], [0.0, 0.0, 3.0]], &junk, &BimOptions::default()).is_empty());
    }

    #[test]
    fn a_zero_margin_storey_runs_from_slab_top_to_slab_base() {
        // The documented degradation of M-3, stated as semantics: with no margin the room touches
        // both slabs, which is what the paper's clearance becomes when the user asks for none.
        let o = BimOptions { safety_margin: 0.0, ..BimOptions::default() };
        let storeys = split(&two_slabs_at(0.0, 3.0), &o);
        let s = storeys.first().expect("0 margin is still a room");
        assert!(s.base_z.abs() < 1e-9 && (s.top_z - 3.0).abs() < 1e-9, "{} .. {}", s.base_z, s.top_z);
    }

    #[test]
    fn a_ceiling_over_the_slab_makes_a_second_storey() {
        // B2's second fixture: the case a real scan is in — floor 0.0, slab faces 3.0 and 3.3,
        // ceiling 6.3 — which pairs into three slabs and divides into two storeys.
        let slabs = find(&four_face_box(), &BimOptions::default()).expect("the box has slabs");
        assert_eq!(slabs.len(), 3, "the floor, the measured slab, the ceiling");
        let storeys = split(&slabs, &BimOptions::default());
        assert_eq!(storeys.len(), 2, "one room under the slab and one above it");
        assert!(
            (storeys[0].base_z - 0.1).abs() < 1e-9 && (storeys[0].top_z - 2.9).abs() < 1e-9,
            "the first room: 0.1 m off the floor and 0.1 m under the slab, got {} .. {}",
            storeys[0].base_z,
            storeys[0].top_z
        );
        assert!(
            (storeys[1].base_z - 3.4).abs() < 1e-9 && (storeys[1].top_z - 6.2).abs() < 1e-9,
            "the second room stops 0.1 m under the ceiling, got {} .. {}",
            storeys[1].base_z,
            storeys[1].top_z
        );
        assert_eq!(storeys[1].index, 1);
    }
}
