use std::collections::HashMap;

use crate::common::country::Country;

use super::address_index::AddressPolygonIndex;
use super::admin::{AdministrativeBoundary, AdministrativeBoundaryIndex};
use super::coordinate::{Coordinate, CoordinateStore};
use super::geometry::BoundingBox;
use super::street::StreetIndex;

// ---------------------------------------------------------------------------
// Intermediate data collected across passes
// ---------------------------------------------------------------------------

/// Admin boundary relation data collected during pass 1, used to build the
/// spatial index after node coordinates are available (pass 3).
pub(crate) struct AdminRelationData {
    pub(crate) name: String,
    pub(crate) admin_level: i32,
    pub(crate) ref_code: String,
    pub(crate) way_ids: Vec<i64>,
    pub(crate) country: Country,
}

/// Street way data collected during pass 2, used to build the street spatial
/// index for nearest-street lookups.
pub(crate) struct StreetWayData {
    pub(crate) name: String,
    pub(crate) node_ids: Vec<i64>,
}

/// A closed way carrying `addr:street`, collected during pass 2. Used to build the
/// address-polygon index so contained POIs can inherit the polygon's address.
pub(crate) struct AddressWayData {
    pub(crate) street: String,
    pub(crate) housenumber: Option<String>,
    pub(crate) node_ids: Vec<i64>,
    pub(crate) way_id: i64,
}

// ---------------------------------------------------------------------------
// Index builders
// ---------------------------------------------------------------------------

pub(crate) fn build_admin_boundary_index(
    admin_relations: &[AdminRelationData],
    admin_way_node_ids: &HashMap<i64, Vec<i64>>,
    nodes_coords: &CoordinateStore,
    index: &mut AdministrativeBoundaryIndex,
) {
    for relation in admin_relations {
        // Gather all coordinates for this admin boundary's ways.
        // Each admin relation references multiple ways; each way references multiple nodes.
        // We look up the node IDs for each way, then resolve each node ID to its coordinate.
        let all_node_coords: Vec<Coordinate> = relation
            .way_ids
            .iter()
            .flat_map(|way_id| {
                let node_ids = admin_way_node_ids.get(way_id);
                node_ids
                    .map(|nids| {
                        nids.iter()
                            .filter_map(|&nid| nodes_coords.get(nid))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();

        if all_node_coords.is_empty() {
            continue;
        }

        let centroid = Coordinate {
            lat: all_node_coords.iter().map(|c| c.lat).sum::<f64>()
                / all_node_coords.len() as f64,
            lon: all_node_coords.iter().map(|c| c.lon).sum::<f64>()
                / all_node_coords.len() as f64,
        };
        let bbox = BoundingBox::from_coordinates(&all_node_coords);

        let boundary = AdministrativeBoundary {
            name: relation.name.clone(),
            admin_level: relation.admin_level,
            ref_code: Some(relation.ref_code.clone()),
            country: relation.country.clone(),
            centroid,
            bbox,
            boundary_nodes: all_node_coords,
        };
        index.add_boundary(boundary);
    }
}

pub(crate) fn build_street_index(
    street_ways: &[StreetWayData],
    nodes_coords: &CoordinateStore,
    index: &mut StreetIndex,
) {
    eprintln!("  Building street index...");
    let mut skipped = 0;

    for street in street_ways {
        let coordinates: Vec<Coordinate> = street
            .node_ids
            .iter()
            .filter_map(|&nid| nodes_coords.get(nid))
            .collect();

        if coordinates.len() >= 2 {
            index.add_street(&street.name, &coordinates);
        } else {
            skipped += 1;
        }
    }

    if skipped > 0 {
        eprintln!(
            "  Warning: Skipped {} streets due to missing node coordinates",
            skipped
        );
    }
}

pub(crate) fn build_address_polygon_index(
    addressed_ways: &[AddressWayData],
    nodes_coords: &CoordinateStore,
    index: &mut AddressPolygonIndex,
) {
    eprintln!("  Building address polygon index...");
    for way in addressed_ways {
        let ring: Vec<Coordinate> = way
            .node_ids
            .iter()
            .filter_map(|&nid| nodes_coords.get(nid))
            .collect();
        if ring.len() >= 3 {
            index.add_polygon(&way.street, way.housenumber.as_deref(), &ring, way.way_id);
        }
    }
}
