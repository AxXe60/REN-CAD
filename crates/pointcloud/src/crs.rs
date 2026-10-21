//! The coordinate reference system a scan says it is in.
//!
//! A LAS file carries its CRS in one of two variable length records: the GeoTIFF keys (the EPSG
//! code) or the OGC WKT (the full definition). The [`las`] crate parses both, so this module
//! only has to turn them into something the palette, the reports and the exports can use.
//!
//! The fork does not reproject: the drawing is 2D in local units, and a scan is placed where its
//! file puts it. What the CRS buys is the truth about where the scan is — the code, the name, and
//! a WKT to write next to an exported drawing.

/// The coordinate reference system of a cloud.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crs {
    /// The EPSG code, when the file names one.
    pub epsg: Option<u32>,
    /// The WKT, when the file carries one.
    pub wkt: Option<String>,
    /// The offset of the local grid on the easting (0 for the standard system).
    pub false_easting: f64,
    /// The offset of the local grid on the northing (0 for the standard system).
    pub false_northing: f64,
}

impl Crs {
    /// Read the CRS from a LAS header: the GeoTIFF keys give the EPSG code, the WKT the whole
    /// definition. `None` when the file says nothing about where it is.
    pub fn from_header(header: &las::Header) -> Option<Self> {
        let wkt =
            header.get_wkt_crs_bytes().and_then(|bytes| std::str::from_utf8(bytes).ok()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let epsg = header
            .get_geotiff_crs()
            .ok()
            .flatten()
            .and_then(|keys| keys.get_projected_crs_geo_key_value().or_else(|| keys.get_geodetic_crs_geo_key_value()))
            .map(u32::from)
            .filter(|code| *code >= 1024)
            .or_else(|| wkt.as_deref().and_then(epsg_from_wkt));
        if epsg.is_none() && wkt.is_none() {
            return None;
        }
        Some(Crs { epsg, wkt, false_easting: 0.0, false_northing: 0.0 })
    }

    /// The name of the system, for the ones a survey office meets every day: WGS 84, the UTM
    /// zones, the Romanian Stereo 70 and the web mercator. `None` for the rest — the code is the
    /// honest answer.
    pub fn name(&self) -> Option<String> {
        let code = self.epsg?;
        Some(match code {
            4326 => "WGS 84 (geographic)".to_string(),
            4269 => "NAD83 (geographic)".to_string(),
            3857 => "WGS 84 / Pseudo-Mercator".to_string(),
            31700 => "Stereo 70 (Romania)".to_string(),
            32601..=32660 => format!("WGS 84 / UTM zone {}N", code - 32600),
            32701..=32760 => format!("WGS 84 / UTM zone {}S", code - 32700),
            26901..=26923 => format!("NAD83 / UTM zone {}N", code - 26900),
            _ => return None,
        })
    }

    /// Whether the system is geographic (longitude and latitude) rather than projected.
    pub fn is_geographic(&self) -> bool {
        matches!(self.epsg, Some(4326 | 4269))
    }

    /// The line written next to an exported drawing.
    pub fn prj(&self) -> Option<&str> {
        self.wkt.as_deref()
    }

    /// The WKT of a system the fork knows, for the files that name none: WGS 84, NAD83, their UTM
    /// zones and the Romanian Stereo 70. `None` for the rest — the code is still the honest answer.
    ///
    /// The offsets are the local grid's own: a survey office that writes the northing without its
    /// leading millions gets a WKT that says so, and the exported points land where they belong.
    pub fn wkt_for(epsg: u32, false_easting: f64, false_northing: f64) -> Option<String> {
        let geographic = |name: &str, datum: &str, ellipsoid: &str, code: &str| {
            format!(
                "GEOGCS[\"{name}\",DATUM[\"{datum}\",SPHEROID[\"{ellipsoid}\",6378137,298.257223563,AUTHORITY[\"EPSG\",\"7030\"]],AUTHORITY[\"EPSG\",\"6326\"]],PRIMEM[\"Greenwich\",0,AUTHORITY[\"EPSG\",\"8901\"]],UNIT[\"degree\",0.0174532925199433,AUTHORITY[\"EPSG\",\"9122\"]],AUTHORITY[\"EPSG\",\"{code}\"]]"
            )
        };
        let projected = |name: &str, base: &str, central_meridian: f64, scale: f64, easting: f64, northing: f64, code: u32| {
            format!(
                "PROJCS[\"{name}\",{base},PROJECTION[\"Transverse_Mercator\"],PARAMETER[\"latitude_of_origin\",0],PARAMETER[\"central_meridian\",{central_meridian}],PARAMETER[\"scale_factor\",{scale}],PARAMETER[\"false_easting\",{easting}],PARAMETER[\"false_northing\",{northing}],UNIT[\"metre\",1,AUTHORITY[\"EPSG\",\"9001\"]],AUTHORITY[\"EPSG\",\"{code}\"]]",
                name = name,
                base = base,
                central_meridian = central_meridian,
                scale = scale,
                easting = easting,
                northing = northing,
                code = code
            )
        };
        let wgs84 = geographic("WGS 84", "WGS_1984", "WGS 84", "4326");
        let nad83 = geographic("NAD83", "North_American_Datum_1983", "GRS 1980", "4269");
        Some(match epsg {
            4326 => wgs84,
            4269 => nad83,
            32601..=32660 => projected(
                &format!("WGS 84 / UTM zone {}N", epsg - 32600),
                &wgs84,
                6.0 * f64::from(epsg - 32600) - 183.0,
                0.9996,
                500_000.0 + false_easting,
                false_northing,
                epsg,
            ),
            32701..=32760 => projected(
                &format!("WGS 84 / UTM zone {}S", epsg - 32700),
                &wgs84,
                6.0 * f64::from(epsg - 32700) - 183.0,
                0.9996,
                500_000.0 + false_easting,
                10_000_000.0 + false_northing,
                epsg,
            ),
            26901..=26923 => projected(
                &format!("NAD83 / UTM zone {}N", epsg - 26900),
                &nad83,
                6.0 * f64::from(epsg - 26900) - 183.0,
                0.9996,
                500_000.0 + false_easting,
                false_northing,
                epsg,
            ),
            // Stereo 70: the Romanian national grid on GRS 80, the central meridian 25°E and the
            // scale factor 0.9997. The false northing of the standard is 3 000 000 m; a local grid
            // that writes the northing without its leading millions carries the difference here.
            31700 => projected(
                "Stereo 70",
                &geographic("ETRS89", "European_Terrestrial_Reference_System_1989", "GRS 1980", "6258"),
                25.0,
                0.9997,
                500_000.0 + false_easting,
                3_000_000.0 + false_northing,
                epsg,
            ),
            _ => return None,
        })
    }
}

/// The EPSG code out of a WKT string: the last `AUTHORITY["EPSG","32610"]` (or `ID["EPSG",32610]`)
/// in it. A WKT may name several authorities, and the last one is the system's own.
pub fn epsg_from_wkt(wkt: &str) -> Option<u32> {
    let mut found = None;
    for (index, _) in wkt.match_indices("AUTHORITY") {
        // `AUTHORITY["EPSG","32610"]`: the quoted pieces are the authority's name and its code.
        let pieces: Vec<&str> = wkt[index..].split('"').collect();
        if pieces.len() > 3
            && pieces[1].eq_ignore_ascii_case("EPSG")
            && let Ok(code) = pieces[3].trim().parse::<u32>()
        {
            found = Some(code);
        }
    }
    if found.is_some() {
        return found;
    }
    // `ID["EPSG",32610]`, as GeoTIFF WKT writes it.
    for (index, _) in wkt.match_indices("ID[\"EPSG\"") {
        let rest = &wkt[index + "ID[\"EPSG\"".len()..];
        if let Some(digits) = rest.split([',', '"', ']']).map(str::trim).find(|p| !p.is_empty())
            && let Ok(code) = digits.parse::<u32>()
        {
            found = Some(code);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diagnostic: `RENCAD_CRS_PROBE=<file.las> cargo test -p cadcraft-pointcloud crs_probe`
    /// prints the header's variable length records, to see how a file names its system.
    #[test]
    fn crs_probe() {
        let Ok(path) = std::env::var("RENCAD_CRS_PROBE") else { return };
        let reader = las::Reader::from_path(&path).expect("the file opens");
        let header = reader.header();
        println!("version {:?} format {:?}", header.version(), header.point_format());
        for vlr in header.all_vlrs() {
            println!("VLR user={:?} record={} desc={:?} bytes={}", vlr.user_id, vlr.record_id, vlr.description, vlr.data.len());
            if vlr.data.len() < 400 {
                println!("   text: {}", String::from_utf8_lossy(&vlr.data));
            }
        }
    }

    /// A diagnostic: `RENCAD_READ_PROBE=<file.las> cargo test -p cadcraft-pointcloud read_probe`
    /// times the read on its own and with the cell set, to see where a 50-million-point file goes.
    #[test]
    fn read_probe() {
        let Ok(path) = std::env::var("RENCAD_READ_PROBE") else { return };
        let t = std::time::Instant::now();
        let mut reader = las::Reader::from_path(&path).expect("the file opens");
        let mut n = 0u64;
        loop {
            let batch = reader.read_points(1_000_000).expect("the batch reads");
            if batch.is_empty() {
                break;
            }
            n += batch.points().count() as u64;
        }
        println!("read {n} points in {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let mut reader = las::Reader::from_path(&path).expect("the file opens");
        let mut seen: std::collections::HashSet<(i64, i64, i64)> = std::collections::HashSet::new();
        let mut kept = 0u64;
        loop {
            let batch = reader.read_points(1_000_000).expect("the batch reads");
            if batch.is_empty() {
                break;
            }
            for p in batch.points() {
                let p = p.expect("the point reads");
                let key = ((p.x / 0.005).floor() as i64, (p.y / 0.005).floor() as i64, (p.z / 0.005).floor() as i64);
                if seen.insert(key) {
                    kept += 1;
                }
            }
        }
        println!("read + binned {kept} points in {:?}", t.elapsed());
    }

    const WKT: &str = r#"PROJCS["WGS 84 / UTM zone 10N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],UNIT["metre",1],AUTHORITY["EPSG","32610"]]"#;

    /// A header with the OGC WKT record: the EPSG code comes out of the WKT.
    #[test]
    fn the_wkt_record_gives_the_epsg_code() {
        let mut header = las::Builder::from(las::Version::new(1, 4)).into_header().unwrap_or_default();
        header.set_wkt_crs(WKT.as_bytes().to_vec()).expect("the WKT record fits");
        let crs = Crs::from_header(&header).expect("a CRS");
        assert_eq!(crs.epsg, Some(32610));
        assert_eq!(crs.name().as_deref(), Some("WGS 84 / UTM zone 10N"));
        assert!(crs.wkt.is_some());
    }

    /// A header with the GeoTIFF keys: the EPSG code is the projected CRS key (3072).
    #[test]
    fn the_geotiff_keys_give_the_epsg_code() {
        let mut keys: Vec<u8> = Vec::new();
        for value in [1u16, 1, 0, 1] {
            keys.extend_from_slice(&value.to_le_bytes());
        }
        // Key 3072 (ProjectedCSTypeGeoKey), value in the record, count 1.
        for value in [3072u16, 0, 1, 32610] {
            keys.extend_from_slice(&value.to_le_bytes());
        }
        let mut builder = las::Builder::from(las::Version::new(1, 2));
        builder.vlrs.push(las::Vlr {
            user_id: "LASF_Projection".to_string(),
            record_id: 34735,
            description: "GeoTIFF projection".to_string(),
            data: keys,
        });
        let header = builder.into_header().unwrap_or_default();
        let crs = Crs::from_header(&header).expect("a CRS");
        assert_eq!(crs.epsg, Some(32610));
    }

    /// A file that says nothing about where it is.
    #[test]
    fn a_file_without_a_crs_has_none() {
        let header = las::Builder::from(las::Version::new(1, 2)).into_header().unwrap_or_default();
        assert_eq!(Crs::from_header(&header), None);
    }

    #[test]
    fn the_epsg_code_comes_out_of_a_wkt() {
        let wkt = r#"PROJCS["WGS 84 / UTM zone 10N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],UNIT["metre",1],AUTHORITY["EPSG","32610"]]"#;
        assert_eq!(epsg_from_wkt(wkt), Some(32610));
    }

    #[test]
    fn the_last_authority_wins() {
        let wkt = r#"GEOGCS["WGS 84",AUTHORITY["EPSG","4326"],AUTHORITY["EPSG","4979"]]"#;
        assert_eq!(epsg_from_wkt(wkt), Some(4979));
    }

    #[test]
    fn the_id_form_is_read_too() {
        let wkt = r#"GEOGCS["WGS 84",ID["EPSG",4326]]"#;
        assert_eq!(epsg_from_wkt(wkt), Some(4326));
    }

    #[test]
    fn a_wkt_without_an_epsg_code_gives_none() {
        assert_eq!(epsg_from_wkt(r#"LOCAL_CS["Site grid"]"#), None);
        assert_eq!(epsg_from_wkt(""), None);
    }

    #[test]
    fn the_common_systems_have_names() {
        let named = |code: u32| Crs { epsg: Some(code), wkt: None, false_easting: 0.0, false_northing: 0.0 }.name();
        assert_eq!(named(4326).as_deref(), Some("WGS 84 (geographic)"));
        assert_eq!(named(32610).as_deref(), Some("WGS 84 / UTM zone 10N"));
        assert_eq!(named(32734).as_deref(), Some("WGS 84 / UTM zone 34S"));
        assert_eq!(named(3857).as_deref(), Some("WGS 84 / Pseudo-Mercator"));
        assert_eq!(named(1234), None, "an unknown code has no name, only the number");
    }

    #[test]
    fn a_geographic_system_is_told_from_a_projected_one() {
        assert!(Crs { epsg: Some(4326), wkt: None, false_easting: 0.0, false_northing: 0.0 }.is_geographic());
        assert!(!Crs { epsg: Some(32610), wkt: None, false_easting: 0.0, false_northing: 0.0 }.is_geographic());
    }

    /// The WKT the fork writes next to an exported drawing when only the code is known.
    #[test]
    fn the_known_systems_get_a_wkt() {
        let wkt = Crs::wkt_for(32610, 0.0, 0.0).expect("UTM 10N is known");
        assert!(wkt.contains("AUTHORITY[\"EPSG\",\"32610\"]"), "{wkt}");
        assert!(wkt.contains("PARAMETER[\"central_meridian\",-123]"), "zone 10's meridian is 123°W: {wkt}");
        assert!(wkt.contains("PARAMETER[\"false_easting\",500000]"), "{wkt}");
        let south = Crs::wkt_for(32734, 0.0, 0.0).expect("UTM 34S is known");
        assert!(south.contains("PARAMETER[\"false_northing\",10000000]"), "{south}");
        let geographic = Crs::wkt_for(4326, 0.0, 0.0).expect("WGS 84 is known");
        assert!(geographic.starts_with("GEOGCS[\"WGS 84\""), "{geographic}");
        assert_eq!(Crs::wkt_for(1234, 0.0, 0.0), None, "an unknown code gets no WKT, only the number");
    }

    /// Stereo 70 with a local grid: the offsets are in the WKT, so an exported drawing lands where
    /// the survey office says it does.
    #[test]
    fn stereo_70_carries_the_local_offsets() {
        let wkt = Crs::wkt_for(31700, 0.0, -4_000_000.0).expect("Stereo 70 is known");
        assert!(wkt.contains("Stereo 70"), "{wkt}");
        assert!(wkt.contains("PARAMETER[\"central_meridian\",25]"), "{wkt}");
        assert!(wkt.contains("PARAMETER[\"scale_factor\",0.9997]"), "{wkt}");
        assert!(wkt.contains("PARAMETER[\"false_northing\",-1000000]"), "3 000 000 − 4 000 000: {wkt}");
        assert_eq!(Crs { epsg: Some(31700), wkt: None, false_easting: 0.0, false_northing: 0.0 }.name().as_deref(), Some("Stereo 70 (Romania)"));
    }
}
