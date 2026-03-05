use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use country_boundaries::{CountryBoundaries, LatLon};
use std::sync::LazyLock;

thread_local! {
    static UTM33_TO_WGS84: proj::Proj = proj::Proj::new_known_crs("EPSG:25833", "EPSG:4326", None)
        .expect("Failed to create UTM33N -> WGS84 projection");
}

/// Convert UTM zone 33N (EPSG:25833) to WGS84 lat/lon using the proj crate.
pub fn convert_utm33_to_lat_lon(easting: f64, northing: f64) -> Coordinate {
    UTM33_TO_WGS84.with(|proj| {
        let (lon, lat) = proj.convert((easting, northing)).expect("Failed to convert coordinates");
        Coordinate::new(lat, lon)
    })
}

/// Embedded country boundaries data (same file as used by Kotlin converter).
const BOUNDARIES_DATA: &[u8] = include_bytes!("../../data/boundaries60x30.ser");

static BOUNDARIES: LazyLock<CountryBoundaries> = LazyLock::new(|| {
    CountryBoundaries::from_reader(BOUNDARIES_DATA)
        .expect("Failed to load country boundaries")
});

/// Country detection from coordinates using country-boundaries crate.
pub fn get_country(coord: &Coordinate) -> Option<Country> {
    let latlon = LatLon::new(coord.lat, coord.lon).ok()?;
    let ids = BOUNDARIES.ids(latlon);
    ids.iter()
        .find(|id| id.len() == 2)
        .and_then(|id| Country::parse(Some(id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zagreb_is_croatia() {
        let coord = Coordinate::new(45.803417, 15.992278);
        let country = get_country(&coord);
        assert_eq!(country.unwrap().name, "hr");
    }

    #[test]
    fn test_oslo_is_norway() {
        let coord = Coordinate::new(59.9139, 10.7522);
        let country = get_country(&coord);
        assert_eq!(country.unwrap().name, "no");
    }
}
