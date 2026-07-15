//! Point-in-polygon address inheritance for OSM POIs.
//!
//! Two spatial indexes let a POI that lacks its own `addr:street` inherit an address from
//! nearby OSM data: [`AddressPolygonIndex`] (the addressed building the POI sits inside) and
//! [`AddressNodeIndex`] (the nearest standalone address node). Both mirror the grid-bucket
//! pattern in `street.rs`; containment uses `geometry::ray_cast_contains`.

use std::collections::HashMap;

use super::coordinate::Coordinate;
use super::geometry::{ray_cast_contains, BoundingBox};
use super::grid::{distance_meters, GRID_SIZE};

/// Address nodes farther than this from a POI are not treated as its address.
const MAX_ADDR_NODE_DISTANCE_METERS: f64 = 20.0;

/// A street + optional housenumber inherited from a containing polygon or a nearby node.
pub(crate) struct InheritedAddress {
    pub street: String,
    pub housenumber: Option<String>,
}

// ---------------------------------------------------------------------------
// AddressPolygonIndex -- addressed building polygons
// ---------------------------------------------------------------------------

struct AddressPolygon {
    street: String,
    housenumber: Option<String>,
    ring: Vec<Coordinate>,
    bbox: BoundingBox,
    way_id: i64,
}

/// Grid-bucketed index of addressed polygons. A polygon is bucketed into every cell its
/// bounding box covers, so a query point that lies inside a polygon always finds it in its
/// own cell -- no ring expansion needed.
pub(crate) struct AddressPolygonIndex {
    polygons: Vec<AddressPolygon>,
    grid: HashMap<(i32, i32), Vec<usize>>,
}

impl AddressPolygonIndex {
    pub fn new() -> Self {
        Self { polygons: Vec::new(), grid: HashMap::new() }
    }

    /// Adds a closed ring carrying `addr:street`. Rings with fewer than 3 points are skipped.
    pub fn add_polygon(
        &mut self,
        street: &str,
        housenumber: Option<&str>,
        ring: &[Coordinate],
        way_id: i64,
    ) {
        if ring.len() < 3 {
            return;
        }
        let Some(bbox) = BoundingBox::from_coordinates(ring) else {
            return;
        };
        let idx = self.polygons.len();
        self.polygons.push(AddressPolygon {
            street: street.to_string(),
            housenumber: housenumber.map(|s| s.to_string()),
            ring: ring.to_vec(),
            bbox,
            way_id,
        });

        let min_lat_cell = (bbox.min_lat / GRID_SIZE) as i32;
        let max_lat_cell = (bbox.max_lat / GRID_SIZE) as i32;
        let min_lon_cell = (bbox.min_lon / GRID_SIZE) as i32;
        let max_lon_cell = (bbox.max_lon / GRID_SIZE) as i32;
        for lat_cell in min_lat_cell..=max_lat_cell {
            for lon_cell in min_lon_cell..=max_lon_cell {
                self.grid.entry((lat_cell, lon_cell)).or_default().push(idx);
            }
        }
    }

    /// Containing polygon with the smallest bounding box wins (an inner building beats an
    /// enclosing block); ties broken by smallest `way_id` for determinism.
    pub fn find_containing(&self, coord: &Coordinate) -> Option<InheritedAddress> {
        let cell = ((coord.lat / GRID_SIZE) as i32, (coord.lon / GRID_SIZE) as i32);
        self.grid
            .get(&cell)?
            .iter()
            .map(|&idx| &self.polygons[idx])
            .filter(|p| p.bbox.contains(coord) && ray_cast_contains(&p.ring, coord))
            .min_by(|a, b| {
                a.bbox.area().total_cmp(&b.bbox.area()).then(a.way_id.cmp(&b.way_id))
            })
            .map(|p| InheritedAddress {
                street: p.street.clone(),
                housenumber: p.housenumber.clone(),
            })
    }

    pub fn get_statistics(&self) -> String {
        format!(
            "Loaded {} addressed polygons in {} grid cells",
            self.polygons.len(),
            self.grid.len()
        )
    }
}

// ---------------------------------------------------------------------------
// AddressNodeIndex -- standalone address nodes
// ---------------------------------------------------------------------------

struct AddressNode {
    coord: Coordinate,
    street: String,
    housenumber: Option<String>,
    node_id: i64,
}

/// Grid-bucketed index of standalone address nodes, queried by nearest-within-radius.
pub(crate) struct AddressNodeIndex {
    nodes: Vec<AddressNode>,
    grid: HashMap<(i32, i32), Vec<usize>>,
}

impl AddressNodeIndex {
    pub fn new() -> Self {
        Self { nodes: Vec::new(), grid: HashMap::new() }
    }

    pub fn add_node(
        &mut self,
        coord: Coordinate,
        street: &str,
        housenumber: Option<&str>,
        node_id: i64,
    ) {
        let idx = self.nodes.len();
        self.nodes.push(AddressNode {
            coord,
            street: street.to_string(),
            housenumber: housenumber.map(|s| s.to_string()),
            node_id,
        });
        let cell = ((coord.lat / GRID_SIZE) as i32, (coord.lon / GRID_SIZE) as i32);
        self.grid.entry(cell).or_default().push(idx);
    }

    /// Nearest address node within [`MAX_ADDR_NODE_DISTANCE_METERS`]; ties broken by smallest
    /// `node_id`. The cutoff is far smaller than a grid cell, so any match is at most one cell
    /// away and a fixed 3x3 neighborhood scan is exhaustive.
    pub fn find_nearest(&self, coord: &Coordinate) -> Option<InheritedAddress> {
        let lat_cell = (coord.lat / GRID_SIZE) as i32;
        let lon_cell = (coord.lon / GRID_SIZE) as i32;

        let mut best: Option<(&AddressNode, f64)> = None;
        for d_lat in -1..=1 {
            for d_lon in -1..=1 {
                let Some(indices) = self.grid.get(&(lat_cell + d_lat, lon_cell + d_lon)) else {
                    continue;
                };
                for &idx in indices {
                    let node = &self.nodes[idx];
                    let dist = distance_meters(coord, &node.coord);
                    if dist > MAX_ADDR_NODE_DISTANCE_METERS {
                        continue;
                    }
                    let better = match best {
                        None => true,
                        Some((b, bd)) => dist < bd || (dist == bd && node.node_id < b.node_id),
                    };
                    if better {
                        best = Some((node, dist));
                    }
                }
            }
        }

        best.map(|(node, _)| InheritedAddress {
            street: node.street.clone(),
            housenumber: node.housenumber.clone(),
        })
    }

    pub fn get_statistics(&self) -> String {
        format!(
            "Loaded {} address nodes in {} grid cells",
            self.nodes.len(),
            self.grid.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A unit square [59.0,59.001] x [10.0,10.001], closed ring.
    fn unit_square() -> Vec<Coordinate> {
        vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 59.0, lon: 10.001 },
            Coordinate { lat: 59.001, lon: 10.001 },
            Coordinate { lat: 59.001, lon: 10.0 },
            Coordinate { lat: 59.0, lon: 10.0 },
        ]
    }

    #[test]
    fn polygon_contains_inherits_both() {
        let mut index = AddressPolygonIndex::new();
        index.add_polygon("Storgata", Some("5"), &unit_square(), 1);

        let inside = index.find_containing(&Coordinate { lat: 59.0005, lon: 10.0005 }).unwrap();
        assert_eq!(inside.street, "Storgata");
        assert_eq!(inside.housenumber.as_deref(), Some("5"));
    }

    #[test]
    fn polygon_miss_outside_ring() {
        let mut index = AddressPolygonIndex::new();
        index.add_polygon("Storgata", Some("5"), &unit_square(), 1);

        assert!(index.find_containing(&Coordinate { lat: 59.5, lon: 10.5 }).is_none());
    }

    #[test]
    fn polygon_smallest_area_wins() {
        let mut index = AddressPolygonIndex::new();
        // Big enclosing block.
        let big = vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 59.0, lon: 10.01 },
            Coordinate { lat: 59.01, lon: 10.01 },
            Coordinate { lat: 59.01, lon: 10.0 },
            Coordinate { lat: 59.0, lon: 10.0 },
        ];
        index.add_polygon("Block", Some("1"), &big, 10);
        // Small inner building fully inside the block.
        index.add_polygon("Storgata", Some("5"), &unit_square(), 20);

        let hit = index.find_containing(&Coordinate { lat: 59.0005, lon: 10.0005 }).unwrap();
        assert_eq!(hit.street, "Storgata");
    }

    #[test]
    fn polygon_rejects_degenerate_ring() {
        let mut index = AddressPolygonIndex::new();
        index.add_polygon(
            "X",
            None,
            &[Coordinate { lat: 59.0, lon: 10.0 }, Coordinate { lat: 59.0, lon: 10.001 }],
            1,
        );
        assert!(index.find_containing(&Coordinate { lat: 59.0, lon: 10.0005 }).is_none());
    }

    #[test]
    fn node_within_radius_inherits_both() {
        let mut index = AddressNodeIndex::new();
        index.add_node(Coordinate { lat: 59.9, lon: 10.7 }, "Storgata", Some("12"), 1);

        // ~11 m north -- within 20 m.
        let hit = index.find_nearest(&Coordinate { lat: 59.9001, lon: 10.7 }).unwrap();
        assert_eq!(hit.street, "Storgata");
        assert_eq!(hit.housenumber.as_deref(), Some("12"));
    }

    #[test]
    fn node_outside_radius_is_none() {
        let mut index = AddressNodeIndex::new();
        index.add_node(Coordinate { lat: 59.9, lon: 10.7 }, "Storgata", Some("12"), 1);

        // ~55 m north -- beyond 20 m.
        assert!(index.find_nearest(&Coordinate { lat: 59.9005, lon: 10.7 }).is_none());
    }

    #[test]
    fn node_radius_boundary_brackets_cutoff() {
        let mut index = AddressNodeIndex::new();
        index.add_node(Coordinate { lat: 59.9, lon: 10.7 }, "Storgata", Some("12"), 1);

        // ~18 m north -- inside the 20 m cutoff.
        assert!(index.find_nearest(&Coordinate { lat: 59.9001622, lon: 10.7 }).is_some());
        // ~22 m north -- outside.
        assert!(index.find_nearest(&Coordinate { lat: 59.9001982, lon: 10.7 }).is_none());
    }

    #[test]
    fn node_tie_broken_by_smallest_id() {
        let mut index = AddressNodeIndex::new();
        let c = Coordinate { lat: 59.9, lon: 10.7 };
        index.add_node(c, "Second", Some("2"), 99);
        index.add_node(c, "First", Some("1"), 5);

        let hit = index.find_nearest(&Coordinate { lat: 59.9, lon: 10.7 }).unwrap();
        assert_eq!(hit.street, "First");
    }
}
