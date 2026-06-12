use rusqlite::Connection;
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct BelagenhetAdress {
    pub objektidentitet: String,
    pub adressplatstyp: String,
    pub adressomrade_namn: Option<String>,
    pub gardsadressomrade_namn: Option<String>,
    pub adressplatsnummer: Option<String>,
    pub bokstavstillagg: Option<String>,
    pub lagestillagg: Option<String>,
    pub lagestillaggsnummer: Option<String>,
    pub avviker: bool,
    pub avvikande_beteckning: Option<String>,
    pub kommundel_namn: Option<String>,
    pub kommunkod: Option<String>,
    pub kommunnamn: Option<String>,
    pub lanskod: Option<String>,
    pub popularnamn: Option<String>,
    pub postnummer: Option<String>,
    pub postort: Option<String>,
    pub easting: f64,
    pub northing: f64,
}

pub(crate) struct StreetAgg {
    pub representative: BelagenhetAdress,
    pub sum_east: f64,
    pub sum_north: f64,
    pub count: usize,
}

impl BelagenhetAdress {
    /// Build the street or place name based on address type.
    pub fn street_or_place_name(&self) -> Option<String> {
        let base = self.adressomrade_namn.as_deref()?;
        if self.adressplatstyp == "Gårdsadressplats" {
            if let Some(farm) = &self.gardsadressomrade_namn {
                Some(format!("{base} {farm}"))
            } else {
                Some(base.to_string())
            }
        } else {
            Some(base.to_string())
        }
    }

    /// Whether this address type uses addr:street (vs addr:place).
    /// Unused in production: kept (test-only) to document the
    /// Gatuadressplats/Metertalsadressplats distinction for future
    /// addr:street vs addr:place handling.
    #[cfg(test)]
    pub fn is_street_address(&self) -> bool {
        matches!(self.adressplatstyp.as_str(), "Gatuadressplats" | "Metertalsadressplats")
    }

    /// Build the housenumber from component fields.
    pub fn housenumber(&self) -> Option<String> {
        if self.avviker {
            return self.avvikande_beteckning.clone();
        }

        let base = self.adressplatsnummer.as_deref()?;
        let mut hn = base.to_string();

        if let Some(letter) = &self.bokstavstillagg {
            hn.push_str(letter);
        }

        if let Some(tillagg) = &self.lagestillagg {
            hn.push(' ');
            hn.push_str(tillagg);
            if let Some(num) = &self.lagestillaggsnummer {
                hn.push_str(num);
            }
        }

        Some(hn)
    }
}

pub(crate) fn parse_gpkg(input: &Path) -> Result<Vec<BelagenhetAdress>, Box<dyn std::error::Error>> {
    let conn = Connection::open_with_flags(input, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // GeoPackage geometry is stored as binary. We parse easting/northing from the
    // raw geometry blob since ST_X/ST_Y are Spatialite functions not available in
    // plain rusqlite.
    let mut stmt = conn.prepare(
        "SELECT
            belagenhetsadress_objektidentitet,
            adressplatstyp,
            adressomrade_faststalltnamn,
            gardsadressomrade_faststalltnamn,
            adressplatsnummer,
            bokstavstillagg,
            lagestillagg,
            lagestillaggsnummer,
            avvikerfranstandarden,
            avvikandeadressplatsbeteckning,
            kommundel_faststalltnamn,
            kommunkod,
            kommunnamn,
            lanskod,
            popularnamn,
            postnummer,
            postort,
            statusforbelagenhetsadress,
            adressplatspunkt
        FROM belagenhetsadress"
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(RawRow {
            objektidentitet: row.get(0)?,
            adressplatstyp: row.get(1)?,
            adressomrade_namn: row.get(2)?,
            gardsadressomrade_namn: row.get(3)?,
            adressplatsnummer: row.get::<_, Option<String>>(4)?,
            bokstavstillagg: row.get(5)?,
            lagestillagg: row.get(6)?,
            lagestillaggsnummer: row.get::<_, Option<i64>>(7)?.map(|n| n.to_string()),
            avviker: row.get::<_, Option<i32>>(8)?.unwrap_or(0) != 0,
            avvikande_beteckning: row.get(9)?,
            kommundel_namn: row.get(10)?,
            kommunkod: row.get(11)?,
            kommunnamn: row.get(12)?,
            lanskod: row.get(13)?,
            popularnamn: row.get(14)?,
            postnummer: row.get(15)?,
            postort: row.get(16)?,
            status: row.get(17)?,
            geom_blob: row.get(18)?,
        })
    })?;

    let mut addresses = Vec::new();
    for row_result in rows {
        let raw = row_result?;

        // Filter: only "Gällande" (current) addresses with postort and postnummer > 0
        if raw.status != "Gällande" { continue; }
        if raw.postort.is_none() { continue; }
        if raw.postnummer.unwrap_or(0) == 0 { continue; }

        let (easting, northing) = parse_gpkg_point_geometry(&raw.geom_blob)?;

        let postnummer_str = raw.postnummer.map(|n| format!("{:05}", n));

        addresses.push(BelagenhetAdress {
            objektidentitet: raw.objektidentitet,
            adressplatstyp: raw.adressplatstyp,
            adressomrade_namn: raw.adressomrade_namn,
            gardsadressomrade_namn: raw.gardsadressomrade_namn,
            adressplatsnummer: raw.adressplatsnummer,
            bokstavstillagg: raw.bokstavstillagg,
            lagestillagg: raw.lagestillagg,
            lagestillaggsnummer: raw.lagestillaggsnummer,
            avviker: raw.avviker,
            avvikande_beteckning: raw.avvikande_beteckning,
            kommundel_namn: raw.kommundel_namn,
            kommunkod: raw.kommunkod,
            kommunnamn: raw.kommunnamn,
            lanskod: raw.lanskod,
            popularnamn: raw.popularnamn,
            postnummer: postnummer_str,
            postort: raw.postort,
            easting,
            northing,
        });
    }

    eprintln!("Parsed {} addresses from GeoPackage", addresses.len());
    Ok(addresses)
}

struct RawRow {
    objektidentitet: String,
    adressplatstyp: String,
    adressomrade_namn: Option<String>,
    gardsadressomrade_namn: Option<String>,
    adressplatsnummer: Option<String>,
    bokstavstillagg: Option<String>,
    lagestillagg: Option<String>,
    lagestillaggsnummer: Option<String>,
    avviker: bool,
    avvikande_beteckning: Option<String>,
    kommundel_namn: Option<String>,
    kommunkod: Option<String>,
    kommunnamn: Option<String>,
    lanskod: Option<String>,
    popularnamn: Option<String>,
    postnummer: Option<i64>,
    postort: Option<String>,
    status: String,
    geom_blob: Vec<u8>,
}

/// Parse a GeoPackage geometry binary (Standard GeoPackageBinary header + WKB Point).
///
/// GeoPackage binary format:
/// - 2 bytes: magic "GP" (0x47, 0x50)
/// - 1 byte: version
/// - 1 byte: flags (bit 0: byte order indicator for envelope, bits 1-3: envelope type)
/// - 4 bytes: SRS ID (int32)
/// - envelope (variable size based on envelope type)
/// - WKB geometry
///
/// WKB Point:
/// - 1 byte: byte order (1 = little-endian)
/// - 4 bytes: geometry type (1 = Point)
/// - 8 bytes: X (f64)
/// - 8 bytes: Y (f64)
fn parse_gpkg_point_geometry(blob: &[u8]) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    if blob.len() < 8 {
        return Err("Geometry blob too short".into());
    }

    // Verify GeoPackage magic bytes
    if blob[0] != 0x47 || blob[1] != 0x50 {
        return Err("Invalid GeoPackage geometry: missing GP magic bytes".into());
    }

    let flags = blob[3];
    let envelope_type = (flags >> 1) & 0x07;

    // Calculate envelope size
    let envelope_size: usize = match envelope_type {
        0 => 0,          // no envelope
        1 => 32,         // [minx, maxx, miny, maxy] = 4 * f64
        2 => 48,         // + [minz, maxz]
        3 => 48,         // + [minm, maxm]
        4 => 64,         // + [minz, maxz, minm, maxm]
        _ => return Err(format!("Unknown envelope type: {envelope_type}").into()),
    };

    let wkb_offset = 8 + envelope_size; // 8 = magic(2) + version(1) + flags(1) + srs_id(4)

    if blob.len() < wkb_offset + 21 {
        return Err("Geometry blob too short for WKB point".into());
    }

    let wkb = &blob[wkb_offset..];
    let wkb_byte_order = wkb[0]; // 0 = big-endian, 1 = little-endian

    // Verify geometry type is Point (1)
    let geom_type = if wkb_byte_order == 1 {
        u32::from_le_bytes(wkb[1..5].try_into()?)
    } else {
        u32::from_be_bytes(wkb[1..5].try_into()?)
    };
    if geom_type != 1 {
        return Err(format!("Expected WKB Point (type 1), got type {geom_type}").into());
    }

    let read_f64 = |range: std::ops::Range<usize>| -> Result<f64, Box<dyn std::error::Error>> {
        let bytes: [u8; 8] = wkb[range].try_into()?;
        Ok(if wkb_byte_order == 1 { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) })
    };
    let x = read_f64(5..13)?;
    let y = read_f64(13..21)?;

    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_addr(overrides: impl FnOnce(&mut BelagenhetAdress)) -> BelagenhetAdress {
        let mut addr = BelagenhetAdress {
            objektidentitet: "test-uuid".to_string(),
            adressplatstyp: "Gatuadressplats".to_string(),
            adressomrade_namn: Some("Storgatan".to_string()),
            gardsadressomrade_namn: None,
            adressplatsnummer: Some("42".to_string()),
            bokstavstillagg: None,
            lagestillagg: None,
            lagestillaggsnummer: None,
            avviker: false,
            avvikande_beteckning: None,
            kommundel_namn: None,
            kommunkod: None,
            kommunnamn: None,
            lanskod: None,
            popularnamn: None,
            postnummer: Some("11122".to_string()),
            postort: Some("Stockholm".to_string()),
            easting: 0.0,
            northing: 0.0,
        };
        overrides(&mut addr);
        addr
    }

    #[test]
    fn test_housenumber_simple() {
        let addr = test_addr(|_| {});
        assert_eq!(addr.housenumber(), Some("42".to_string()));
    }

    #[test]
    fn test_housenumber_with_letter() {
        let addr = test_addr(|a| { a.bokstavstillagg = Some("B".to_string()); });
        assert_eq!(addr.housenumber(), Some("42B".to_string()));
    }

    #[test]
    fn test_housenumber_with_lagestillagg() {
        let addr = test_addr(|a| {
            a.adressplatsnummer = Some("5".to_string());
            a.lagestillagg = Some("lgh".to_string());
            a.lagestillaggsnummer = Some("1001".to_string());
        });
        assert_eq!(addr.housenumber(), Some("5 lgh1001".to_string()));
    }

    #[test]
    fn test_housenumber_avvikande() {
        let addr = test_addr(|a| {
            a.avviker = true;
            a.avvikande_beteckning = Some("S:t Göran 3".to_string());
        });
        assert_eq!(addr.housenumber(), Some("S:t Göran 3".to_string()));
    }

    #[test]
    fn test_housenumber_avviker_with_no_beteckning() {
        let addr = test_addr(|a| {
            a.avviker = true;
            a.avvikande_beteckning = None;
        });
        assert_eq!(addr.housenumber(), None);
    }

    #[test]
    fn test_housenumber_no_nummer() {
        let addr = test_addr(|a| { a.adressplatsnummer = None; });
        assert_eq!(addr.housenumber(), None);
    }

    #[test]
    fn test_street_or_place_name_gatu() {
        let addr = test_addr(|a| { a.adressomrade_namn = Some("Kungsgatan".to_string()); });
        assert_eq!(addr.street_or_place_name(), Some("Kungsgatan".to_string()));
        assert!(addr.is_street_address());
    }

    #[test]
    fn test_street_or_place_name_gard() {
        let addr = test_addr(|a| {
            a.adressplatstyp = "Gårdsadressplats".to_string();
            a.adressomrade_namn = Some("Lilla By".to_string());
            a.gardsadressomrade_namn = Some("Nygård".to_string());
        });
        assert_eq!(addr.street_or_place_name(), Some("Lilla By Nygård".to_string()));
        assert!(!addr.is_street_address());
    }

    #[test]
    fn test_is_street_address() {
        for typ in ["Gatuadressplats", "Metertalsadressplats"] {
            let addr = test_addr(|a| { a.adressplatstyp = typ.to_string(); });
            assert!(addr.is_street_address(), "{typ} should be a street address");
        }
        for typ in ["Byadressplats", "Gårdsadressplats"] {
            let addr = test_addr(|a| { a.adressplatstyp = typ.to_string(); });
            assert!(!addr.is_street_address(), "{typ} should NOT be a street address");
        }
    }

    #[test]
    fn test_parse_gpkg_point_geometry_le_no_envelope() {
        let mut blob = Vec::new();
        blob.push(0x47); blob.push(0x50);
        blob.push(0x00); // version
        blob.push(0x00); // flags: no envelope
        blob.extend_from_slice(&3006_i32.to_le_bytes());
        blob.push(0x01); // WKB: LE
        blob.extend_from_slice(&1_u32.to_le_bytes()); // type = Point
        let x: f64 = 674032.67;
        let y: f64 = 6580125.42;
        blob.extend_from_slice(&x.to_le_bytes());
        blob.extend_from_slice(&y.to_le_bytes());

        let (ex, ey) = parse_gpkg_point_geometry(&blob).unwrap();
        assert!((ex - 674032.67).abs() < 0.01);
        assert!((ey - 6580125.42).abs() < 0.01);
    }

    #[test]
    fn test_parse_gpkg_point_geometry_with_envelope() {
        let mut blob = Vec::new();
        blob.push(0x47); blob.push(0x50);
        blob.push(0x00); // version
        blob.push(0x03); // flags: envelope_type=1, byte_order=1
        blob.extend_from_slice(&3006_i32.to_le_bytes());
        for _ in 0..4 { blob.extend_from_slice(&0.0_f64.to_le_bytes()); }
        blob.push(0x01); // WKB: LE
        blob.extend_from_slice(&1_u32.to_le_bytes());
        let x: f64 = 500000.0;
        let y: f64 = 6600000.0;
        blob.extend_from_slice(&x.to_le_bytes());
        blob.extend_from_slice(&y.to_le_bytes());

        let (ex, ey) = parse_gpkg_point_geometry(&blob).unwrap();
        assert!((ex - 500000.0).abs() < 0.01);
        assert!((ey - 6600000.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_gpkg_point_geometry_big_endian() {
        let mut blob = Vec::new();
        blob.push(0x47); blob.push(0x50);
        blob.push(0x00); // version
        blob.push(0x00); // flags: no envelope
        blob.extend_from_slice(&3006_i32.to_le_bytes()); // SRS ID in header is always based on flags byte order
        blob.push(0x00); // WKB: big-endian
        blob.extend_from_slice(&1_u32.to_be_bytes()); // type = Point (BE)
        let x: f64 = 674032.67;
        let y: f64 = 6580125.42;
        blob.extend_from_slice(&x.to_be_bytes());
        blob.extend_from_slice(&y.to_be_bytes());

        let (ex, ey) = parse_gpkg_point_geometry(&blob).unwrap();
        assert!((ex - 674032.67).abs() < 0.01);
        assert!((ey - 6580125.42).abs() < 0.01);
    }

    #[test]
    fn test_parse_gpkg_rejects_non_point_geometry() {
        let mut blob = Vec::new();
        blob.push(0x47); blob.push(0x50);
        blob.push(0x00);
        blob.push(0x00);
        blob.extend_from_slice(&3006_i32.to_le_bytes());
        blob.push(0x01); // WKB: LE
        blob.extend_from_slice(&2_u32.to_le_bytes()); // type = LineString (not Point)
        blob.extend_from_slice(&0.0_f64.to_le_bytes());
        blob.extend_from_slice(&0.0_f64.to_le_bytes());

        let result = parse_gpkg_point_geometry(&blob);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected WKB Point"));
    }
}
