use std::collections::HashMap;

use super::coordinate::Coordinate;
use super::geometry::{ray_cast_contains, BoundingBox};
use crate::common::country::Country;

/// OSM admin_level for counties/regions (e.g. Norwegian fylker, Swedish län).
pub const ADMIN_LEVEL_COUNTY: i32 = 4;
/// OSM admin_level for municipalities (e.g. Norwegian kommuner, Swedish kommuner).
pub const ADMIN_LEVEL_MUNICIPALITY: i32 = 7;

/// Cache precision: coordinates are rounded to 0.01° (~1 km) grid cells for lookup caching.
const CACHE_PRECISION: f64 = 100.0;

// ---------------------------------------------------------------------------
// AdministrativeBoundary
// ---------------------------------------------------------------------------

/// A county or municipality polygon from OSM, used for reverse-geocoding points
/// to their administrative region.
pub struct AdministrativeBoundary {
    pub name: String,
    pub admin_level: i32,
    pub ref_code: Option<String>,
    pub country: Country,
    pub centroid: Coordinate,
    pub bbox: Option<BoundingBox>,
    pub boundary_nodes: Vec<Coordinate>,
}

impl AdministrativeBoundary {
    /// Ray-casting algorithm -- check if point is inside the polygon.
    pub fn contains_point(&self, coord: &Coordinate) -> bool {
        ray_cast_contains(&self.boundary_nodes, coord)
    }

    /// Euclidean distance from the given point to this boundary's centroid.
    pub fn distance_to_point(&self, coord: &Coordinate) -> f64 {
        let d_lat = coord.lat - self.centroid.lat;
        let d_lon = coord.lon - self.centroid.lon;
        (d_lat * d_lat + d_lon * d_lon).sqrt()
    }

    pub fn is_in_bounding_box(&self, coord: &Coordinate) -> bool {
        self.bbox.as_ref().is_some_and(|b| b.contains(coord))
    }
}

// ---------------------------------------------------------------------------
// AdministrativeBoundaryIndex
// ---------------------------------------------------------------------------

/// Spatial index over admin boundaries. Given a coordinate, finds the containing
/// county and municipality using a 3-tier strategy: ray-casting → bounding box →
/// nearest centroid. Results are cached on a ~0.01° grid to avoid repeated lookups.
pub struct AdministrativeBoundaryIndex {
    counties: Vec<AdministrativeBoundary>,
    municipalities: Vec<AdministrativeBoundary>,
    lookup_cache: HashMap<(i64, i64), (Option<usize>, Option<usize>)>,
}

impl AdministrativeBoundaryIndex {
    pub fn new() -> Self {
        Self {
            counties: Vec::new(),
            municipalities: Vec::new(),
            lookup_cache: HashMap::new(),
        }
    }

    pub fn add_boundary(&mut self, boundary: AdministrativeBoundary) {
        match boundary.admin_level {
            ADMIN_LEVEL_COUNTY => self.counties.push(boundary),
            ADMIN_LEVEL_MUNICIPALITY => self.municipalities.push(boundary),
            _ => {}
        }
    }

    /// Finds both the county and municipality for the given coordinates.
    /// Results are cached with 0.01 degree precision.
    pub fn find_county_and_municipality(
        &mut self,
        coord: &Coordinate,
    ) -> (Option<&AdministrativeBoundary>, Option<&AdministrativeBoundary>) {
        let key = (
            round_cache_coord(coord.lat),
            round_cache_coord(coord.lon),
        );

        if !self.lookup_cache.contains_key(&key) {
            let county_idx = Self::find_best_match_idx(&self.counties, coord);
            let muni_idx = Self::find_best_match_idx(&self.municipalities, coord);
            self.lookup_cache.insert(key, (county_idx, muni_idx));
        }

        let &(county_idx, muni_idx) = self.lookup_cache.get(&key).unwrap();
        (
            county_idx.map(|i| &self.counties[i]),
            muni_idx.map(|i| &self.municipalities[i]),
        )
    }

    /// 3-tier lookup: ray-casting, bounding box + closest centroid, closest centroid.
    fn find_best_match_idx(
        boundaries: &[AdministrativeBoundary],
        coord: &Coordinate,
    ) -> Option<usize> {
        if boundaries.is_empty() {
            return None;
        }

        // Tier 1: ray-casting polygon containment
        let containing: Vec<usize> = boundaries
            .iter()
            .enumerate()
            .filter(|(_, b)| b.contains_point(coord))
            .map(|(i, _)| i)
            .collect();

        if !containing.is_empty() {
            return containing
                .into_iter()
                .min_by(|&a, &b| {
                    let area_a = boundaries[a].bbox.map_or(f64::MAX, |bb| bb.area());
                    let area_b = boundaries[b].bbox.map_or(f64::MAX, |bb| bb.area());
                    area_a.partial_cmp(&area_b).unwrap_or(std::cmp::Ordering::Equal)
                });
        }

        // Tier 2: bounding box + closest centroid
        let in_bbox: Vec<usize> = boundaries
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_in_bounding_box(coord))
            .map(|(i, _)| i)
            .collect();

        if !in_bbox.is_empty() {
            return in_bbox
                .into_iter()
                .min_by(|&a, &b| {
                    let da = boundaries[a].distance_to_point(coord);
                    let db = boundaries[b].distance_to_point(coord);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
        }

        // Tier 3: closest centroid (last resort)
        boundaries
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = a.distance_to_point(coord);
                let db = b.distance_to_point(coord);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    pub fn get_statistics(&self) -> String {
        format!(
            "Loaded {} counties and {} municipalities",
            self.counties.len(),
            self.municipalities.len()
        )
    }
}

fn round_cache_coord(v: f64) -> i64 {
    (v * CACHE_PRECISION) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ray_casting() {
        // Simple square polygon
        let boundary = AdministrativeBoundary {
            name: "Test".to_string(),
            admin_level: 4,
            ref_code: None,
            country: Country::no(),
            centroid: Coordinate { lat: 60.0, lon: 11.0 },
            bbox: None,
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        };
        assert!(boundary.contains_point(&Coordinate { lat: 60.0, lon: 11.0 }));
        assert!(!boundary.contains_point(&Coordinate { lat: 62.0, lon: 11.0 }));
    }
}
