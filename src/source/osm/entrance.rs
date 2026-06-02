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
//! one by this priority (see [`select_entrance_for_feature`]). Pedestrian access is preferred
//! throughout, since most arrivals here are on foot / via transit:
//! 1. an explicit `*=main` marker (`routing:entrance=main` or `entrance=main`)
//! 2. a public pedestrian `entrance=*` (any allowed value except `service`) / `routing:entrance=*`
//! 3. a pedestrian barrier crossing (`barrier=stile`/`turnstile`/...)
//! 4. a vehicle gate / control passable on foot (`barrier=gate`/`bollard`/`cattle_grid`/...)
//! 5. a `service` (staff/delivery) `entrance=*`, demoted below real gates
//! 6. a routable gate (member of a `highway=*` way), then the gate on the most major road
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

/// Barrier values that exist specifically for people on foot to cross a boundary. The strongest
/// barrier signal for a public-transport geocoder, where the arrival point should be where a
/// pedestrian actually enters: ranked above vehicle barriers by [`select_entrance_for_feature`].
const PEDESTRIAN_BARRIERS: &[&str] = &[
    "stile",
    "kissing_gate",
    "wicket_gate",
    "turnstile",
    "full-height_turnstile",
];

/// Barrier values that are primarily vehicle gates / vehicle-control points but are passable on
/// foot. They mark where a way crosses the perimeter, so they are a usable arrival point, but a
/// weaker signal than a dedicated pedestrian crossing -- ranked below [`PEDESTRIAN_BARRIERS`].
/// Deliberately excluded: `toll_booth`/`border_control` (a payment/crossing point on a
/// through-road, not an entrance to a destination), `sally_port` (secure, non-public),
/// `bus_trap`/`sump_buster`/`height_restrictor` (vehicle filters, not passages), and solid
/// enclosures like `wall`/`fence`/`hedge`.
const VEHICLE_BARRIERS: &[&str] = &[
    "gate",
    "sliding_gate",
    "slide_gate",
    "swing_gate",
    "lift_gate",
    "bollard",
    "block",
    "chain",
    "cycle_barrier",
    "cattle_grid",
];

/// True if `barrier` is a passable entry point we treat as a candidate (either tier).
fn is_gate_barrier(barrier: &str) -> bool {
    PEDESTRIAN_BARRIERS.contains(&barrier) || VEHICLE_BARRIERS.contains(&barrier)
}

/// Entrance values accepted as a public arrival point reachable on foot. An allowlist, so unknown
/// or new values default to *not* a candidate. Deliberately excluded: `garage`/`parking`/`car_wash`
/// (vehicle), `emergency` (emergency-only), `exit` (one-way egress, "not an entrance" per the wiki),
/// and `no` (the negation).
const ARRIVAL_ENTRANCES: &[&str] = &[
    "yes",
    "main",
    "secondary",
    "service",
    "shop",
    "restaurant",
    "home",
    "staircase",
    "entrance",
];

/// Classify a node by its tags. Returns `Some` if it is a candidate entrance/gate node.
pub(crate) fn is_entrance_candidate(tags: &HashMap<&str, &str>) -> Option<EntranceNodeTags> {
    let entrance = tags
        .get("entrance")
        .filter(|v| ARRIVAL_ENTRANCES.contains(v))
        .map(|s| s.to_string());
    // `routing:entrance` is an explicit routing directive, so any value is accepted -- unlike the
    // crowd-sourced `entrance`, which is allowlisted above.
    let routing_entrance = tags.get("routing:entrance").map(|s| s.to_string());
    let barrier = tags
        .get("barrier")
        .filter(|v| is_gate_barrier(v))
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
            // Ranking key (higher is better), tuned for an on-foot / transit arrival:
            //   1. an explicit "main" marker (`routing:entrance=main` / `entrance=main`)
            //   2. a public pedestrian entrance (`entrance=*` except `service`, or any
            //      `routing:entrance=*`)
            //   3. a pedestrian barrier crossing (stile, turnstile, ...)
            //   4. a vehicle gate / control passable on foot (gate, bollard, cattle_grid, ...)
            //   5. a `service` (staff/delivery) entrance -- demoted below real gates
            //   6. routable (on a highway), then the most major road
            let entrance = node_tags.entrance.as_deref();
            let is_main = node_tags.routing_entrance.as_deref() == Some("main")
                || entrance == Some("main");
            let is_service_entrance = entrance == Some("service");
            let is_public_entrance = node_tags.routing_entrance.is_some()
                || (entrance.is_some() && !is_service_entrance);
            let is_pedestrian_barrier = node_tags
                .barrier
                .as_deref()
                .is_some_and(|b| PEDESTRIAN_BARRIERS.contains(&b));
            let is_vehicle_barrier = node_tags
                .barrier
                .as_deref()
                .is_some_and(|b| VEHICLE_BARRIERS.contains(&b));
            let routable = is_routable(nid, data);
            let road_pref = highway_majorness(nid, data)
                .map(|m| (HIGHWAY_TYPES.len() - m) as i64)
                .unwrap_or(0);
            let score = (
                u8::from(is_main),
                u8::from(is_public_entrance),
                u8::from(is_pedestrian_barrier),
                u8::from(is_vehicle_barrier),
                u8::from(is_service_entrance),
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
    fn pedestrian_barrier_beats_vehicle_barrier() {
        let mut data = EntranceData::default();
        let stile = 1_i64; // pedestrian crossing, not on a road
        let gate = 2_i64; // vehicle gate, on a road
        data.coords.insert(stile, coord(60.0, 11.0));
        data.coords.insert(gate, coord(60.1, 11.1));
        data.tags.insert(stile, EntranceNodeTags { barrier: Some("stile".into()), ..Default::default() });
        data.tags.insert(gate, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        // The vehicle gate is even routable, but the pedestrian crossing must still win.
        data.highway_ways.insert(600, "service".into());
        data.node_highways.insert(gate, vec![600]);

        let ep = select_entrance_for_feature(&[stile, gate], &data).unwrap();
        assert_eq!(ep.node_id, stile);
    }

    #[test]
    fn service_entrance_is_demoted_below_barrier_gate() {
        let mut data = EntranceData::default();
        let service = 1_i64; // staff/delivery back entrance
        let gate = 2_i64; // public vehicle gate
        data.coords.insert(service, coord(60.0, 11.0));
        data.coords.insert(gate, coord(60.1, 11.1));
        data.tags.insert(service, EntranceNodeTags { entrance: Some("service".into()), ..Default::default() });
        data.tags.insert(gate, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });

        // A staff entrance is a worse public arrival point than the real gate.
        let ep = select_entrance_for_feature(&[service, gate], &data).unwrap();
        assert_eq!(ep.node_id, gate);
        // ...but a non-service public entrance still beats the gate.
        data.tags.insert(service, EntranceNodeTags { entrance: Some("yes".into()), ..Default::default() });
        let ep = select_entrance_for_feature(&[service, gate], &data).unwrap();
        assert_eq!(ep.node_id, service);
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
        // Pedestrian crossings and gate variants are candidates.
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "stile")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "turnstile")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "cattle_grid")])).is_some());
        // barrier=wall is a solid enclosure, not a passable gate.
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "wall")])).is_none());
        // toll_booth / border_control are points on a through-road, not feature entrances.
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "toll_booth")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "border_control")])).is_none());
        // Entrances are an allowlist: no/garage/emergency/exit aren't public arrival points on
        // foot, and an unknown value defaults to rejected.
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "no")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "garage")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "emergency")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "exit")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "something_new")])).is_none());
        // ...unless an independent gate tag is also present.
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "no"), ("barrier", "gate")])).is_some());
        // Genuine pedestrian entrances are kept.
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "service")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "staircase")])).is_some());
        assert!(is_entrance_candidate(&HashMap::from([("amenity", "cafe")])).is_none());
    }

    #[test]
    fn highway_type_filters_to_tracked_types() {
        assert_eq!(highway_type(&HashMap::from([("highway", "unclassified")])), Some("unclassified".into()));
        assert_eq!(highway_type(&HashMap::from([("highway", "proposed")])), None);
        assert_eq!(highway_type(&HashMap::from([("landuse", "military")])), None);
    }
}
