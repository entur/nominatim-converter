//! Entrance/gate point enrichment for large area features.
//!
//! Large OSM areas (military camps, parks, campuses, quarries) are emitted with the polygon
//! centroid as their coordinate, which is a poor routing destination -- it can sit deep inside
//! an inaccessible area. Where a gate/entrance node exists on the feature's perimeter we
//! substitute that node's coordinate instead, so the returned point is a usable routing origin.
//!
//! This module is pure data + logic (selecting the best gate among a feature's member nodes).
//! The PBF scanning that populates [`EntranceData`] lives in `pass4.rs`; the centroid override
//! that consumes the result lives in `entity.rs`.
//!
//! ## Per-feature selection
//! Each eligible feature looks at the entrance candidate nodes among its own perimeter and picks
//! one by this priority (see [`select_entrance_for_feature`]):
//! 1. an explicit `*=main` marker (`routing:entrance=main` or `entrance=main`)
//! 2. a pedestrian `entrance=*` / `routing:entrance=*` node (preferred over a vehicle gate, since
//!    most arrivals are on foot / via transit)
//! 3. a `barrier=*` gate node
//! 4. a routable gate (member of a `highway=*` way)
//! 5. the gate on the most major road
//!
//! Ties are broken by the smaller node id, for determinism.
//!
//! Selecting per-feature (rather than assigning each gate to a single "canonical" parent) means
//! every co-named overlapping parcel that physically contains the gate gets enriched, instead of
//! just one of them.

use std::collections::HashMap;

use super::coordinate::Coordinate;
use super::street::HIGHWAY_TYPES;

/// The entrance-relevant tags of a candidate node.
#[derive(Default, Clone)]
pub(crate) struct EntranceNodeTags {
    pub(crate) entrance: Option<String>,
    pub(crate) barrier: Option<String>,
    pub(crate) routing_entrance: Option<String>,
}

/// All inputs the selector needs, collected during pass 4.
#[derive(Default)]
pub(crate) struct EntranceData {
    /// Candidate node coordinates, keyed by node id.
    pub(crate) coords: HashMap<i64, Coordinate>,
    /// Candidate node entrance tags, keyed by node id.
    pub(crate) tags: HashMap<i64, EntranceNodeTags>,
    /// Highway ways (those touching a candidate node), keyed by way id -> highway type. Used
    /// only to decide routability and road majorness.
    pub(crate) highway_ways: HashMap<i64, String>,
    /// Reverse index: candidate node id -> the highway way ids it belongs to.
    pub(crate) node_highways: HashMap<i64, Vec<i64>>,
}

/// The chosen entrance for one feature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EntrancePoint {
    pub(crate) node_id: i64,
    pub(crate) coord: Coordinate,
}

/// Barrier values that represent a passable entry point (a gate), not a solid enclosure.
const GATE_BARRIERS: &[&str] = &[
    "gate",
    "lift_gate",
    "swing_gate",
    "bollard",
    "cycle_barrier",
    "kissing_gate",
    "block",
    "chain",
];

/// Classify a node by its tags. Returns `Some` if it is a candidate entrance/gate node.
/// `entrance=no` is the explicit negation of an entrance and is never a candidate on its own.
pub(crate) fn is_entrance_candidate(tags: &HashMap<&str, &str>) -> Option<EntranceNodeTags> {
    let entrance = tags
        .get("entrance")
        .filter(|v| **v != "no")
        .map(|s| s.to_string());
    let routing_entrance = tags.get("routing:entrance").map(|s| s.to_string());
    let barrier = tags
        .get("barrier")
        .filter(|v| GATE_BARRIERS.contains(v))
        .map(|s| s.to_string());

    if entrance.is_some() || routing_entrance.is_some() || barrier.is_some() {
        Some(EntranceNodeTags { entrance, barrier, routing_entrance })
    } else {
        None
    }
}

/// Returns the highway type of a way if it is a routable highway we track, else `None`.
pub(crate) fn highway_type(tags: &HashMap<&str, &str>) -> Option<String> {
    tags.get("highway")
        .filter(|h| HIGHWAY_TYPES.contains(h))
        .map(|s| s.to_string())
}

/// True if the node sits on a routable highway (member of a tracked `highway=*` way).
fn is_routable(node_id: i64, data: &EntranceData) -> bool {
    data.node_highways
        .get(&node_id)
        .is_some_and(|ways| ways.iter().any(|w| data.highway_ways.contains_key(w)))
}

/// Index of the most major highway this node sits on (lower = more major), if any.
fn highway_majorness(node_id: i64, data: &EntranceData) -> Option<usize> {
    data.node_highways.get(&node_id)?.iter()
        .filter_map(|w| data.highway_ways.get(w))
        .filter_map(|h| HIGHWAY_TYPES.iter().position(|t| t == h))
        .min()
}

/// Pick the best entrance among the candidate nodes that are members of this feature. Returns
/// `None` if the feature contains no candidate entrance node. Ties on score are broken by the
/// smaller node id for determinism.
pub(crate) fn select_entrance_for_feature(
    member_node_ids: &[i64],
    data: &EntranceData,
) -> Option<EntrancePoint> {
    member_node_ids
        .iter()
        .filter_map(|&nid| {
            let node_tags = data.tags.get(&nid)?;
            let &coord = data.coords.get(&nid)?;
            // Ranking key (higher is better): an explicit "main" marker, then a pedestrian
            // entrance node (entrance=*/routing:entrance=*) over a vehicle barrier gate, then a
            // barrier=* gate, then routable (on a highway), then the most major road. Pedestrian
            // entrances are preferred because most arrivals here are on foot / via transit.
            let is_main = node_tags.routing_entrance.as_deref() == Some("main")
                || node_tags.entrance.as_deref() == Some("main");
            let is_pedestrian_entrance =
                node_tags.entrance.is_some() || node_tags.routing_entrance.is_some();
            let is_barrier_gate = node_tags.barrier.is_some();
            let routable = is_routable(nid, data);
            let road_pref = highway_majorness(nid, data)
                .map(|m| (HIGHWAY_TYPES.len() - m) as i64)
                .unwrap_or(0);
            let score = (
                u8::from(is_main),
                u8::from(is_pedestrian_entrance),
                u8::from(is_barrier_gate),
                u8::from(routable),
                road_pref,
            );
            Some((score, nid, EntrancePoint { node_id: nid, coord }))
        })
        // Highest score wins; on equal score the smaller node id wins.
        .max_by(|a, b| (a.0, std::cmp::Reverse(a.1)).cmp(&(b.0, std::cmp::Reverse(b.1))))
        .map(|(_, _, point)| point)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(lat: f64, lon: f64) -> Coordinate {
        Coordinate { lat, lon }
    }

    /// Build the Terningmoen (Elverum) topology, matching test-data/terningmoen.osm:
    /// gate node/1240473681 (barrier=lift_gate) at 60.8750/11.5600 is a member of the fence
    /// 518127311, the landuse parcel 518428220, and the highway=unclassified roads
    /// 845258844 + 845260051.
    fn terningmoen() -> EntranceData {
        let gate = 1240473681_i64;
        let mut data = EntranceData::default();
        data.coords.insert(gate, coord(60.8750, 11.5600));
        data.tags.insert(
            gate,
            EntranceNodeTags { barrier: Some("lift_gate".into()), ..Default::default() },
        );
        data.highway_ways.insert(845258844, "unclassified".into());
        data.highway_ways.insert(845260051, "unclassified".into());
        data.node_highways.insert(gate, vec![845258844, 845260051]);
        data
    }

    #[test]
    fn fence_member_gate_is_selected() {
        let data = terningmoen();
        // Fence 518127311 perimeter includes the gate.
        let fence_nodes = vec![1001, 1002, 1240473681, 1003, 1004, 1001];
        let ep = select_entrance_for_feature(&fence_nodes, &data).expect("gate selected");
        assert_eq!(ep.node_id, 1240473681);
        assert!((ep.coord.lat - 60.8750).abs() < 1e-9);
        assert!((ep.coord.lon - 11.5600).abs() < 1e-9);
    }

    #[test]
    fn co_named_parcel_containing_gate_also_selects_it() {
        let data = terningmoen();
        // Landuse parcel 518428220 also contains the gate -> it must select it too.
        let landuse_nodes = vec![1002, 1240473681, 1005, 1006, 1003];
        let ep = select_entrance_for_feature(&landuse_nodes, &data).expect("gate selected");
        assert_eq!(ep.node_id, 1240473681);
    }

    #[test]
    fn feature_without_member_gate_returns_none() {
        let data = terningmoen();
        // Outer parcel 50537344 does not contain the gate.
        let outer_nodes = vec![3001, 3002, 3003, 3004, 3001];
        assert!(select_entrance_for_feature(&outer_nodes, &data).is_none());
    }

    #[test]
    fn routing_entrance_main_wins_over_other_gates() {
        let mut data = EntranceData::default();
        let main_gate = 1_i64;
        let other_gate = 2_i64;
        data.coords.insert(main_gate, coord(60.0, 11.0));
        data.coords.insert(other_gate, coord(60.1, 11.1));
        data.tags.insert(
            main_gate,
            EntranceNodeTags { barrier: Some("gate".into()), routing_entrance: Some("main".into()), ..Default::default() },
        );
        data.tags.insert(other_gate, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        // The "other" gate is also on a highway (routable); main must still win.
        data.highway_ways.insert(600, "residential".into());
        data.node_highways.insert(other_gate, vec![600]);

        let ep = select_entrance_for_feature(&[main_gate, other_gate], &data).unwrap();
        assert_eq!(ep.node_id, main_gate);
    }

    #[test]
    fn pedestrian_entrance_beats_barrier_gate() {
        let mut data = EntranceData::default();
        let foot = 1_i64; // pedestrian entrance, not on a road
        let gate = 2_i64; // vehicle barrier gate, on a road
        data.coords.insert(foot, coord(60.0, 11.0));
        data.coords.insert(gate, coord(60.1, 11.1));
        data.tags.insert(foot, EntranceNodeTags { entrance: Some("yes".into()), ..Default::default() });
        data.tags.insert(gate, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        // The vehicle gate is even routable, but the pedestrian entrance must still win.
        data.highway_ways.insert(600, "service".into());
        data.node_highways.insert(gate, vec![600]);

        let ep = select_entrance_for_feature(&[foot, gate], &data).unwrap();
        assert_eq!(ep.node_id, foot);
    }

    #[test]
    fn routable_gate_beats_non_routable_when_no_main() {
        let mut data = EntranceData::default();
        let routable = 1_i64;
        let bare = 2_i64;
        data.coords.insert(routable, coord(60.0, 11.0));
        data.coords.insert(bare, coord(60.1, 11.1));
        data.tags.insert(routable, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.tags.insert(bare, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.highway_ways.insert(600, "tertiary".into());
        data.node_highways.insert(routable, vec![600]);

        let ep = select_entrance_for_feature(&[routable, bare], &data).unwrap();
        assert_eq!(ep.node_id, routable);
    }

    #[test]
    fn is_entrance_candidate_classifies_tags() {
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "lift_gate")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "main")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("routing:entrance", "main")])).is_some());
        // barrier=wall is a solid enclosure, not a passable gate.
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "wall")])).is_none());
        // entrance=no is the negation of an entrance.
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "no")])).is_none());
        // ...unless an independent gate tag is also present.
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "no"), ("barrier", "gate")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("amenity", "cafe")])).is_none());
    }

    #[test]
    fn highway_type_filters_to_tracked_types() {
        assert_eq!(highway_type(&HashMap::from([("highway", "unclassified")])), Some("unclassified".into()));
        assert_eq!(highway_type(&HashMap::from([("highway", "proposed")])), None);
        assert_eq!(highway_type(&HashMap::from([("landuse", "military")])), None);
    }
}
