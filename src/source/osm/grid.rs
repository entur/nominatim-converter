use super::coordinate::Coordinate;

/// Grid cell size in degrees (~550 m at 60°N latitude). Street segments are bucketed
/// into cells of this size for fast spatial lookup.
pub(crate) const GRID_SIZE: f64 = 0.005;

/// Approximate distance in metres between two coordinates (equirectangular projection).
/// Fast enough for spatial-index scans; accurate to well under a metre at the OSM
/// pipeline's ~100 m lookup radius.
pub(crate) fn distance_meters(a: &Coordinate, b: &Coordinate) -> f64 {
    let lat_scale = 111_000.0;
    let lon_scale = 111_000.0 * a.lat.to_radians().cos();
    let dx = (b.lon - a.lon) * lon_scale;
    let dy = (b.lat - a.lat) * lat_scale;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_meters_small_offset() {
        // 0.0001 deg lat is ~11.1 m.
        let d = distance_meters(
            &Coordinate { lat: 59.9, lon: 10.7 },
            &Coordinate { lat: 59.9001, lon: 10.7 },
        );
        assert!((d - 11.1).abs() < 0.5, "expected ~11.1 m, got {d}");
    }
}
