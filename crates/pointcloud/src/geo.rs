//! Putting a point on the map: latitude and longitude to the grid the project uses.
//!
//! The fork does not reproject a cloud, but it has to know where the project sits on the globe: a
//! survey office says "this scan is at 44.1124° N, 24.9746° E" and the app has to tie that to the
//! coordinates in the file. The conversion is the standard Transverse Mercator series, on the
//! ellipsoid the system uses (GRS 80 for Stereo 70 and the UTM zones).

/// A point on the ellipsoid, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// The ellipsoid and the projection of one system.
struct Projection {
    /// The semi-major axis, metres.
    a: f64,
    /// The flattening.
    f: f64,
    /// The scale factor on the central meridian.
    k0: f64,
    /// The central meridian, degrees.
    lon0: f64,
    /// The false easting, metres.
    false_easting: f64,
    /// The false northing, metres.
    false_northing: f64,
}

/// GRS 80: the same ellipsoid as WGS 84 to within a millimetre.
const GRS80_A: f64 = 6_378_137.0;
const GRS80_F: f64 = 1.0 / 298.257_222_101;

/// The projection of a system the fork knows, or `None` for the rest.
fn projection(epsg: u32) -> Option<Projection> {
    let utm = |zone: u32, south: bool| {
        Some(Projection {
            a: GRS80_A,
            f: GRS80_F,
            k0: 0.9996,
            lon0: 6.0 * f64::from(zone) - 183.0,
            false_easting: 500_000.0,
            false_northing: if south { 10_000_000.0 } else { 0.0 },
        })
    };
    match epsg {
        32601..=32660 => utm(epsg - 32600, false),
        32701..=32760 => utm(epsg - 32700, true),
        // Stereo 70: the central meridian 25°E, the scale factor 0.9997, the false northing
        // 3 000 000 m.
        31700 => Some(Projection { a: GRS80_A, f: GRS80_F, k0: 0.9997, lon0: 25.0, false_easting: 500_000.0, false_northing: 3_000_000.0 }),
        _ => None,
    }
}

/// The point in the system's grid, in metres: `[easting, northing]`.
///
/// `None` for a system the fork does not know how to project.
pub fn grid_from_lat_lon(epsg: u32, point: LatLon) -> Option<[f64; 2]> {
    let p = projection(epsg)?;
    if !point.lat.is_finite() || !point.lon.is_finite() || !(0.0..=90.0).contains(&point.lat) {
        return None;
    }
    let phi = point.lat.to_radians();
    let lambda = point.lon.to_radians();
    let lambda0 = p.lon0.to_radians();
    let e2 = p.f * (2.0 - p.f);
    let ep2 = e2 / (1.0 - e2);
    let sin_phi = phi.sin();
    let cos_phi = phi.cos();
    let tan_phi = phi.tan();
    let n = p.a / (1.0 - e2 * sin_phi * sin_phi).sqrt();
    let t = tan_phi * tan_phi;
    let c = ep2 * cos_phi * cos_phi;
    let a = (lambda - lambda0) * cos_phi;
    let a2 = a * a;
    let m = meridional_arc(p.a, e2, phi);
    // The latitude of origin is the equator in every system the fork knows, so the arc of the
    // foot point is zero.
    let m0 = 0.0;
    let easting =
        p.false_easting + p.k0 * n * (a + (1.0 - t + c) * a2 * a / 6.0 + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * a2 * a2 * a / 120.0);
    let northing = p.false_northing
        + p.k0
            * (m - m0
                + n * tan_phi
                    * (a2 / 2.0
                        + (5.0 - t + 9.0 * c + 4.0 * c * c) * a2 * a2 / 24.0
                        + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * a2 * a2 * a2 / 720.0));
    Some([easting, northing])
}

/// The meridional arc: the distance from the equator along the meridian.
fn meridional_arc(a: f64, e2: f64, phi: f64) -> f64 {
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    a * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
        + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
        - (35.0 * e6 / 3072.0) * (6.0 * phi).sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published GRS 80 quadrant: the meridional arc from the equator to the pole is
    /// 10 001 965.729 m.
    #[test]
    fn the_meridional_arc_matches_the_published_quadrant() {
        let m = meridional_arc(GRS80_A, GRS80_F * (2.0 - GRS80_F), std::f64::consts::FRAC_PI_2);
        assert!((m - 10_001_965.729).abs() < 1.0, "{m}");
    }

    /// A point on the central meridian lands on the false easting.
    #[test]
    fn a_point_on_the_meridian_lands_on_the_false_easting() {
        let g = grid_from_lat_lon(31700, LatLon { lat: 45.0, lon: 25.0 }).expect("Stereo 70");
        assert!((g[0] - 500_000.0).abs() < 0.001, "{g:?}");
        // The northing is the arc of 45° N, scaled by the system's 0.9997, plus the false northing.
        let arc = meridional_arc(GRS80_A, GRS80_F * (2.0 - GRS80_F), 45.0_f64.to_radians());
        assert!((g[1] - (3_000_000.0 + 0.9997 * arc)).abs() < 1.0, "the northing is the arc plus the false northing: {g:?}");
    }

    /// Three degrees east of the meridian is about 236 km east, on the ellipsoid.
    #[test]
    fn the_easting_grows_with_the_distance_from_the_meridian() {
        let g = grid_from_lat_lon(31700, LatLon { lat: 45.0, lon: 28.0 }).expect("Stereo 70");
        let east = g[0] - 500_000.0;
        assert!((east - 236_000.0).abs() < 3_000.0, "3° of longitude at 45° N: {east} m");
    }

    /// The project of Curte Ulei: 44.1124386 N, 24.9745642 E, in Stereo 70.
    #[test]
    fn the_project_sits_where_the_maps_say() {
        let g = grid_from_lat_lon(31700, LatLon { lat: 44.112_438_574_529_7, lon: 24.974_564_167_361_805 }).expect("Stereo 70");
        // The easting is 1.8 km west of the false easting (the meridian is 25° E).
        assert!((g[0] - 498_000.0).abs() < 400.0, "the easting: {g:?}");
        // The northing is the meridional arc of 44.11° N plus the false northing.
        assert!((g[1] - 7_884_000.0).abs() < 3_000.0, "the northing: {g:?}");
    }

    #[test]
    fn a_broken_point_gives_nothing() {
        assert!(grid_from_lat_lon(31700, LatLon { lat: f64::NAN, lon: 25.0 }).is_none());
        assert!(grid_from_lat_lon(31700, LatLon { lat: 91.0, lon: 25.0 }).is_none());
        assert!(grid_from_lat_lon(1234, LatLon { lat: 45.0, lon: 25.0 }).is_none(), "an unknown system");
    }
}
