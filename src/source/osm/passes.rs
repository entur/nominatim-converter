use std::collections::{HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader};

use crate::common::importance::ImportanceCalculator;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_place::NominatimPlace;

use super::address_index::{AddressNodeIndex, AddressPolygonIndex};
use super::admin::{ADMIN_LEVEL_COUNTY, ADMIN_LEVEL_MUNICIPALITY, AdministrativeBoundaryIndex};
use super::coordinate::{Coordinate, CoordinateStore};
use super::entity::{OsmEntityConverter, extract_country_code};
use super::indexing::{
    AddressWayData, AdminRelationData, StreetWayData, build_address_polygon_index,
    build_admin_boundary_index, build_street_index,
};
use super::pass4;
use super::popularity::OsmPopularityCalculator;
use super::street::{StreetIndex, HIGHWAY_TYPES};

/// Data collected in pass 1 (relations): admin boundaries and POI relation members.
pub(crate) struct Pass1Result {
    pub admin_relations: Vec<AdminRelationData>,
    pub poi_relation_member_way_ids: HashSet<i64>,
    pub poi_relation_node_ids: HashSet<i64>,
}

/// Data collected in pass 2 (ways): streets, POI ways, addressed polygons, and the node IDs
/// needed for pass 3.
pub(crate) struct Pass2Result {
    pub street_ways: Vec<StreetWayData>,
    pub poi_way_ids: HashSet<i64>,
    pub needed_node_ids: HashSet<i64>,
    pub admin_way_node_ids: HashMap<i64, Vec<i64>>,
    pub addressed_ways: Vec<AddressWayData>,
}

// ---------------------------------------------------------------------------
// OsmConverter -- main 4-pass converter
// ---------------------------------------------------------------------------

pub struct OsmConverter {
    config: Config,
}

impl OsmConverter {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Convert an OSM PBF file to Nominatim NDJSON.
    pub fn convert(
        &self,
        input: &Path,
        output: &Path,
        is_appending: bool,
        usage: &crate::common::usage::UsageBoost,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        assert!(input.exists(), "Input file does not exist: {:?}", input);

        let mut nodes_coords = CoordinateStore::new(500_000);
        let mut way_centroids = CoordinateStore::new(50_000);
        let mut admin_boundary_index = AdministrativeBoundaryIndex::new();
        let mut street_index = StreetIndex::new();
        let mut address_polygon_index = AddressPolygonIndex::new();
        let mut address_node_index = AddressNodeIndex::new();
        let popularity_calculator = OsmPopularityCalculator::new(&self.config);

        let p1 = self.pass1_relations(input, &popularity_calculator)?;

        let p2 = self.pass2_ways(
            input,
            &p1.admin_relations,
            &p1.poi_relation_member_way_ids,
            &p1.poi_relation_node_ids,
            &popularity_calculator,
        )?;

        Self::pass3_nodes(
            input,
            &p2.needed_node_ids,
            &mut nodes_coords,
            &mut address_node_index,
        )?;

        build_admin_boundary_index(
            &p1.admin_relations,
            &p2.admin_way_node_ids,
            &nodes_coords,
            &mut admin_boundary_index,
        );
        eprintln!("  {}", admin_boundary_index.get_statistics());

        build_street_index(&p2.street_ways, &nodes_coords, &mut street_index);
        eprintln!("  {}", street_index.get_statistics());

        build_address_polygon_index(&p2.addressed_ways, &nodes_coords, &mut address_polygon_index);
        eprintln!("  {}", address_polygon_index.get_statistics());
        eprintln!("  {}", address_node_index.get_statistics());

        let results = self.pass4_convert(
            input,
            &p2.poi_way_ids,
            &p1.poi_relation_member_way_ids,
            &nodes_coords,
            &mut way_centroids,
            &mut admin_boundary_index,
            &street_index,
            &address_polygon_index,
            &address_node_index,
            &popularity_calculator,
            usage,
        )?;

        eprintln!("Finished processing {} entities", results.len());
        let count = JsonWriter::export(&results, output, is_appending)?;

        Ok(count)
    }

    /// Pass 1: Relations -- collect admin boundaries and POI relation member IDs.
    fn pass1_relations(
        &self,
        input: &Path,
        popularity_calculator: &OsmPopularityCalculator,
    ) -> Result<Pass1Result, Box<dyn std::error::Error>> {
        eprintln!("Pass 1/4: Scanning relations for admin boundaries and POI relations...");

        let mut admin_relations: Vec<AdminRelationData> = Vec::new();
        let mut poi_relation_member_way_ids: HashSet<i64> = HashSet::new();
        let mut poi_relation_node_ids: HashSet<i64> = HashSet::new();

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            if let Element::Relation(relation) = element {
                let tags: HashMap<&str, &str> = relation.tags().collect();

                collect_admin_relation(
                    &relation,
                    &tags,
                    &mut admin_relations,
                    &mut poi_relation_node_ids,
                );

                collect_poi_relation_members(
                    &relation,
                    &tags,
                    popularity_calculator,
                    &mut poi_relation_member_way_ids,
                    &mut poi_relation_node_ids,
                );
            }
        })?;

        eprintln!(
            "  Found {} admin boundary relations",
            admin_relations.len()
        );
        eprintln!(
            "  Found {} POI relation member ways",
            poi_relation_member_way_ids.len()
        );

        Ok(Pass1Result { admin_relations, poi_relation_member_way_ids, poi_relation_node_ids })
    }

    /// Pass 2: Ways -- collect all required node IDs and way metadata.
    fn pass2_ways(
        &self,
        input: &Path,
        admin_relations: &[AdminRelationData],
        poi_relation_member_way_ids: &HashSet<i64>,
        poi_relation_node_ids: &HashSet<i64>,
        popularity_calculator: &OsmPopularityCalculator,
    ) -> Result<Pass2Result, Box<dyn std::error::Error>> {
        eprintln!("Pass 2/4: Scanning ways for streets, admin boundaries, and POIs...");

        let admin_way_ids: HashSet<i64> = admin_relations
            .iter()
            .flat_map(|r| r.way_ids.iter().copied())
            .collect();

        let mut street_ways: Vec<StreetWayData> = Vec::new();
        let mut needed_node_ids: HashSet<i64> = poi_relation_node_ids.clone();
        let mut poi_way_ids: HashSet<i64> = HashSet::new();
        let mut admin_way_node_ids: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut addressed_ways: Vec<AddressWayData> = Vec::new();

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            if let Element::Way(way) = element {
                let tags: HashMap<&str, &str> = way.tags().collect();
                let node_ids: Vec<i64> = way.refs().collect();

                process_way(
                    &way,
                    &tags,
                    &node_ids,
                    &admin_way_ids,
                    poi_relation_member_way_ids,
                    popularity_calculator,
                    &mut street_ways,
                    &mut needed_node_ids,
                    &mut poi_way_ids,
                    &mut admin_way_node_ids,
                    &mut addressed_ways,
                );
            }
        })?;

        eprintln!("  Found {} street ways", street_ways.len());
        eprintln!("  Found {} POI ways", poi_way_ids.len());
        eprintln!("  Found {} addressed polygons", addressed_ways.len());
        eprintln!(
            "  Total unique node coordinates needed: {}",
            needed_node_ids.len()
        );

        Ok(Pass2Result {
            street_ways,
            needed_node_ids,
            poi_way_ids,
            admin_way_node_ids,
            addressed_ways,
        })
    }

    /// Pass 3: Nodes -- collect coordinates for all needed nodes, and index standalone
    /// address nodes (any node with `addr:street` + `addr:housenumber`, whether or not it is
    /// otherwise needed) for address inheritance.
    fn pass3_nodes(
        input: &Path,
        needed_node_ids: &HashSet<i64>,
        nodes_coords: &mut CoordinateStore,
        address_node_index: &mut AddressNodeIndex,
    ) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("Pass 3/4: Collecting node coordinates...");

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            match element {
                Element::Node(node) => {
                    let coord = Coordinate { lat: node.lat(), lon: node.lon() };
                    if needed_node_ids.contains(&node.id()) {
                        nodes_coords.put(node.id(), coord);
                    }
                    collect_address_node(node.id(), coord, node.tags(), address_node_index);
                }
                Element::DenseNode(node) => {
                    let coord = Coordinate { lat: node.lat(), lon: node.lon() };
                    if needed_node_ids.contains(&node.id) {
                        nodes_coords.put(node.id, coord);
                    }
                    collect_address_node(node.id, coord, node.tags(), address_node_index);
                }
                _ => {}
            }
        })?;

        eprintln!("  Building administrative boundary index...");
        Ok(())
    }

    /// Pass 4: Read PBF again, collect POI data, compute centroids, and convert.
    #[allow(clippy::too_many_arguments)]
    fn pass4_convert(
        &self,
        input: &Path,
        poi_way_ids: &HashSet<i64>,
        poi_relation_member_way_ids: &HashSet<i64>,
        nodes_coords: &CoordinateStore,
        way_centroids: &mut CoordinateStore,
        admin_boundary_index: &mut AdministrativeBoundaryIndex,
        street_index: &StreetIndex,
        address_polygon_index: &AddressPolygonIndex,
        address_node_index: &AddressNodeIndex,
        popularity_calculator: &OsmPopularityCalculator,
        usage: &crate::common::usage::UsageBoost,
    ) -> Result<Vec<NominatimPlace>, Box<dyn std::error::Error>> {
        eprintln!("Pass 4/4: Processing POI entities and writing output...");

        let all_needed_way_ids: HashSet<i64> = poi_way_ids
            .iter()
            .chain(poi_relation_member_way_ids.iter())
            .copied()
            .collect();

        let enrichment_enabled = popularity_calculator.any_entrance_filter();
        let pass4::Pass4Data {
            nodes: node_data,
            ways: way_data,
            rels: rel_data,
            entrances: entrance_data,
        } = pass4::collect_pass4_data(
            input,
            &all_needed_way_ids,
            popularity_calculator,
            enrichment_enabled,
        )?;

        pass4::compute_way_centroids(&way_data, nodes_coords, way_centroids);

        let overrides = if enrichment_enabled {
            pass4::compute_entrance_overrides(
                &entrance_data,
                &way_data,
                &rel_data,
                poi_way_ids,
                nodes_coords,
                way_centroids,
                popularity_calculator,
            )
        } else {
            pass4::EntranceOverrides::default()
        };

        let importance_calc = ImportanceCalculator::new(usage);
        // Resolved once here; the osm section is guaranteed present when converting osm.
        let rank_address = &self
            .config
            .osm
            .as_ref()
            .expect("osm config present when converting osm")
            .rank_address;
        let mut converter = OsmEntityConverter {
            nodes_coords,
            way_centroids,
            admin_boundary_index,
            street_index,
            address_polygon_index,
            address_node_index,
            popularity_calculator,
            importance_calc,
            rank_address,
            way_entrance_points: &overrides.way_points,
            relation_entrance_points: &overrides.rel_points,
        };

        let mut results: Vec<NominatimPlace> = Vec::new();

        pass4::convert_poi_nodes(&node_data, &mut converter, &mut results);
        pass4::convert_poi_ways(&way_data, poi_way_ids, &mut converter, &mut results);
        pass4::convert_poi_relations(&rel_data, &mut converter, &mut results);

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Pass 1 helpers
// ---------------------------------------------------------------------------

fn collect_admin_relation(
    relation: &osmpbf::Relation,
    tags: &HashMap<&str, &str>,
    admin_relations: &mut Vec<AdminRelationData>,
    poi_relation_node_ids: &mut HashSet<i64>,
) {
    if tags.get("boundary") != Some(&"administrative") {
        return;
    }

    if let Some(data) = parse_admin_relation(relation, tags) {
        admin_relations.push(data);
    }

    // Collect node members from admin relations -- unconditionally, even when the relation
    // itself is not a county/municipality boundary we keep.
    for member in relation.members() {
        if member.member_type == osmpbf::RelMemberType::Node {
            poi_relation_node_ids.insert(member.member_id);
        }
    }
}

/// Parse a `boundary=administrative` relation into [`AdminRelationData`]. Returns `None` unless
/// the relation has a parseable county/municipality `admin_level` and all required fields
/// (name, ref, country) are present.
fn parse_admin_relation(
    relation: &osmpbf::Relation,
    tags: &HashMap<&str, &str>,
) -> Option<AdminRelationData> {
    let admin_level: i32 = tags.get("admin_level")?.parse().ok()?;
    if admin_level != ADMIN_LEVEL_COUNTY && admin_level != ADMIN_LEVEL_MUNICIPALITY {
        return None;
    }

    let name = tags.get("name")?.to_string();
    let ref_code = tags.get("ref")?.to_string();
    let country = extract_country_code(tags)?;

    let way_ids: Vec<i64> = relation
        .members()
        .filter(|m| m.member_type == osmpbf::RelMemberType::Way)
        .map(|m| m.member_id)
        .collect();

    Some(AdminRelationData { name, admin_level, ref_code, way_ids, country })
}

fn collect_poi_relation_members(
    relation: &osmpbf::Relation,
    tags: &HashMap<&str, &str>,
    popularity_calculator: &OsmPopularityCalculator,
    poi_relation_member_way_ids: &mut HashSet<i64>,
    poi_relation_node_ids: &mut HashSet<i64>,
) {
    if !popularity_calculator.is_poi(tags) {
        return;
    }

    for member in relation.members() {
        match member.member_type {
            osmpbf::RelMemberType::Way => {
                poi_relation_member_way_ids.insert(member.member_id);
            }
            osmpbf::RelMemberType::Node => {
                poi_relation_node_ids.insert(member.member_id);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 2 helpers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn process_way(
    way: &osmpbf::Way,
    tags: &HashMap<&str, &str>,
    node_ids: &[i64],
    admin_way_ids: &HashSet<i64>,
    poi_relation_member_way_ids: &HashSet<i64>,
    popularity_calculator: &OsmPopularityCalculator,
    street_ways: &mut Vec<StreetWayData>,
    needed_node_ids: &mut HashSet<i64>,
    poi_way_ids: &mut HashSet<i64>,
    admin_way_node_ids: &mut HashMap<i64, Vec<i64>>,
    addressed_ways: &mut Vec<AddressWayData>,
) {
    // Street way? A named road we index for nearest-street lookups.
    if let Some(name) = tags.get("name")
        && is_street_way(tags)
    {
        street_ways.push(StreetWayData {
            name: name.to_string(),
            node_ids: node_ids.to_vec(),
        });
        needed_node_ids.extend(node_ids);
    }

    // Addressed building? A closed `building` way carrying addr:street. Contained POIs
    // inherit its street + housenumber. Non-building addressed areas (landuse, parking, ...)
    // are excluded: one address rarely covers the whole area.
    if let Some(&street) = tags.get("addr:street")
        && tags.get("building").is_some_and(|&v| v != "no")
        && is_closed_way(node_ids)
    {
        addressed_ways.push(AddressWayData {
            street: street.to_string(),
            housenumber: tags.get("addr:housenumber").map(|s| s.to_string()),
            node_ids: node_ids.to_vec(),
            way_id: way.id(),
        });
        needed_node_ids.extend(node_ids);
    }

    // Admin boundary way?
    if admin_way_ids.contains(&way.id()) {
        admin_way_node_ids.insert(way.id(), node_ids.to_vec());
        needed_node_ids.extend(node_ids);
    }

    // POI relation member way?
    if poi_relation_member_way_ids.contains(&way.id()) {
        needed_node_ids.extend(node_ids);
    }

    // Direct POI way?
    if popularity_calculator.is_poi(tags) {
        poi_way_ids.insert(way.id());
        needed_node_ids.extend(node_ids);
    }
}

/// A closed ring: first node repeated as last, at least a triangle.
fn is_closed_way(node_ids: &[i64]) -> bool {
    node_ids.len() >= 4 && node_ids.first() == node_ids.last()
}

/// A way whose name is usable as a street address: an indexed `highway` type at the
/// addressable surface. A road passing under a structure is never a feature's address -- a
/// `tunnel` (e.g. the E18 Operatunnelen under Nasjonalmuseet) or a `covered` way (arcades,
/// avalanche snow-sheds). Bridges are kept: they carry the continuing street name.
fn is_street_way(tags: &HashMap<&str, &str>) -> bool {
    tags.get("highway").is_some_and(|h| HIGHWAY_TYPES.contains(h))
        && !tags.get("tunnel").is_some_and(|&v| v != "no")
        && !tags.get("covered").is_some_and(|&v| v != "no")
}

// ---------------------------------------------------------------------------
// Pass 3 helpers
// ---------------------------------------------------------------------------

/// Index a node as a standalone address source when it carries both `addr:street` and
/// `addr:housenumber` (a full address point, e.g. Kartverket-imported addresses).
fn collect_address_node<'a>(
    id: i64,
    coord: Coordinate,
    tags: impl Iterator<Item = (&'a str, &'a str)>,
    index: &mut AddressNodeIndex,
) {
    let mut street: Option<&str> = None;
    let mut housenumber: Option<&str> = None;
    for (k, v) in tags {
        match k {
            "addr:street" => street = Some(v),
            "addr:housenumber" => housenumber = Some(v),
            _ => {}
        }
    }
    if let (Some(street), Some(housenumber)) = (street, housenumber) {
        index.add_node(coord, street, Some(housenumber), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_closed_way_requires_repeated_first_last() {
        assert!(is_closed_way(&[1, 2, 3, 1]));
        assert!(!is_closed_way(&[1, 2, 3])); // open
        assert!(!is_closed_way(&[1, 2, 1])); // fewer than 4
        assert!(!is_closed_way(&[1, 2, 3, 4])); // not closed
    }

    #[test]
    fn is_street_way_excludes_tunnels_and_non_highways() {
        assert!(is_street_way(&HashMap::from([("highway", "residential")])));
        assert!(is_street_way(&HashMap::from([("highway", "trunk"), ("tunnel", "no")])));
        // A bridge keeps the street name.
        assert!(is_street_way(&HashMap::from([("highway", "primary"), ("bridge", "yes")])));
        // Below the addressable surface: never a street address.
        assert!(!is_street_way(&HashMap::from([("highway", "trunk"), ("tunnel", "yes")])));
        assert!(!is_street_way(&HashMap::from([("highway", "residential"), ("covered", "yes")])));
        // Not an indexed highway type.
        assert!(!is_street_way(&HashMap::from([("highway", "footway")])));
        assert!(!is_street_way(&HashMap::from([("amenity", "hospital")])));
    }

    #[test]
    fn collect_address_node_requires_street_and_housenumber() {
        let coord = Coordinate { lat: 59.9, lon: 10.7 };

        let mut both = AddressNodeIndex::new();
        collect_address_node(
            1,
            coord,
            [("addr:street", "Storgata"), ("addr:housenumber", "3")].into_iter(),
            &mut both,
        );
        assert!(both.find_nearest(&coord).is_some());

        let mut street_only = AddressNodeIndex::new();
        collect_address_node(2, coord, [("addr:street", "Storgata")].into_iter(), &mut street_only);
        assert!(street_only.find_nearest(&coord).is_none());
    }
}
