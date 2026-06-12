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
//! 2. an unrestricted candidate over a restricted one (`foot=no`, or `access=private` without
//!    explicit foot access, is demoted; `access=no` / `locked=yes` are never candidates at all)
//! 3. a public pedestrian `entrance=*` (any allowed value except `service`) / `routing:entrance=*`
//! 4. a pedestrian barrier crossing (`barrier=stile`/`turnstile`/...)
//! 5. a vehicle gate / control passable on foot (`barrier=gate`/`bollard`/`cattle_grid`/...)
//! 6. a `service` (staff/delivery) `entrance=*`, demoted below real gates
//! 7. a named gate ("Hovedporten") over an unnamed one of the same kind -- a weak signal (names
//!    also mark numbered side gates like "Port 2"), so it never outranks entrance type
//! 8. reachable (member of a tracked street or path `highway=*` way), then on a way built for
//!    walking (footway/path/pedestrian/...) over one that is only on a road
//!
//! Ties are broken by the smaller node id, for determinism. When two or more candidates tie on the
//! full ranking key the pick among them is arbitrary; the selection reports the tied coordinates so
//! pass 4 can log how often that happens and how far apart the tied gates are.
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
    /// The node carries a `name` ("Hovedporten", ...). A weak signal -- names also mark numbered
    /// side gates ("Port 2") -- so it only outranks candidates of the same kind, never a better
    /// entrance type.
    pub(crate) named: bool,
    /// `foot=no`, or `access=private` without explicit foot access: passable, but probably not
    /// for an arriving member of the public on foot. Demoted below unrestricted candidates.
    pub(crate) restricted: bool,
}

/// All inputs the selector needs, collected during pass 4.
#[derive(Default)]
pub(crate) struct EntranceData {
    /// Candidate node coordinates, keyed by node id.
    pub(crate) coords: HashMap<i64, Coordinate>,
    /// Candidate node entrance tags, keyed by node id.
    pub(crate) tags: HashMap<i64, EntranceNodeTags>,
    /// Highway ways (those touching a candidate node), keyed by way id -> highway type. Used
    /// only to decide reachability and whether a gate sits on a foot-priority way.
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
    // A gate you cannot pass is not an arrival point, whatever else it is tagged. Common on
    // private forest-road gates (bomveg).
    if tags.get("access") == Some(&"no") || tags.get("locked") == Some(&"yes") {
        return None;
    }

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
        let named = tags.get("name").is_some_and(|n| !n.is_empty());
        // `foot=no` always restricts. `access=private` only restricts when pedestrians are not
        // explicitly welcomed: `access=private` + `foot=yes` is the classic "cars restricted,
        // people welcome" gate -- exactly what a foot arrival wants.
        let foot = tags.get("foot").copied();
        let foot_welcome = matches!(foot, Some("yes" | "permissive" | "designated"));
        let restricted = foot == Some("no")
            || (tags.get("access") == Some(&"private") && !foot_welcome);
        Some(EntranceNodeTags { entrance, barrier, routing_entrance, named, restricted })
    } else {
        None
    }
}

/// Pedestrian-network ways tracked for entrance ranking in addition to the street network in
/// [`HIGHWAY_TYPES`]. Most arrivals here are on foot, so a gate reached by a footpath is just as
/// reachable as one on a road. (This list is entrance-specific; `street.rs` keeps serving
/// addressing with the street network only.)
const PATH_HIGHWAYS: &[&str] = &["footway", "path", "cycleway", "steps", "track", "bridleway"];

/// Ways built for walking. A gate on one of these is where arriving pedestrians actually enter,
/// preferred over a gate that is only on a road. `track` is tracked for reachability above but is
/// primarily vehicular, and `steps` stay reachable but are not preferred -- a stairs-only point is
/// a poor canonical arrival for wheelchairs, prams and luggage.
const FOOT_PRIORITY_HIGHWAYS: &[&str] = &[
    "footway",
    "path",
    "pedestrian",
    "cycleway",
    "bridleway",
    "living_street",
];

/// Returns the highway type of a way if it is a street or path we track for entrance ranking,
/// else `None`.
pub(crate) fn highway_type(tags: &HashMap<&str, &str>) -> Option<String> {
    tags.get("highway")
        .filter(|h| HIGHWAY_TYPES.contains(h) || PATH_HIGHWAYS.contains(h))
        .map(|s| s.to_string())
}

/// True if the node sits on a tracked `highway=*` way (street or path), i.e. is reachable at all.
fn is_routable(node_id: i64, data: &EntranceData) -> bool {
    data.node_highways
        .get(&node_id)
        .is_some_and(|ways| ways.iter().any(|w| data.highway_ways.contains_key(w)))
}

/// True if the node sits on a way built for walking (see [`FOOT_PRIORITY_HIGHWAYS`]). Replaces the
/// old most-major-road preference, which was car logic: for a pedestrian, a park gate on a footpath
/// beats a vehicle gate on a trunk road.
fn on_foot_priority_way(node_id: i64, data: &EntranceData) -> bool {
    data.node_highways.get(&node_id).is_some_and(|ways| {
        ways.iter()
            .filter_map(|w| data.highway_ways.get(w))
            .any(|h| FOOT_PRIORITY_HIGHWAYS.contains(&h.as_str()))
    })
}

/// The chosen entrance for one feature, plus the coordinates of every candidate that tied with the
/// winner on the full ranking key (winner included, so `tied.len() >= 1`). `tied.len() > 1` means
/// the pick among them was arbitrary (smallest node id); pass 4 logs the tie rate and spread so we
/// can judge from real data whether ambiguous picks warrant a centroid fallback.
#[derive(Debug)]
pub(crate) struct SelectedEntrance {
    pub(crate) point: EntrancePoint,
    pub(crate) tied: Vec<Coordinate>,
}

/// The ranking key for one candidate (higher is better), tuned for an on-foot / transit arrival.
///
/// Field order equals the tier order documented in the module doc: the derived `Ord` compares
/// fields lexicographically in declaration order (with `false < true`), so the highest-priority
/// tier decides first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EntranceScore {
    /// An explicit `*=main` marker (`routing:entrance=main` or `entrance=main`).
    is_main: bool,
    /// Unrestricted beats restricted (`foot=no`, or `access=private` without explicit foot access).
    unrestricted: bool,
    /// A public pedestrian `entrance=*` (any allowed value except `service`) / `routing:entrance=*`.
    is_public_entrance: bool,
    /// A pedestrian barrier crossing (`barrier=stile`/`turnstile`/...).
    is_pedestrian_barrier: bool,
    /// A vehicle gate / control passable on foot (`barrier=gate`/`bollard`/`cattle_grid`/...).
    is_vehicle_barrier: bool,
    /// A `service` (staff/delivery) `entrance=*`, demoted below real gates.
    is_service_entrance: bool,
    /// Below the type tiers: a name only discriminates between same-kind candidates.
    named: bool,
    /// Reachable: member of a tracked street or path `highway=*` way.
    routable: bool,
    /// On a way built for walking (footway/path/pedestrian/...), preferred over road-only.
    on_foot_priority_way: bool,
}

/// Pick the best entrance among the candidate nodes that are members of this feature. Returns
/// `None` if the feature contains no candidate entrance node. Ties on score are broken by the
/// smaller node id for determinism; the tied coordinates are reported alongside the winner.
pub(crate) fn select_entrance_for_feature(
    member_node_ids: &[i64],
    data: &EntranceData,
) -> Option<SelectedEntrance> {
    // Perimeter rings repeat their first node and co-listed ways share endpoints; score each
    // candidate once.
    let mut seen = std::collections::HashSet::new();
    let scored: Vec<(EntranceScore, i64, EntrancePoint)> = member_node_ids
        .iter()
        .filter(|&&nid| seen.insert(nid))
        .filter_map(|&nid| {
            let node_tags = data.tags.get(&nid)?;
            let &coord = data.coords.get(&nid)?;
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
            let score = EntranceScore {
                is_main,
                unrestricted: !node_tags.restricted,
                is_public_entrance,
                is_pedestrian_barrier,
                is_vehicle_barrier,
                is_service_entrance,
                named: node_tags.named,
                routable: is_routable(nid, data),
                on_foot_priority_way: on_foot_priority_way(nid, data),
            };
            Some((score, nid, EntrancePoint { node_id: nid, coord }))
        })
        .collect();

    let best = scored.iter().map(|(score, _, _)| *score).max()?;
    let mut top: Vec<_> = scored.into_iter().filter(|(score, _, _)| *score == best).collect();
    // Smallest node id wins among equals, for determinism.
    top.sort_by_key(|&(_, nid, _)| nid);
    let point = top[0].2;
    let tied = top.iter().map(|&(_, _, p)| p.coord).collect();
    Some(SelectedEntrance { point, tied })
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
        let ep = select_entrance_for_feature(&fence_nodes, &data).expect("gate selected").point;
        assert_eq!(ep.node_id, 1240473681);
        assert!((ep.coord.lat - 60.8750).abs() < 1e-9);
        assert!((ep.coord.lon - 11.5600).abs() < 1e-9);
    }

    #[test]
    fn co_named_parcel_containing_gate_also_selects_it() {
        let data = terningmoen();
        // Landuse parcel 518428220 also contains the gate -> it must select it too.
        let landuse_nodes = vec![1002, 1240473681, 1005, 1006, 1003];
        let ep = select_entrance_for_feature(&landuse_nodes, &data).expect("gate selected").point;
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

        let ep = select_entrance_for_feature(&[main_gate, other_gate], &data).unwrap().point;
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

        let ep = select_entrance_for_feature(&[foot, gate], &data).unwrap().point;
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

        let ep = select_entrance_for_feature(&[stile, gate], &data).unwrap().point;
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
        let ep = select_entrance_for_feature(&[service, gate], &data).unwrap().point;
        assert_eq!(ep.node_id, gate);
        // ...but a non-service public entrance still beats the gate.
        data.tags.insert(service, EntranceNodeTags { entrance: Some("yes".into()), ..Default::default() });
        let ep = select_entrance_for_feature(&[service, gate], &data).unwrap().point;
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

        let ep = select_entrance_for_feature(&[routable, bare], &data).unwrap().point;
        assert_eq!(ep.node_id, routable);
    }

    #[test]
    fn named_gate_wins_within_its_tier() {
        let mut data = EntranceData::default();
        let named = 1_i64; // "Hovedporten", not on any way
        let plain = 2_i64; // unnamed gate, on a road
        data.coords.insert(named, coord(60.0, 11.0));
        data.coords.insert(plain, coord(60.1, 11.1));
        data.tags.insert(
            named,
            EntranceNodeTags { barrier: Some("gate".into()), named: true, ..Default::default() },
        );
        data.tags.insert(plain, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        // The unnamed gate is even routable; the named (principal) gate must still win.
        data.highway_ways.insert(600, "residential".into());
        data.node_highways.insert(plain, vec![600]);

        let ep = select_entrance_for_feature(&[named, plain], &data).unwrap().point;
        assert_eq!(ep.node_id, named);
    }

    #[test]
    fn entrance_type_beats_named_gate() {
        // Names also mark numbered side gates ("Port 2"), so a name must never outrank a better
        // entrance type.
        let mut data = EntranceData::default();
        let door = 1_i64; // unnamed entrance=yes
        let gate = 2_i64; // named AND routable vehicle gate
        data.coords.insert(door, coord(60.0, 11.0));
        data.coords.insert(gate, coord(60.1, 11.1));
        data.tags.insert(door, EntranceNodeTags { entrance: Some("yes".into()), ..Default::default() });
        data.tags.insert(
            gate,
            EntranceNodeTags { barrier: Some("gate".into()), named: true, ..Default::default() },
        );
        data.highway_ways.insert(600, "residential".into());
        data.node_highways.insert(gate, vec![600]);

        let ep = select_entrance_for_feature(&[door, gate], &data).unwrap().point;
        assert_eq!(ep.node_id, door);

        // An unnamed pedestrian crossing also beats the named vehicle gate.
        data.tags.insert(door, EntranceNodeTags { barrier: Some("turnstile".into()), ..Default::default() });
        let ep = select_entrance_for_feature(&[door, gate], &data).unwrap().point;
        assert_eq!(ep.node_id, door);
    }

    #[test]
    fn foot_priority_ways_are_always_tracked() {
        // Invariant: a way cannot be foot-priority without also being collected as reachable.
        for h in FOOT_PRIORITY_HIGHWAYS {
            assert!(
                HIGHWAY_TYPES.contains(h) || PATH_HIGHWAYS.contains(h),
                "{h} is foot-priority but would not be collected as a tracked way"
            );
        }
    }

    #[test]
    fn restricted_gate_is_demoted() {
        let mut data = EntranceData::default();
        let private = 1_i64; // access=private, named AND routable
        let public = 2_i64; // plain public gate
        data.coords.insert(private, coord(60.0, 11.0));
        data.coords.insert(public, coord(60.1, 11.1));
        data.tags.insert(
            private,
            EntranceNodeTags {
                barrier: Some("gate".into()),
                named: true,
                restricted: true,
                ..Default::default()
            },
        );
        data.tags.insert(public, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.highway_ways.insert(600, "residential".into());
        data.node_highways.insert(private, vec![600]);

        // The public gate wins despite the private one being named and routable...
        let ep = select_entrance_for_feature(&[private, public], &data).unwrap().point;
        assert_eq!(ep.node_id, public);
        // ...but a lone restricted gate is still better than nothing.
        let ep = select_entrance_for_feature(&[private], &data).unwrap().point;
        assert_eq!(ep.node_id, private);
    }

    #[test]
    fn gate_on_footpath_beats_gate_on_road() {
        let mut data = EntranceData::default();
        let on_path = 1_i64; // gate where a footway crosses the perimeter
        let on_road = 2_i64; // gate on a residential road
        data.coords.insert(on_path, coord(60.0, 11.0));
        data.coords.insert(on_road, coord(60.1, 11.1));
        data.tags.insert(on_path, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.tags.insert(on_road, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.highway_ways.insert(600, "footway".into());
        data.highway_ways.insert(601, "residential".into());
        data.node_highways.insert(on_path, vec![600]);
        data.node_highways.insert(on_road, vec![601]);

        // Both are reachable, but the arriving pedestrian enters via the footpath.
        let ep = select_entrance_for_feature(&[on_road, on_path], &data).unwrap().point;
        assert_eq!(ep.node_id, on_path);
    }

    #[test]
    fn full_score_tie_is_reported_with_both_coordinates() {
        let mut data = EntranceData::default();
        let a = 5_i64;
        let b = 9_i64;
        data.coords.insert(a, coord(60.0, 11.0));
        data.coords.insert(b, coord(60.2, 11.2));
        data.tags.insert(a, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });
        data.tags.insert(b, EntranceNodeTags { barrier: Some("gate".into()), ..Default::default() });

        // Repeated ids (ring closure) must not inflate the tie count.
        let sel = select_entrance_for_feature(&[b, a, b, a], &data).unwrap();
        assert_eq!(sel.point.node_id, a, "smaller node id wins the tie");
        assert_eq!(sel.tied.len(), 2);

        // A clear winner reports no tie.
        data.tags.insert(a, EntranceNodeTags { barrier: Some("gate".into()), named: true, ..Default::default() });
        let sel = select_entrance_for_feature(&[b, a], &data).unwrap();
        assert_eq!(sel.tied.len(), 1);
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
        // Impassable gates are never candidates, whatever else they carry.
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("access", "no")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("locked", "yes")])).is_none());
        assert!(is_entrance_candidate(&HashMap::from([("entrance", "main"), ("access", "no")])).is_none());
        // Private / foot=no gates stay candidates but are flagged restricted.
        let t = is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("access", "private")])).unwrap();
        assert!(t.restricted);
        let t = is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("foot", "no")])).unwrap();
        assert!(t.restricted);
        // ...but access=private + foot=yes is "cars restricted, people welcome": unrestricted.
        let t = is_entrance_candidate(&HashMap::from([
            ("barrier", "gate"), ("access", "private"), ("foot", "yes"),
        ])).unwrap();
        assert!(!t.restricted);
        // A name marks the principal gate.
        let t = is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("name", "Hovedporten")])).unwrap();
        assert!(t.named);
        let t = is_entrance_candidate(&HashMap::from([("barrier", "gate"), ("name", "")])).unwrap();
        assert!(!t.named);
    }

    #[test]
    fn highway_type_filters_to_tracked_types() {
        assert_eq!(highway_type(&HashMap::from([("highway", "unclassified")])), Some("unclassified".into()));
        // The pedestrian network counts: most arrivals are on foot.
        assert_eq!(highway_type(&HashMap::from([("highway", "footway")])), Some("footway".into()));
        assert_eq!(highway_type(&HashMap::from([("highway", "track")])), Some("track".into()));
        assert_eq!(highway_type(&HashMap::from([("highway", "proposed")])), None);
        assert_eq!(highway_type(&HashMap::from([("landuse", "military")])), None);
    }
}
