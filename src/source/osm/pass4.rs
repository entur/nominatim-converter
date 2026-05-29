use std::collections::{HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader};

use crate::target::nominatim_place::NominatimPlace;

use super::coordinate::{Coordinate, CoordinateStore};
use super::entity::OsmEntityConverter;
use super::entrance::{self, EntranceData, EntrancePoint, highway_type, is_entrance_candidate};
use super::geometry::calculate_centroid;
use super::popularity::OsmPopularityCalculator;

/// Minimum feature size (longer bbox side, in metres) for entrance handling to apply. Smaller
/// features are emitted unchanged regardless of useEntrance.
const MIN_AREA_SIZE_METERS: f64 = 150.0;

// ---------------------------------------------------------------------------
// Pass 4 intermediate data structures
//
// These hold the POI data collected during the final PBF scan. We store owned
// Strings (not references) because the PBF reader only lends data for the
// duration of each element callback.
// ---------------------------------------------------------------------------

/// POI nodes: coordinates and tags for nodes that match a configured filter.
pub(crate) struct NodePoiData {
    pub(crate) ids: Vec<i64>,
    pub(crate) coords: HashMap<i64, Coordinate>,
    pub(crate) tags: HashMap<i64, Vec<(String, String)>>,
}

/// Way data: node lists and tags for ways referenced by POI relations or matching filters directly.
pub(crate) struct WayPassData {
    pub(crate) ids: Vec<i64>,
    pub(crate) way_node_ids: HashMap<i64, Vec<i64>>,
    pub(crate) way_tags: HashMap<i64, Vec<(String, String)>>,
}

/// Relation POI data: member node/way IDs and tags for relations matching filters.
pub(crate) struct RelationPassData {
    pub(crate) ids: Vec<i64>,
    pub(crate) member_node_ids: HashMap<i64, Vec<i64>>,
    pub(crate) member_way_ids: HashMap<i64, Vec<i64>>,
    pub(crate) tags: HashMap<i64, Vec<(String, String)>>,
}

// ---------------------------------------------------------------------------
// Data collection
// ---------------------------------------------------------------------------

pub(crate) fn collect_pass4_data(
    input: &Path,
    all_needed_way_ids: &HashSet<i64>,
    popularity_calculator: &OsmPopularityCalculator,
    enrichment_enabled: bool,
) -> Result<(NodePoiData, WayPassData, RelationPassData, EntranceData), Box<dyn std::error::Error>>
{
    let mut node_data = NodePoiData {
        ids: Vec::new(),
        coords: HashMap::new(),
        tags: HashMap::new(),
    };
    let mut way_data = WayPassData {
        ids: Vec::new(),
        way_node_ids: HashMap::new(),
        way_tags: HashMap::new(),
    };
    let mut rel_data = RelationPassData {
        ids: Vec::new(),
        member_node_ids: HashMap::new(),
        member_way_ids: HashMap::new(),
        tags: HashMap::new(),
    };
    // Entrance enrichment relies on the standard PBF ordering (all nodes precede all ways), so
    // candidate nodes are already collected by the time we scan ways and can resolve parentage.
    let mut entrance_data = EntranceData::default();

    let reader = ElementReader::from_path(input)?;
    reader.for_each(|element| {
        match element {
            // Collect each node's tags once and share them with both collectors -- on a national
            // PBF this avoids a second HashMap allocation per node when enrichment is enabled.
            Element::Node(node) => {
                let tags: HashMap<&str, &str> = node.tags().collect();
                collect_poi_node(node.id(), node.lat(), node.lon(), &tags, popularity_calculator, &mut node_data);
                if enrichment_enabled {
                    collect_entrance_node(node.id(), node.lat(), node.lon(), &tags, &mut entrance_data);
                }
            }
            Element::DenseNode(node) => {
                let tags: HashMap<&str, &str> = node.tags().collect();
                collect_poi_node(node.id, node.lat(), node.lon(), &tags, popularity_calculator, &mut node_data);
                if enrichment_enabled {
                    collect_entrance_node(node.id, node.lat(), node.lon(), &tags, &mut entrance_data);
                }
            }
            Element::Way(way) => {
                collect_way(&way, all_needed_way_ids, &mut way_data);
                if enrichment_enabled {
                    collect_entrance_highway(&way, &mut entrance_data);
                }
            }
            Element::Relation(relation) => {
                collect_poi_relation(&relation, popularity_calculator, &mut rel_data);
            }
        }
    })?;

    Ok((node_data, way_data, rel_data, entrance_data))
}

fn collect_poi_node(
    id: i64,
    lat: f64,
    lon: f64,
    tags: &HashMap<&str, &str>,
    popularity_calculator: &OsmPopularityCalculator,
    node_data: &mut NodePoiData,
) {
    if is_poi_entity(tags, popularity_calculator) {
        let owned_tags: Vec<(String, String)> =
            tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        node_data.ids.push(id);
        node_data.coords.insert(id, Coordinate { lat, lon });
        node_data.tags.insert(id, owned_tags);
    }
}

/// Record an entrance/barrier/routing:entrance candidate node (coord + relevant tags). Done for
/// every such node, not just POI-way members, so the gate is captured regardless of which
/// co-named parcel a name search later resolves to.
fn collect_entrance_node(
    id: i64,
    lat: f64,
    lon: f64,
    tags: &HashMap<&str, &str>,
    entrance_data: &mut EntranceData,
) {
    if let Some(node_tags) = is_entrance_candidate(tags) {
        entrance_data.coords.insert(id, Coordinate { lat, lon });
        entrance_data.tags.insert(id, node_tags);
    }
}

/// Record a highway way if it shares a node with a known candidate, building the node -> highway
/// ways reverse index used to decide a gate's routability. Bounded in size: only highway ways
/// that actually touch a gate are retained.
///
/// Invariant: this depends on standard PBF ordering (all nodes precede all ways) so that
/// `entrance_data.coords` is fully populated before any way is scanned. On out-of-order input the
/// failure mode is graceful -- a gate may be mis-ranked as non-routable, never data corruption.
fn collect_entrance_highway(way: &osmpbf::Way, entrance_data: &mut EntranceData) {
    let tags: HashMap<&str, &str> = way.tags().collect();
    let Some(highway) = highway_type(&tags) else {
        return;
    };
    let touched: Vec<i64> = way
        .refs()
        .filter(|nid| entrance_data.coords.contains_key(nid))
        .collect();
    if touched.is_empty() {
        return;
    }
    entrance_data.highway_ways.insert(way.id(), highway);
    for nid in touched {
        entrance_data.node_highways.entry(nid).or_default().push(way.id());
    }
}

fn collect_way(
    way: &osmpbf::Way,
    all_needed_way_ids: &HashSet<i64>,
    way_data: &mut WayPassData,
) {
    if all_needed_way_ids.contains(&way.id()) {
        let node_ids: Vec<i64> = way.refs().collect();
        way_data.ids.push(way.id());
        way_data.way_node_ids.insert(way.id(), node_ids);
        let owned_tags: Vec<(String, String)> =
            way.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        way_data.way_tags.insert(way.id(), owned_tags);
    }
}

fn collect_poi_relation(
    relation: &osmpbf::Relation,
    popularity_calculator: &OsmPopularityCalculator,
    rel_data: &mut RelationPassData,
) {
    let tags: HashMap<&str, &str> = relation.tags().collect();
    if !is_poi_entity(&tags, popularity_calculator) {
        return;
    }

    let mut member_nodes = Vec::new();
    let mut member_ways = Vec::new();
    for member in relation.members() {
        match member.member_type {
            osmpbf::RelMemberType::Node => {
                member_nodes.push(member.member_id);
            }
            osmpbf::RelMemberType::Way => {
                member_ways.push(member.member_id);
            }
            _ => {}
        }
    }
    rel_data.ids.push(relation.id());
    rel_data.member_node_ids.insert(relation.id(), member_nodes);
    rel_data.member_way_ids.insert(relation.id(), member_ways);
    let owned_tags: Vec<(String, String)> = relation
        .tags()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    rel_data.tags.insert(relation.id(), owned_tags);
}

// ---------------------------------------------------------------------------
// Entrance point selection (worth-enriching gate + coverage measurement)
// ---------------------------------------------------------------------------

/// The centroid overrides produced by entrance handling, keyed separately for ways and relations
/// (a way id and a relation id can collide numerically).
#[derive(Default)]
pub(crate) struct EntranceOverrides {
    pub(crate) way_points: HashMap<i64, EntrancePoint>,
    pub(crate) rel_points: HashMap<i64, EntrancePoint>,
}

#[derive(Default)]
struct CoverageStats {
    eligible: usize,
    below_threshold: usize,
    distances: Vec<f64>,
}

/// For each large-area POI feature (way or multipolygon relation) whose matched filter sets
/// `useEntrance`, substitute the best entrance among the feature's own perimeter nodes for the
/// centroid where one exists. Features without a gate keep their centroid.
///
/// Also logs a coverage summary (eligible/below-threshold counts and the centroid->entrance
/// distance distribution).
pub(crate) fn compute_entrance_overrides(
    entrance_data: &EntranceData,
    way_data: &WayPassData,
    rel_data: &RelationPassData,
    poi_way_ids: &HashSet<i64>,
    nodes_coords: &CoordinateStore,
    way_centroids: &CoordinateStore,
    popularity_calculator: &OsmPopularityCalculator,
) -> EntranceOverrides {
    let mut o = EntranceOverrides::default();
    let mut stats = CoverageStats::default();

    // --- Ways ---
    for &way_id in &way_data.ids {
        if !poi_way_ids.contains(&way_id) {
            continue;
        }
        let Some(owned_tags) = way_data.way_tags.get(&way_id) else {
            continue;
        };
        let tags: HashMap<&str, &str> = owned_tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        if !popularity_calculator.use_entrance(&tags) {
            continue;
        }
        let Some(node_ids) = way_data.way_node_ids.get(&way_id) else {
            continue;
        };
        if !size_ok(bbox_size_meters(node_ids, nodes_coords), &mut stats) {
            continue;
        }
        stats.eligible += 1;
        if let Some(ep) = entrance::select_entrance_for_feature(node_ids, entrance_data) {
            if let Some(centroid) = way_centroids.get(way_id) {
                stats.distances.push(meters_between(&centroid, &ep.coord));
            }
            o.way_points.insert(way_id, ep);
        }
    }

    // --- Multipolygon relations ---
    for &rel_id in &rel_data.ids {
        let Some(owned_tags) = rel_data.tags.get(&rel_id) else {
            continue;
        };
        let tags: HashMap<&str, &str> = owned_tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        if !popularity_calculator.use_entrance(&tags) {
            continue;
        }
        let perimeter = relation_perimeter_nodes(rel_id, rel_data, way_data);
        if !size_ok(bbox_size_meters(&perimeter, nodes_coords), &mut stats) {
            continue;
        }
        stats.eligible += 1;
        if let Some(ep) = entrance::select_entrance_for_feature(&perimeter, entrance_data) {
            if let Some(centroid) = relation_centroid(rel_id, rel_data, nodes_coords, way_centroids) {
                stats.distances.push(meters_between(&centroid, &ep.coord));
            }
            o.rel_points.insert(rel_id, ep);
        }
    }

    log_coverage(&mut stats);
    o
}

/// The perimeter node ids of a relation feature: its direct node members plus the nodes of all
/// its member ways (outer/inner rings).
fn relation_perimeter_nodes(
    rel_id: i64,
    rel_data: &RelationPassData,
    way_data: &WayPassData,
) -> Vec<i64> {
    let mut nodes: Vec<i64> = rel_data.member_node_ids.get(&rel_id).cloned().unwrap_or_default();
    if let Some(way_ids) = rel_data.member_way_ids.get(&rel_id) {
        for wid in way_ids {
            if let Some(wn) = way_data.way_node_ids.get(wid) {
                nodes.extend(wn.iter().copied());
            }
        }
    }
    nodes
}

/// The centroid of a relation, mirroring how `convert_relation` computes it: the mean of its
/// member node coordinates and member way centroids. Used only for the distance summary.
fn relation_centroid(
    rel_id: i64,
    rel_data: &RelationPassData,
    nodes_coords: &CoordinateStore,
    way_centroids: &CoordinateStore,
) -> Option<Coordinate> {
    let mut coords: Vec<Coordinate> = Vec::new();
    if let Some(nids) = rel_data.member_node_ids.get(&rel_id) {
        coords.extend(nids.iter().filter_map(|&n| nodes_coords.get(n)));
    }
    if let Some(wids) = rel_data.member_way_ids.get(&rel_id) {
        coords.extend(wids.iter().filter_map(|&w| way_centroids.get(w)));
    }
    calculate_centroid(&coords)
}

/// True if the feature is large enough for entrance handling. Counts a sub-threshold feature
/// (one with geometry but below the size floor) so the coverage log can distinguish it from a
/// feature with no usable geometry.
fn size_ok(size: Option<f64>, stats: &mut CoverageStats) -> bool {
    match size {
        Some(s) if s >= MIN_AREA_SIZE_METERS => true,
        Some(_) => {
            stats.below_threshold += 1;
            false
        }
        None => false,
    }
}

/// Size in metres of a set of nodes' bounding box along its longer side. Converts degrees to
/// metres so the threshold is latitude-independent.
fn bbox_size_meters(node_ids: &[i64], nodes_coords: &CoordinateStore) -> Option<f64> {
    let coords: Vec<Coordinate> = node_ids
        .iter()
        .filter_map(|&nid| nodes_coords.get(nid))
        .collect();
    let bbox = super::geometry::BoundingBox::from_coordinates(&coords)?;
    let mid_lat = (bbox.min_lat + bbox.max_lat) / 2.0;
    let height_m = (bbox.max_lat - bbox.min_lat) * 111_000.0;
    let width_m = (bbox.max_lon - bbox.min_lon) * 111_000.0 * mid_lat.to_radians().cos();
    Some(height_m.max(width_m))
}

/// Approximate distance in metres between two coordinates (equirectangular projection).
fn meters_between(a: &Coordinate, b: &Coordinate) -> f64 {
    let lat_scale = 111_000.0;
    let lon_scale = 111_000.0 * a.lat.to_radians().cos();
    let dx = (b.lon - a.lon) * lon_scale;
    let dy = (b.lat - a.lat) * lat_scale;
    (dx * dx + dy * dy).sqrt()
}

fn log_coverage(stats: &mut CoverageStats) {
    let CoverageStats { eligible, below_threshold, distances } = stats;
    let enriched = distances.len();
    if *eligible == 0 {
        eprintln!(
            "  Entrance enrichment: no eligible large-area features (>= {MIN_AREA_SIZE_METERS:.0} m); \
             {below_threshold} matched but were below the size threshold"
        );
        return;
    }
    distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> f64 {
        if distances.is_empty() {
            return 0.0;
        }
        let idx = ((distances.len() as f64 - 1.0) * p).round() as usize;
        distances[idx]
    };
    eprintln!(
        "  Entrance enrichment: {eligible} eligible features (>= {MIN_AREA_SIZE_METERS:.0} m, \
         {below_threshold} more below threshold); {enriched} centroid->entrance substitutions"
    );
    if enriched > 0 {
        eprintln!(
            "    centroid->entrance distance (m): min={:.0} median={:.0} p90={:.0} max={:.0}",
            distances.first().copied().unwrap_or(0.0),
            pct(0.5),
            pct(0.9),
            distances.last().copied().unwrap_or(0.0),
        );
    }
}

// ---------------------------------------------------------------------------
// Centroid computation
// ---------------------------------------------------------------------------

pub(crate) fn compute_way_centroids(
    way_data: &WayPassData,
    nodes_coords: &CoordinateStore,
    way_centroids: &mut CoordinateStore,
) {
    for &way_id in &way_data.ids {
        if let Some(node_ids) = way_data.way_node_ids.get(&way_id) {
            let way_node_coords: Vec<Coordinate> = node_ids
                .iter()
                .filter_map(|&nid| nodes_coords.get(nid))
                .collect();
            if let Some(centroid) = calculate_centroid(&way_node_coords) {
                way_centroids.put(way_id, centroid);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entity conversion
// ---------------------------------------------------------------------------

pub(crate) fn convert_poi_nodes(
    node_data: &NodePoiData,
    converter: &mut OsmEntityConverter,
    results: &mut Vec<NominatimPlace>,
) {
    for &node_id in &node_data.ids {
        if let (Some(&coord), Some(owned_tags)) =
            (node_data.coords.get(&node_id), node_data.tags.get(&node_id))
        {
            let tags: HashMap<&str, &str> = owned_tags
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            if let Some(place) =
                converter.convert_node(node_id, coord.lat, coord.lon, &tags)
            {
                results.push(place);
            }
        }
    }
}

pub(crate) fn convert_poi_ways(
    way_data: &WayPassData,
    poi_way_ids: &HashSet<i64>,
    converter: &mut OsmEntityConverter,
    results: &mut Vec<NominatimPlace>,
) {
    for &way_id in &way_data.ids {
        if poi_way_ids.contains(&way_id)
            && let Some(owned_tags) = way_data.way_tags.get(&way_id) {
                let tags: HashMap<&str, &str> = owned_tags
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                if let Some(place) = converter.convert_way(way_id, &tags) {
                    results.push(place);
                }
            }
    }
}

pub(crate) fn convert_poi_relations(
    rel_data: &RelationPassData,
    converter: &mut OsmEntityConverter,
    results: &mut Vec<NominatimPlace>,
) {
    for &rel_id in &rel_data.ids {
        if let Some(owned_tags) = rel_data.tags.get(&rel_id) {
            let tags: HashMap<&str, &str> = owned_tags
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let member_nodes = rel_data
                .member_node_ids
                .get(&rel_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let member_ways = rel_data
                .member_way_ids
                .get(&rel_id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            if let Some(place) =
                converter.convert_relation(rel_id, member_nodes, member_ways, &tags)
            {
                results.push(place);
            }
        }
    }
}

/// Check if an entity has a name and at least one matching filter tag.
fn is_poi_entity(
    tags: &HashMap<&str, &str>,
    popularity_calculator: &OsmPopularityCalculator,
) -> bool {
    tags.contains_key("name")
        && tags
            .iter()
            .any(|(k, v)| popularity_calculator.has_filter(k, v))
}
