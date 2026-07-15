use std::collections::{BTreeMap, HashMap};

use crate::common::category::{
    as_category, county_ids_category, locality_ids_category, COUNTRY_PREFIX, LAYER_POI,
    LEGACY_CATEGORY_PREFIX, LEGACY_LAYER_ADDRESS, LEGACY_SOURCE_WHOSONFIRST, SOURCE_OSM,
};
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::join_osm_values;
use crate::common::text::titleize;
use crate::config::RankAddress;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;

use super::address_index::{AddressNodeIndex, AddressPolygonIndex};
use super::admin::AdministrativeBoundary;
use super::admin::AdministrativeBoundaryIndex;
use super::coordinate::{Coordinate, CoordinateStore};
use super::geometry::calculate_centroid;
use super::popularity::OsmPopularityCalculator;
use super::street::StreetIndex;

const ACCURACY_POINT: &str = "point";
const ACCURACY_POLYGON: &str = "polygon";

pub(crate) const OBJECT_TYPE_NODE: &str = "N";
pub(crate) const OBJECT_TYPE_WAY: &str = "W";
pub(crate) const OBJECT_TYPE_RELATION: &str = "R";

// ---------------------------------------------------------------------------
// OsmEntityConverter
// ---------------------------------------------------------------------------

/// Converts individual OSM elements (nodes, ways, relations) into Nominatim places.
///
/// The `'a` lifetime means this struct *borrows* (does not own) the indexes, coordinate
/// stores, and config that were built up during passes 1-3. All borrowed data must
/// outlive this converter. This avoids copying large data structures while the borrow
/// checker guarantees the references stay valid at compile time.
pub(crate) struct OsmEntityConverter<'a> {
    pub(crate) nodes_coords: &'a CoordinateStore,
    pub(crate) way_centroids: &'a CoordinateStore,
    pub(crate) admin_boundary_index: &'a mut AdministrativeBoundaryIndex,
    pub(crate) street_index: &'a StreetIndex,
    /// Addressed building/area polygons -- a contained POI inherits their address.
    pub(crate) address_polygon_index: &'a AddressPolygonIndex,
    /// Standalone address nodes -- a nearby POI inherits their address.
    pub(crate) address_node_index: &'a AddressNodeIndex,
    pub(crate) popularity_calculator: &'a OsmPopularityCalculator,
    pub(crate) importance_calc: ImportanceCalculator<'a>,
    /// The configured OSM rank_address values, resolved once at construction from `config.osm`.
    pub(crate) rank_address: &'a RankAddress,
    /// Way id -> chosen entrance/gate point. Pre-filtered to features worth enriching; empty when
    /// enrichment is disabled, so the override below is then a no-op for every feature.
    pub(crate) way_entrance_points: &'a HashMap<i64, super::entrance::EntrancePoint>,
    /// Relation id -> chosen entrance/gate point (multipolygon area features). Keyed separately
    /// from ways because a way id and a relation id can collide numerically.
    pub(crate) relation_entrance_points: &'a HashMap<i64, super::entrance::EntrancePoint>,
}

impl<'a> OsmEntityConverter<'a> {
    /// Filter tags to only those matching configured filters (sorted by key).
    ///
    /// Returns a `BTreeMap` deliberately: iterating it yields tags in alphabetical key order,
    /// matching the original converter's tag ordering. A `HashMap` here would make the category
    /// output nondeterministic.
    pub(crate) fn filter_tags<'t>(
        &self,
        tags: &HashMap<&'t str, &'t str>,
    ) -> BTreeMap<&'t str, &'t str> {
        tags.iter()
            .filter(|(k, v)| self.popularity_calculator.has_filter(k, v))
            .map(|(&k, &v)| (k, v))
            .collect()
    }

    pub(crate) fn convert_node(
        &mut self,
        id: i64,
        lat: f64,
        lon: f64,
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }
        let tags = self.filter_tags(all_tags);
        let coord = Coordinate { lat, lon };
        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_NODE,
            ACCURACY_POINT,
            coord,
            coord,
            None,
            all_tags,
        ))
    }

    pub(crate) fn convert_way(
        &mut self,
        id: i64,
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }
        let centroid = self.way_centroids.get(id)?;
        // The display pin moves to the entrance/gate for large area features worth enriching,
        // but the address is resolved at the interior centroid so a boundary gate can't inherit
        // a neighbour's address.
        let display = self.way_entrance_points.get(&id).map_or(centroid, |ep| ep.coord);
        let tags = self.filter_tags(all_tags);
        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_WAY,
            ACCURACY_POLYGON,
            display,
            centroid,
            None,
            all_tags,
        ))
    }

    pub(crate) fn convert_relation(
        &mut self,
        id: i64,
        member_node_ids: &[i64],
        member_way_ids: &[i64],
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }

        let member_coords = self.collect_member_coords(member_node_ids, member_way_ids);
        if member_coords.is_empty() {
            return None;
        }

        let centroid = calculate_centroid(&member_coords)?;
        // See convert_way: the display pin may move to the entrance, the address stays at the
        // interior centroid.
        let display = self.relation_entrance_points.get(&id).map_or(centroid, |ep| ep.coord);
        let tags = self.filter_tags(all_tags);

        let fallback_county = if tags.get("type") == Some(&"boundary")
            && tags.get("boundary") == Some(&"administrative")
        {
            Some(titleize(name))
        } else {
            None
        };

        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_RELATION,
            ACCURACY_POLYGON,
            display,
            centroid,
            fallback_county.as_deref(),
            all_tags,
        ))
    }

    fn collect_member_coords(
        &self,
        member_node_ids: &[i64],
        member_way_ids: &[i64],
    ) -> Vec<Coordinate> {
        let mut coords = Vec::new();
        for &nid in member_node_ids {
            if let Some(c) = self.nodes_coords.get(nid) {
                coords.push(c);
            }
        }
        for &wid in member_way_ids {
            if let Some(c) = self.way_centroids.get(wid) {
                coords.push(c);
            }
        }
        coords
    }

    /// `centroid` is the display pin (may be an entrance/gate for enriched features);
    /// `address_coord` is the interior point address inheritance is resolved at (equal to
    /// `centroid` unless a display pin was substituted).
    #[allow(clippy::too_many_arguments)]
    fn create_place_content(
        &mut self,
        entity_id: i64,
        tags: &BTreeMap<&str, &str>,
        name: &str,
        object_type: &str,
        accuracy: &str,
        centroid: Coordinate,
        address_coord: Coordinate,
        fallback_county: Option<&str>,
        all_tags: &HashMap<&str, &str>,
    ) -> NominatimPlace {
        let (county, municipality) =
            self.admin_boundary_index.find_county_and_municipality(&centroid);

        let country = determine_country(county, municipality, all_tags, &centroid);
        let osm_id = format!("OSM:TopographicPlace:{}", entity_id);

        let visible_categories = build_visible_categories(tags);
        let alt_names = build_alt_names(tags, name);
        let en_name = tags.get("en:name").copied().map(|s| s.to_string());

        let ResolvedAdmin { gid: county_gid, name: county_name } =
            resolve_county(county, fallback_county);
        let ResolvedAdmin { gid: locality_gid, name: locality } =
            resolve_municipality(municipality);

        let (street, housenumber) = self.resolve_address(all_tags, &address_coord);

        let address = Address {
            street,
            city: locality.clone(),
            county: county_name,
        };

        let extra = build_extra(
            &osm_id,
            accuracy,
            &country,
            &county_gid,
            &locality,
            &locality_gid,
            &visible_categories,
            &alt_names,
        );

        let indexed_categories = build_indexed_categories(
            &osm_id,
            &visible_categories,
            &country,
            &county_gid,
            &locality_gid,
        );

        let rank_address = self.determine_rank_address(tags);
        let importance = self.calculate_importance(tags, &osm_id);

        let content = PlaceContent {
            place_id: as_place_id(&osm_id),
            object_type: object_type.to_string(),
            object_id: 0,
            categories: indexed_categories,
            rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(name.to_string()),
                name_en: en_name,
                alt_name: join_osm_values(&alt_names),
            }),
            housenumber,
            address,
            postcode: None,
            country_code: country.map(|c| c.alpha2.clone()),
            centroid: centroid.centroid(),
            bbox: centroid.bbox(),
            extra,
        };

        NominatimPlace {
            type_: "Place".to_string(),
            content: vec![content],
        }
    }

    /// Street + housenumber for a feature, in priority order:
    /// 1. the feature's own `addr:street` (+ own `addr:housenumber`);
    /// 2. the addressed polygon that contains it -- containment lets us trust an inherited
    ///    housenumber the way a bare nearest-street match cannot;
    /// 3. the nearest standalone address node (within 20 m);
    /// 4. the nearest road segment name, never paired with a housenumber.
    ///
    /// For 2 and 3, the feature's own `addr:housenumber` (dropped by 1 when it has no street)
    /// is preferred over the inherited number, since it is more specific to the feature.
    fn resolve_address(
        &self,
        all_tags: &HashMap<&str, &str>,
        centroid: &Coordinate,
    ) -> (Option<String>, Option<String>) {
        if let Some(&street) = all_tags.get("addr:street") {
            let housenumber = all_tags.get("addr:housenumber").map(|s| s.to_string());
            return (Some(street.to_string()), housenumber);
        }

        if let Some(inherited) = self
            .address_polygon_index
            .find_containing(centroid)
            .or_else(|| self.address_node_index.find_nearest(centroid))
        {
            let own = all_tags.get("addr:housenumber").map(|s| s.to_string());
            return (Some(inherited.street), own.or(inherited.housenumber));
        }

        (self.street_index.find_nearest_street(centroid), None)
    }

    fn determine_rank_address(&self, tags: &BTreeMap<&str, &str>) -> i32 {
        let ra = self.rank_address;
        if tags.contains_key("boundary") {
            ra.boundary
        } else if tags.contains_key("place") {
            ra.place
        } else if tags.contains_key("road") {
            ra.road
        } else if tags.contains_key("building") {
            ra.building
        } else {
            ra.poi
        }
    }

    fn calculate_importance(&self, tags: &BTreeMap<&str, &str>, id: &str) -> RawNumber {
        let popularity = self.popularity_calculator.calculate_popularity(tags);
        RawNumber::from_f64_6dp(self.importance_calc.calculate_importance_for(id, popularity))
    }
}

fn build_visible_categories(tags: &BTreeMap<&str, &str>) -> Vec<String> {
    let mut cats = vec![
        LEGACY_SOURCE_WHOSONFIRST.to_string(),
        LEGACY_LAYER_ADDRESS.to_string(),
        format!("{}poi", LEGACY_CATEGORY_PREFIX),
    ];
    for &v in tags.values() {
        cats.push(format!("{}{}", LEGACY_CATEGORY_PREFIX, v));
    }
    cats
}

fn build_alt_names(tags: &BTreeMap<&str, &str>, name: &str) -> Vec<String> {
    let alt_name_keys = ["alt_name", "old_name", "no:name", "loc_name", "short_name"];
    alt_name_keys
        .iter()
        .filter_map(|&k| tags.get(k).copied())
        .filter(|&v| !v.is_empty() && v != name)
        .map(|s| s.to_string())
        .collect()
}

/// The GID and display name resolved from an administrative boundary lookup.
struct ResolvedAdmin {
    gid: Option<String>,
    name: Option<String>,
}

fn resolve_county(
    county: Option<&AdministrativeBoundary>,
    fallback_county: Option<&str>,
) -> ResolvedAdmin {
    let gid = county
        .and_then(|c| c.ref_code.as_ref())
        .map(|r| format!("KVE:TopographicPlace:{}", r));
    let name = county
        .map(|c| titleize(&c.name))
        .or_else(|| fallback_county.map(|s| s.to_string()));
    ResolvedAdmin { gid, name }
}

fn resolve_municipality(municipality: Option<&AdministrativeBoundary>) -> ResolvedAdmin {
    let gid = municipality
        .and_then(|m| m.ref_code.as_ref())
        .map(|r| format!("KVE:TopographicPlace:{}", r));
    let name = municipality.map(|m| titleize(&m.name));
    ResolvedAdmin { gid, name }
}

#[allow(clippy::too_many_arguments)]
fn build_extra(
    osm_id: &str,
    accuracy: &str,
    country: &Option<Country>,
    county_gid: &Option<String>,
    locality: &Option<String>,
    locality_gid: &Option<String>,
    visible_categories: &[String],
    alt_names: &[String],
) -> Extra {
    Extra {
        id: Some(osm_id.to_string()),
        source: Some("openstreetmap".to_string()),
        accuracy: Some(accuracy.to_string()),
        country_a: country.as_ref().map(|c| c.alpha3.clone()),
        county_gid: county_gid.clone(),
        locality: locality.clone(),
        locality_gid: locality_gid.clone(),
        tags: join_osm_values(visible_categories),
        alt_name: join_osm_values(alt_names),
        ..Extra::default()
    }
}

fn build_indexed_categories(
    osm_id: &str,
    visible_categories: &[String],
    country: &Option<Country>,
    county_gid: &Option<String>,
    locality_gid: &Option<String>,
) -> Vec<String> {
    let mut cats = visible_categories.to_vec();
    cats.push(SOURCE_OSM.to_string());
    cats.push(LAYER_POI.to_string());
    if let Some(c) = country {
        cats.push(format!("{}{}", COUNTRY_PREFIX, c.alpha2));
    }
    if let Some(gid) = county_gid {
        cats.push(county_ids_category(gid));
    }
    if let Some(gid) = locality_gid {
        cats.push(locality_ids_category(gid));
    }
    cats.push(as_category(osm_id));
    cats
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

pub(crate) fn determine_country(
    county: Option<&AdministrativeBoundary>,
    municipality: Option<&AdministrativeBoundary>,
    tags: &HashMap<&str, &str>,
    coord: &Coordinate,
) -> Option<Country> {
    county
        .map(|c| c.country.clone())
        .or_else(|| municipality.map(|m| m.country.clone()))
        .or_else(|| {
            tags.get("addr:country")
                .and_then(|id| Country::parse(id))
        })
        .or_else(|| geo::get_country(coord))
}

/// Extract a 2-letter country code from OSM admin relation tags.
pub(crate) fn extract_country_code(tags: &HashMap<&str, &str>) -> Option<Country> {
    let iso = tags
        .get("ISO3166-2")
        .or_else(|| tags.get("ISO3166-2-lvl4"))
        .or_else(|| tags.get("ISO3166-2:lvl4"))
        .or_else(|| tags.get("is_in:country_code"))
        .or_else(|| tags.get("country_code"));

    if let Some(code) = iso {
        let two_letter = &code[..code.len().min(2)];
        if let Some(c) = Country::parse(two_letter) {
            return Some(c);
        }
    }

    // If ref is all digits, assume Norway
    if let Some(ref_val) = tags.get("ref")
        && ref_val.chars().all(|c| c.is_ascii_digit()) {
            return Some(Country::no());
        }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::usage::UsageBoost;
    use crate::config::Config;
    use super::super::admin::ADMIN_LEVEL_COUNTY;
    use super::super::admin::ADMIN_LEVEL_MUNICIPALITY;
    use super::super::geometry::BoundingBox;
    use crate::source::test_helpers::test_config_with_osm_filters;
    use super::super::entrance::EntrancePoint;

    static EMPTY_USAGE: std::sync::LazyLock<UsageBoost> =
        std::sync::LazyLock::new(UsageBoost::empty);
    static EMPTY_ENTRANCE_POINTS: std::sync::LazyLock<HashMap<i64, EntrancePoint>> =
        std::sync::LazyLock::new(HashMap::new);
    static EMPTY_ADDR_POLYGONS: std::sync::LazyLock<AddressPolygonIndex> =
        std::sync::LazyLock::new(AddressPolygonIndex::new);
    static EMPTY_ADDR_NODES: std::sync::LazyLock<AddressNodeIndex> =
        std::sync::LazyLock::new(AddressNodeIndex::new);

    fn make_converter<'a>(
        config: &'a Config,
        nodes: &'a CoordinateStore,
        ways: &'a CoordinateStore,
        admin_index: &'a mut AdministrativeBoundaryIndex,
        street_index: &'a StreetIndex,
        pop_calc: &'a OsmPopularityCalculator,
    ) -> OsmEntityConverter<'a> {
        make_converter_with_entrances(
            config, nodes, ways, admin_index, street_index, pop_calc, &EMPTY_ENTRANCE_POINTS,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn make_converter_with_entrances<'a>(
        config: &'a Config,
        nodes: &'a CoordinateStore,
        ways: &'a CoordinateStore,
        admin_index: &'a mut AdministrativeBoundaryIndex,
        street_index: &'a StreetIndex,
        pop_calc: &'a OsmPopularityCalculator,
        entrance_points: &'a HashMap<i64, EntrancePoint>,
    ) -> OsmEntityConverter<'a> {
        OsmEntityConverter {
            nodes_coords: nodes,
            way_centroids: ways,
            admin_boundary_index: admin_index,
            street_index,
            address_polygon_index: &EMPTY_ADDR_POLYGONS,
            address_node_index: &EMPTY_ADDR_NODES,
            popularity_calculator: pop_calc,
            importance_calc: ImportanceCalculator::new(&EMPTY_USAGE),
            rank_address: &config.osm.as_ref().expect("osm config present in tests").rank_address,
            way_entrance_points: entrance_points,
            relation_entrance_points: &EMPTY_ENTRANCE_POINTS,
        }
    }

    fn empty_converter_parts(config: &Config) -> (CoordinateStore, CoordinateStore, AdministrativeBoundaryIndex, StreetIndex, OsmPopularityCalculator) {
        (
            CoordinateStore::new(16),
            CoordinateStore::new(16),
            AdministrativeBoundaryIndex::new(),
            StreetIndex::new(),
            OsmPopularityCalculator::new(config),
        )
    }

    // -- filter_tags --

    #[test]
    fn filter_tags_keeps_only_configured_filters() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("amenity", "hospital");
        tags.insert("name", "Oslo Hospital");
        tags.insert("building", "yes");

        let filtered = conv.filter_tags(&tags);
        assert!(filtered.contains_key("amenity"));
        assert!(!filtered.contains_key("name"));
        assert!(!filtered.contains_key("building"));
    }

    #[test]
    fn filter_tags_returns_empty_for_no_matches() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("name", "Something");
        tags.insert("building", "yes");

        let filtered = conv.filter_tags(&tags);
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_tags_returns_sorted_keys() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("tourism", "museum");
        tags.insert("amenity", "hospital");

        let filtered = conv.filter_tags(&tags);
        let keys: Vec<&str> = filtered.keys().copied().collect();
        assert_eq!(keys, vec!["amenity", "tourism"]); // alphabetical
    }

    // -- determine_rank_address --

    #[test]
    fn rank_address_boundary_takes_priority() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("boundary", "administrative"), ("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 10);
    }

    #[test]
    fn rank_address_place_when_no_boundary() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 20);
    }

    #[test]
    fn rank_address_road() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("road", "residential")]);
        assert_eq!(conv.determine_rank_address(&tags), 26);
    }

    #[test]
    fn rank_address_building() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("building", "yes")]);
        assert_eq!(conv.determine_rank_address(&tags), 28);
    }

    #[test]
    fn rank_address_defaults_to_poi() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("amenity", "hospital")]);
        assert_eq!(conv.determine_rank_address(&tags), 30);
    }

    // -- convert_node integration --

    #[test]
    fn convert_node_returns_none_without_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_returns_none_with_empty_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", ""), ("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_has_correct_object_type() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].object_type, "N");
    }

    #[test]
    fn convert_node_has_point_accuracy() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.accuracy.as_deref(), Some("point"));
    }

    #[test]
    fn convert_node_has_osm_source() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.source.as_deref(), Some("openstreetmap"));
    }

    #[test]
    fn convert_node_categories_include_tag_values() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.contains(&"legacy.category.hospital".to_string()));
        assert!(cats.contains(&LEGACY_SOURCE_WHOSONFIRST.to_string()));
        assert!(cats.contains(&LEGACY_LAYER_ADDRESS.to_string()));
        assert!(cats.contains(&LAYER_POI.to_string()));
        assert!(cats.contains(&"legacy.category.poi".to_string()));
    }

    #[test]
    fn convert_node_categories_include_multiple_tag_values() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Museum Hotel"),
            ("amenity", "hospital"),
            ("tourism", "museum"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.contains(&"legacy.category.hospital".to_string()));
        assert!(cats.contains(&"legacy.category.museum".to_string()));
    }

    #[test]
    fn convert_node_categories_exclude_non_filtered_tags() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Something"),
            ("amenity", "hospital"),
            ("building", "yes"),  // not in filters
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(!cats.contains(&"legacy.category.yes".to_string()));
    }

    #[test]
    fn convert_node_has_correct_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Oslo Sykehus"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].name.as_ref().unwrap().name.as_deref(), Some("Oslo Sykehus"));
    }

    #[test]
    fn convert_node_alt_names_from_filtered_tags() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("alt_name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let extra_alt = &place.content[0].extra.alt_name;
        assert!(extra_alt.is_none()); // no visible alt names
    }

    #[test]
    fn convert_node_en_name_from_filtered_tags() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("en:name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert!(place.content[0].name.as_ref().unwrap().name_en.is_none());
    }

    #[test]
    fn convert_node_osm_id_in_extra() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.id.as_deref(), Some("OSM:TopographicPlace:42"));
    }

    #[test]
    fn convert_node_osm_id_in_categories() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.contains(&"OSM.TopographicPlace.42".to_string()));
    }

    #[test]
    fn convert_node_has_correct_coordinates() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.91, 10.75, &tags).unwrap();
        let centroid = &place.content[0].centroid;
        assert_eq!(centroid.len(), 2);
        assert!((centroid[0] - 10.75).abs() < 1e-6); // lon first
        assert!((centroid[1] - 59.91).abs() < 1e-6); // lat second
    }

    #[test]
    fn convert_node_importance_reflects_priority() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let hospital_tags = HashMap::from([("name", "Hospital"), ("amenity", "hospital")]); // priority 9
        let cinema_tags = HashMap::from([("name", "Cinema"), ("amenity", "cinema")]); // priority 1
        let h = conv.convert_node(1, 59.9, 10.7, &hospital_tags).unwrap();
        let c = conv.convert_node(2, 59.9, 10.7, &cinema_tags).unwrap();

        let h_imp: f64 = h.content[0].importance.0.parse().unwrap();
        let c_imp: f64 = c.content[0].importance.0.parse().unwrap();
        assert!(h_imp > c_imp, "hospital importance ({h_imp}) should be higher than cinema ({c_imp})");
    }

    #[test]
    fn convert_node_with_admin_boundary_has_county_gid() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_COUNTY,
            ref_code: Some("03".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.county_gid.as_deref(), Some("KVE:TopographicPlace:03"));
    }

    #[test]
    fn convert_node_with_municipality_has_locality_gid_and_titleized_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_MUNICIPALITY,
            ref_code: Some("0301".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.locality_gid.as_deref(), Some("KVE:TopographicPlace:0301"));
        assert_eq!(place.content[0].extra.locality.as_deref(), Some("Oslo")); // titleized
        assert_eq!(place.content[0].address.city.as_deref(), Some("Oslo"));
    }

    #[test]
    fn convert_node_county_gid_in_categories() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_COUNTY,
            ref_code: Some("03".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.iter().any(|c| c.starts_with("county_gid.") && c.contains("03")));
    }

    // -- addr tags --

    #[test]
    fn convert_node_uses_addr_street_and_housenumber() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Kaffe Gram"),
            ("amenity", "hospital"),
            ("addr:street", "Kristiansands gate"),
            ("addr:housenumber", "2B"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let content = &place.content[0];
        assert_eq!(content.address.street.as_deref(), Some("Kristiansands gate"));
        assert_eq!(content.housenumber.as_deref(), Some("2B"));
    }

    #[test]
    fn convert_node_drops_housenumber_without_addr_street() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Test"),
            ("amenity", "hospital"),
            ("addr:housenumber", "30"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert!(place.content[0].housenumber.is_none());
    }

    #[test]
    fn convert_node_addr_street_without_housenumber() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Test"),
            ("amenity", "hospital"),
            ("addr:street", "Storgata"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].address.street.as_deref(), Some("Storgata"));
        assert!(place.content[0].housenumber.is_none());
    }

    // -- address inheritance --

    /// An addressed unit square [59.0,59.001] x [10.0,10.001]; query (59.0005, 10.0005) is inside.
    fn polygon_index_with_square(street: &str, housenumber: Option<&str>) -> AddressPolygonIndex {
        let mut index = AddressPolygonIndex::new();
        index.add_polygon(
            street,
            housenumber,
            &[
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 10.001 },
                Coordinate { lat: 59.001, lon: 10.001 },
                Coordinate { lat: 59.001, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 10.0 },
            ],
            1,
        );
        index
    }

    #[test]
    fn convert_node_inherits_from_containing_polygon() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let polygons = polygon_index_with_square("Storgata", Some("5"));
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);
        conv.address_polygon_index = &polygons;

        let tags = HashMap::from([("name", "Kaffebar"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.0005, 10.0005, &tags).unwrap();
        let content = &place.content[0];
        assert_eq!(content.address.street.as_deref(), Some("Storgata"));
        assert_eq!(content.housenumber.as_deref(), Some("5"));
    }

    #[test]
    fn convert_node_inherits_from_nearby_address_node() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut addr_nodes = AddressNodeIndex::new();
        addr_nodes.add_node(Coordinate { lat: 59.9, lon: 10.7 }, "Storgata", Some("12"), 1);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);
        conv.address_node_index = &addr_nodes;

        // ~11 m from the address node -- within the 20 m radius.
        let tags = HashMap::from([("name", "Kiosk"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9001, 10.7, &tags).unwrap();
        let content = &place.content[0];
        assert_eq!(content.address.street.as_deref(), Some("Storgata"));
        assert_eq!(content.housenumber.as_deref(), Some("12"));
    }

    #[test]
    fn convert_node_containing_polygon_beats_nearby_node() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let polygons = polygon_index_with_square("Bygningsgata", Some("7"));
        let mut addr_nodes = AddressNodeIndex::new();
        // Address node ~11 m from the query, also inside the same building.
        addr_nodes.add_node(Coordinate { lat: 59.0006, lon: 10.0005 }, "Nodeveien", Some("99"), 1);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);
        conv.address_polygon_index = &polygons;
        conv.address_node_index = &addr_nodes;

        let tags = HashMap::from([("name", "Butikk"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.0005, 10.0005, &tags).unwrap();
        let content = &place.content[0];
        // Containment wins over proximity.
        assert_eq!(content.address.street.as_deref(), Some("Bygningsgata"));
        assert_eq!(content.housenumber.as_deref(), Some("7"));
    }

    #[test]
    fn convert_node_own_housenumber_pairs_with_inherited_street() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let polygons = polygon_index_with_square("Storgata", Some("5"));
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);
        conv.address_polygon_index = &polygons;

        // POI inside the building has its own housenumber but no addr:street.
        let tags = HashMap::from([
            ("name", "Butikk"),
            ("amenity", "hospital"),
            ("addr:housenumber", "5C"),
        ]);
        let place = conv.convert_node(42, 59.0005, 10.0005, &tags).unwrap();
        let content = &place.content[0];
        assert_eq!(content.address.street.as_deref(), Some("Storgata"));
        // Own "5C" wins over the building's "5".
        assert_eq!(content.housenumber.as_deref(), Some("5C"));
    }

    #[test]
    fn convert_node_own_housenumber_pairs_with_address_node_street() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut addr_nodes = AddressNodeIndex::new();
        addr_nodes.add_node(Coordinate { lat: 59.9, lon: 10.7 }, "Storgata", Some("12"), 1);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);
        conv.address_node_index = &addr_nodes;

        // POI near the address node carries its own housenumber but no addr:street.
        let tags = HashMap::from([
            ("name", "Kiosk"),
            ("amenity", "hospital"),
            ("addr:housenumber", "12B"),
        ]);
        let place = conv.convert_node(42, 59.9001, 10.7, &tags).unwrap();
        let content = &place.content[0];
        assert_eq!(content.address.street.as_deref(), Some("Storgata"));
        // Own "12B" wins over the node's "12".
        assert_eq!(content.housenumber.as_deref(), Some("12B"));
    }

    #[test]
    fn convert_node_no_source_yields_no_housenumber() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        // Empty polygon/node/street indexes: nothing to inherit from.
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.0005, 10.0005, &tags).unwrap();
        let content = &place.content[0];
        assert!(content.address.street.is_none());
        assert!(content.housenumber.is_none());
    }

    // -- extract_country_code --

    #[test]
    fn extract_country_code_from_iso3166_2() {
        let tags = HashMap::from([("ISO3166-2", "NO-03")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.alpha2, "no");
    }

    #[test]
    fn extract_country_code_from_country_code_tag() {
        let tags = HashMap::from([("country_code", "NO")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.alpha2, "no");
    }

    #[test]
    fn extract_country_code_from_numeric_ref_assumes_norway() {
        let tags = HashMap::from([("ref", "0301")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.alpha2, "no");
    }

    #[test]
    fn extract_country_code_returns_none_for_no_tags() {
        let tags: HashMap<&str, &str> = HashMap::new();
        assert!(extract_country_code(&tags).is_none());
    }

    #[test]
    fn extract_country_code_returns_none_for_non_numeric_ref() {
        let tags = HashMap::from([("ref", "abc")]);
        assert!(extract_country_code(&tags).is_none());
    }

    // -- entrance enrichment override --

    #[test]
    fn convert_way_uses_entrance_coord_when_present() {
        let config = test_config_with_osm_filters();
        let (nodes, _ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut ways = CoordinateStore::new(16);
        // Way centroid sits inside the camp; the gate is on the perimeter.
        ways.put(518127311, Coordinate { lat: 60.90, lon: 11.60 });
        let gate = Coordinate { lat: 60.8771, lon: 11.5503 };
        let entrance_points = HashMap::from([(
            518127311_i64,
            EntrancePoint { node_id: 1240473681, coord: gate },
        )]);
        let mut conv = make_converter_with_entrances(
            &config, &nodes, &ways, &mut admin, &streets, &pop, &entrance_points,
        );

        let tags = HashMap::from([("name", "Terningmoen Leir"), ("tourism", "attraction")]);
        let place = conv.convert_way(518127311, &tags).unwrap();
        let c = &place.content[0].centroid; // [lon, lat]
        assert!((c[0] - 11.5503).abs() < 1e-6, "lon should be the gate's");
        assert!((c[1] - 60.8771).abs() < 1e-6, "lat should be the gate's");
        // bbox tracks the substituted point, not the centroid.
        let b = &place.content[0].bbox;
        assert!((b[0] - 11.5503).abs() < 1e-6);
        assert!((b[1] - 60.8771).abs() < 1e-6);
    }

    #[test]
    fn convert_way_resolves_address_at_centroid_not_entrance() {
        let config = test_config_with_osm_filters();
        let (nodes, _ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut ways = CoordinateStore::new(16);
        // True centroid inside "our" building; the gate sits over on the neighbour.
        ways.put(700, Coordinate { lat: 59.0005, lon: 10.0005 });
        let gate = Coordinate { lat: 59.2005, lon: 10.2005 };
        let entrance_points = HashMap::from([(700_i64, EntrancePoint { node_id: 1, coord: gate })]);

        let mut polygons = polygon_index_with_square("Riktig gate", Some("1"));
        // Neighbour building around the gate.
        polygons.add_polygon(
            "Nabogate",
            Some("99"),
            &[
                Coordinate { lat: 59.2, lon: 10.2 },
                Coordinate { lat: 59.2, lon: 10.201 },
                Coordinate { lat: 59.201, lon: 10.201 },
                Coordinate { lat: 59.201, lon: 10.2 },
                Coordinate { lat: 59.2, lon: 10.2 },
            ],
            2,
        );

        let mut conv = make_converter_with_entrances(
            &config, &nodes, &ways, &mut admin, &streets, &pop, &entrance_points,
        );
        conv.address_polygon_index = &polygons;

        let tags = HashMap::from([("name", "Stor Klinikk"), ("amenity", "hospital")]);
        let place = conv.convert_way(700, &tags).unwrap();
        let content = &place.content[0];
        // Display pin is the gate...
        assert!((content.centroid[0] - 10.2005).abs() < 1e-6);
        // ...but the address is the building the centroid sits in, not the neighbour at the gate.
        assert_eq!(content.address.street.as_deref(), Some("Riktig gate"));
        assert_eq!(content.housenumber.as_deref(), Some("1"));
    }

    #[test]
    fn convert_way_keeps_centroid_when_no_entrance() {
        let config = test_config_with_osm_filters();
        let (nodes, _ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut ways = CoordinateStore::new(16);
        ways.put(999, Coordinate { lat: 60.90, lon: 11.60 });
        // empty entrance_points (the default) -> no override
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Somewhere"), ("tourism", "attraction")]);
        let place = conv.convert_way(999, &tags).unwrap();
        let c = &place.content[0].centroid;
        assert!((c[0] - 11.60).abs() < 1e-6);
        assert!((c[1] - 60.90).abs() < 1e-6);
    }

}
