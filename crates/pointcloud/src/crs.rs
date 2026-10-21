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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Crs {
    /// The EPSG code, when the file names one.
    pub epsg: Option<u32>,
    /// The WKT, when the file carries one.
    pub wkt: Option<String>,
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
        Some(Crs { epsg, wkt })
    }

    /// The name of the system, for the ones a survey office meets every day: WGS 84, the UTM
    /// zones and the web mercator. `None` for the rest — the code is the honest answer.
    pub fn name(&self) -> Option<String> {
        let code = self.epsg?;
        Some(match code {
            4326 => "WGS 84 (geographic)".to_string(),
            4269 => "NAD83 (geographic)".to_string(),
            3857 => "WGS 84 / Pseudo-Mercator".to_string(),
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
        let named = |code: u32| Crs { epsg: Some(code), wkt: None }.name();
        assert_eq!(named(4326).as_deref(), Some("WGS 84 (geographic)"));
        assert_eq!(named(32610).as_deref(), Some("WGS 84 / UTM zone 10N"));
        assert_eq!(named(32734).as_deref(), Some("WGS 84 / UTM zone 34S"));
        assert_eq!(named(3857).as_deref(), Some("WGS 84 / Pseudo-Mercator"));
        assert_eq!(named(1234), None, "an unknown code has no name, only the number");
    }

    #[test]
    fn a_geographic_system_is_told_from_a_projected_one() {
        assert!(Crs { epsg: Some(4326), wkt: None }.is_geographic());
        assert!(!Crs { epsg: Some(32610), wkt: None }.is_geographic());
    }
}
