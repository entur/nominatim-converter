use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::{ImportanceCalculator, IMPORTANCE_FLOOR};
use crate::common::text::{join_osm_values, OSM_TAG_SEPARATOR};
use crate::common::translator;
use crate::common::usage::UsageBoost;
use crate::config::{Config, StopPlaceConfig};
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Importance floor for GoSPs listed in `secondaryGosps`. Must be strictly positive: Photon
/// adds importance to the document score after wrapping the query in a `function_score` with
/// `boostMode=Sum`. Empirically, a negative importance can drive the combined score below zero,
/// at which point Lucene clamps it and the document disappears from results. The actual
/// assigned value is `max(SECONDARY_GOSP_IMPORTANCE, IMPORTANCE_FLOOR)` so the
/// `[floor, 1.0]` invariant always holds even if floor is raised above 0.001.
const SECONDARY_GOSP_IMPORTANCE: f64 = 0.001;

/// Cap on non-secondary GoSP importance. Member popularities are multiplied
/// (`calculate_gosp_popularity`), so any busy hub saturates to 1.0 and the group outranks an exact
/// match on its own member stop ("Oslo" over "Oslo bussterminal"). Capping just below that keeps
/// the member match winning while the group still tops bare-name/typo queries. Empirically tuned;
/// the viable band is tight (~0.88-0.96), so re-check the `oslo`/`olso` acceptance tests if changed.
const GOSP_IMPORTANCE_CAP: f64 = 0.92;

use super::farezone::{FareZone, FareZones};
use super::popularity::calculate_stop_popularity;
use super::xml::*;

/// Apply the configured non-Norwegian importance penalty. A stop/GoSP resolved to any country
/// other than Norway has its importance multiplied by `factor` (1.0 = no penalty), then clamped
/// to `[IMPORTANCE_FLOOR, 1.0]` so the result stays a valid Photon importance even if `factor`
/// is misconfigured above 1.0.
fn apply_foreign_penalty(importance: f64, country: &Country, factor: f64) -> f64 {
    if country.alpha2 == "no" {
        importance
    } else {
        (importance * factor).clamp(IMPORTANCE_FLOOR, 1.0)
    }
}

pub fn convert_all(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
    fare_zone_input: Option<&Path>,
    usage: &UsageBoost,
) -> Result<usize, Box<dyn std::error::Error>> {
    let stop_place = config.stop_place.as_ref().ok_or("config is missing the required `stopPlace` section")?;
    let xml = std::fs::read_to_string(input)?;
    let result = parse_netex(&xml)?;
    let fare_zones = load_fare_zones(fare_zone_input)?;
    let importance_calc = ImportanceCalculator::new(usage);

    // Build child stop types map (parentRef -> list of child stopPlaceTypes)
    let mut stop_place_types: HashMap<String, Vec<String>> = HashMap::new();
    for sp in &result.stop_places {
        if let (Some(parent_ref), Some(sp_type)) = (&sp.parent_site_ref, &sp.stop_place_type) {
            stop_place_types
                .entry(parent_ref.ref_.clone())
                .or_default()
                .push(sp_type.clone());
        }
    }

    // Calculate popularities. The optional usage boost nudges popular stops upward;
    // member boosts reach GoSPs through the member-product propagation, and
    // `convert_gosp` additionally applies the group's own usage entry, so GoSP
    // ranking survives single-member churn in the stop place register.
    let stop_popularities: HashMap<String, i64> = result.stop_places.iter().map(|sp| {
        let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
        let pop = calculate_stop_popularity(stop_place, sp, &child_types, usage.factor(&sp.id));
        (sp.id.clone(), pop)
    }).collect();

    // Build child stops map (parentRef -> child stop places, in document order).
    // Child names are derived from this map where needed.
    let mut child_stops: HashMap<String, Vec<&StopPlaceXml>> = HashMap::new();
    for sp in &result.stop_places {
        if let Some(parent_ref) = &sp.parent_site_ref {
            child_stops.entry(parent_ref.ref_.clone()).or_default().push(sp);
        }
    }

    let mut entries = Vec::new();

    // Convert stop places
    for sp in &result.stop_places {
        let pop = stop_popularities.get(&sp.id).copied().unwrap_or(0);
        let my_child_stops = child_stops.get(&sp.id).cloned().unwrap_or_default();

        if let Some(entry) = convert_stop_place(
            stop_place, &importance_calc, sp, &result.topo_places,
            &stop_place_types, &fare_zones, pop, &my_child_stops,
        ) {
            entries.push(entry);
        }
    }

    let stop_by_id: HashMap<&str, &StopPlaceXml> =
        result.stop_places.iter().map(|sp| (sp.id.as_str(), sp)).collect();

    let secondary_gosps: HashSet<&str> = stop_place.group_of_stop_places.secondary_gosps
        .iter().map(String::as_str).collect();
    log_secondary_gosps(&secondary_gosps, &result.groups);

    // Convert groups of stop places
    for gosp in &result.groups {
        let is_secondary = secondary_gosps.contains(gosp.id.as_str());
        if let Some(entry) = convert_gosp(
            stop_place, &importance_calc, gosp, &result.topo_places,
            &stop_popularities, &stop_by_id, is_secondary,
        ) {
            entries.push(entry);
        }
    }

    let count = JsonWriter::export(&entries, output, is_appending)?;
    Ok(count)
}

fn load_fare_zones(input: Option<&Path>) -> Result<FareZones, Box<dyn std::error::Error>> {
    let Some(path) = input else {
        eprintln!("warning: no fare zone input; stop places get no fare zone data");
        return Ok(FareZones::empty());
    };
    let zones = FareZones::load(path)?;
    if zones.is_empty() {
        // A zone-less index looks healthy from every downstream check, so stop here instead.
        return Err(format!("fare zone input {} holds no zones", path.display()).into());
    }
    Ok(zones)
}

/// Log the configured secondary-GoSP demotions (and warn about configured IDs that
/// don't exist in the input) to stderr. No effect on the NDJSON output.
fn log_secondary_gosps(secondary_gosps: &HashSet<&str>, groups: &[GroupOfStopPlacesXml]) {
    if secondary_gosps.is_empty() {
        return;
    }
    let gosp_ids: HashSet<&str> = groups.iter().map(|g| g.id.as_str()).collect();
    let n = secondary_gosps.len();
    eprintln!("Demoting {n} configured secondary GoSP{}:", if n == 1 { "" } else { "s" });
    for gosp in groups {
        if secondary_gosps.contains(gosp.id.as_str()) {
            eprintln!("  {} \"{}\"", gosp.id, gosp.name.as_deref().unwrap_or(""));
        }
    }
    for id in secondary_gosps {
        if !gosp_ids.contains(id) {
            eprintln!("  warning: configured secondary GoSP {id} not found in input - typo?");
        }
    }
}

/// A stop place's role in the parent-child hierarchy. Drives the source category, the
/// `multimodal.*`/`source.nsr.*` categories, and the `extra.stop_place_role` response field.
#[derive(Debug, PartialEq)]
enum StopPlaceRole {
    Child,
    Parent,
    Standalone,
}

impl StopPlaceRole {
    /// Canonical role name: the wire value of `extra.stop_place_role` and the `source.nsr.*`
    /// suffix. Must match the proxy's `V3Result.StopPlaceRole` enum names.
    fn as_str(&self) -> &'static str {
        match self {
            StopPlaceRole::Parent => "parent",
            StopPlaceRole::Child => "child",
            StopPlaceRole::Standalone => "standalone",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_stop_place(
    stop_place: &StopPlaceConfig,
    importance_calc: &ImportanceCalculator,
    sp: &StopPlaceXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_place_types: &HashMap<String, Vec<String>>,
    fare_zones: &FareZones,
    popularity: i64,
    child_stops: &[&StopPlaceXml],
) -> Option<NominatimPlace> {
    let centroid_xml = sp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let sp_name = sp.name.as_deref()?;

    let geography = resolve_stop_geography(sp, topo_places);
    let country = determine_country(topo_places, sp, &coord);
    let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
    let importance = RawNumber::from_f64_6dp(apply_foreign_penalty(
        importance_calc.calculate_importance(popularity as f64),
        &country,
        stop_place.foreign_importance_factor,
    ));
    let role = classify_role(&child_types, sp.parent_site_ref.is_some());

    let inferred_types: Vec<String> = child_types
        .iter()
        .cloned()
        .chain(sp.stop_place_type.iter().cloned())
        .collect();

    let zones = fare_zones.zones_for(&sp.id, &coord);

    let StopCategories { visible: visible_cats, indexed: indexed_cats } = build_stop_categories(
        sp, &role, &inferred_types, &country, &geography, &zones,
    );

    let alt_names = build_stop_alt_names(sp, sp_name, child_stops);
    let visible_alt: Vec<String> = alt_stop_names(sp, sp_name, Some("label"));

    let entry = NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&sp.id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address: stop_place.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(sp_name.to_string()),
                name_en: None,
                alt_name: join_osm_values(&alt_names),
            }),
            address: Address {
                city: geography.locality.clone(),
                county: geography.county.clone(),
                ..Default::default()
            },
            housenumber: None,
            postcode: None,
            country_code: Some(country.alpha2.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: build_stop_extra(
                sp, &role, &country, &geography, &visible_alt, &visible_cats, &inferred_types,
                child_stops, &zones,
            ),
        }],
    };
    Some(entry)
}

/// Determine role: has children → Parent, else references a parent → Child, else Standalone.
/// Children win over a parent ref; NSR's hierarchy is single-level, so a node shouldn't be both.
fn classify_role(child_types: &[String], has_parent: bool) -> StopPlaceRole {
    if !child_types.is_empty() {
        StopPlaceRole::Parent
    } else if has_parent {
        StopPlaceRole::Child
    } else {
        StopPlaceRole::Standalone
    }
}

/// Locality/county names and GIDs resolved from a stop place's TopographicPlaceRef,
/// mirroring `GospGeography` for groups of stop places.
struct StopGeography {
    locality: Option<String>,
    locality_gid: Option<String>,
    county: Option<String>,
    county_gid: Option<String>,
}

fn resolve_stop_geography(
    sp: &StopPlaceXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
) -> StopGeography {
    let locality_gid = sp.topographic_place_ref.as_ref().map(|r| r.ref_.clone());
    let locality = locality_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.descriptor.as_ref()?.name.clone())
    });
    let county_gid = locality_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.parent_ref.as_ref().map(|r| r.ref_.clone()))
    });
    let county = county_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.descriptor.as_ref()?.name.clone())
    });
    StopGeography { locality, locality_gid, county, county_gid }
}

/// Categories for a stop place: `visible` feeds the human-facing `tags` extra field;
/// `indexed` (a superset of `visible`) feeds Photon's category filters.
struct StopCategories {
    visible: Vec<String>,
    indexed: Vec<String>,
}

fn build_stop_categories(
    sp: &StopPlaceXml,
    role: &StopPlaceRole,
    inferred_types: &[String],
    country: &Country,
    geography: &StopGeography,
    fare_zones: &[&FareZone],
) -> StopCategories {
    let source_cat = match role {
        StopPlaceRole::Parent => LEGACY_SOURCE_OPENSTREETMAP,
        StopPlaceRole::Child => LEGACY_SOURCE_GEONAMES,
        StopPlaceRole::Standalone => LEGACY_SOURCE_WHOSONFIRST,
    };
    let multimodal_cat = match role {
        StopPlaceRole::Parent => Some("multimodal.parent".to_string()),
        StopPlaceRole::Child => Some("multimodal.child".to_string()),
        StopPlaceRole::Standalone => None,
    };

    let mut visible_cats = vec![
        LEGACY_LAYER_VENUE.to_string(),
    ];
    if sp.transport_mode.as_deref() == Some("funicular") {
        visible_cats.push(format!("{LEGACY_CATEGORY_PREFIX}funicular"));
    }
    visible_cats.push(source_cat.to_string());

    let mut indexed_cats = visible_cats.clone();
    for t in inferred_types {
        indexed_cats.push(format!("{LEGACY_CATEGORY_PREFIX}{t}"));
        // First-class facet for v3's stopPlaceTypes filter; legacy.category.* stays for v2.
        indexed_cats.push(format!("{STOP_PLACE_TYPE_PREFIX}{t}"));
    }
    indexed_cats.push(format!("{SOURCE_NSR}.{}", role.as_str()));
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(LAYER_STOP_PLACE.to_string());
    append_zone_categories(&mut indexed_cats, sp, fare_zones);
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.alpha2));
    if let Some(gid) = &geography.county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &geography.locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    if let Some(mc) = multimodal_cat { indexed_cats.push(mc); }
    indexed_cats.push(as_category(&sp.id));

    StopCategories { visible: visible_cats, indexed: indexed_cats }
}

/// Append zone categories in 4 passes:
/// 1. Tariff zone IDs from the stop's `<TariffZoneRef>`s (`tariff_zone_id.RUT.TariffZone.1`)
/// 2. Fare zone IDs from the fare zone export (`fare_zone_id.RUT.FareZone.4`)
/// 3. Tariff zone authorities from the ref prefixes (`tariff_zone_authority.RUT`)
/// 4. Fare zone authorities from each zone's AuthorityRef (`fare_zone_authority.RUT.Authority.RUT`)
fn append_zone_categories(
    indexed_cats: &mut Vec<String>,
    sp: &StopPlaceXml,
    fare_zones: &[&FareZone],
) {
    for ref_ in tariff_zone_refs(sp) {
        indexed_cats.push(tariff_zone_id_category(ref_));
    }
    for zone in fare_zones {
        indexed_cats.push(fare_zone_id_category(&zone.id));
    }
    // Zone refs follow the pattern "AUTHORITY:TariffZone:NUMBER", so the authority
    // is the prefix before the first colon.
    let mut seen_tz_auth = HashSet::new();
    for ref_ in tariff_zone_refs(sp) {
        if let Some(auth) = ref_.split(':').next() {
            let cat = format!("{TARIFF_ZONE_AUTH_PREFIX}{auth}");
            if seen_tz_auth.insert(cat.clone()) {
                indexed_cats.push(cat);
            }
        }
    }
    let mut seen_fz_auth = HashSet::new();
    for auth_ref in fare_zones.iter().filter_map(|z| z.authority.as_deref()) {
        let cat = fare_zone_authority_category(auth_ref);
        if seen_fz_auth.insert(cat.clone()) {
            indexed_cats.push(cat);
        }
    }
}

/// The stop's `:TariffZone:`-shaped refs. NSR's mirrored `:FareZone:` refs are dropped; the
/// export is the source for those.
fn tariff_zone_refs(sp: &StopPlaceXml) -> impl Iterator<Item = &str> {
    sp.tariff_zones.iter()
        .flat_map(|tz| tz.refs.iter())
        .filter_map(|r| r.ref_.as_deref())
        .filter(|ref_| !ref_.contains(":FareZone:"))
}

/// Indexed alt names: the stop's own alternative names plus, for a multimodal parent, each
/// child's name and alternative names. Without the children's aliases a parent misses queries
/// its children match (e.g. "Nasjonalteatret"), and the proxy's default `multimodal=parent`
/// then filters away the only hits.
fn build_stop_alt_names(sp: &StopPlaceXml, sp_name: &str, child_stops: &[&StopPlaceXml]) -> Vec<String> {
    let mut alt_names: Vec<String> = alt_stop_names(sp, sp_name, None);
    for cs in child_stops {
        alt_names.extend(cs.name.clone());
        alt_names.extend(alt_stop_names(cs, sp_name, None));
    }
    dedup_preserve_order(&mut alt_names);
    alt_names
}

#[allow(clippy::too_many_arguments)]
fn build_stop_extra(
    sp: &StopPlaceXml,
    role: &StopPlaceRole,
    country: &Country,
    geography: &StopGeography,
    visible_alt: &[String],
    visible_cats: &[String],
    inferred_types: &[String],
    child_stops: &[&StopPlaceXml],
    fare_zones: &[&FareZone],
) -> Extra {
    let stop_place_role = role.as_str();
    let tariff_refs: Vec<String> = tariff_zone_refs(sp).map(String::from).collect();
    let fare_refs: Vec<String> = fare_zones.iter().map(|z| z.id.clone()).collect();
    let (tariff_zone_list, fare_zone_list) = (join_osm_values(&tariff_refs), join_osm_values(&fare_refs));

    let description = sp.description.as_ref()
        .filter(|d| !d.is_empty())
        .map(|d| {
            let eng = translator::translate(d);
            format!("nor:{d};eng:{eng}")
        });

    let transport_mode = collect_transport_modes(sp, child_stops);

    let stop_place_type_str = if inferred_types.is_empty() {
        None
    } else {
        Some(inferred_types.join(OSM_TAG_SEPARATOR))
    };

    Extra {
        id: Some(sp.id.clone()),
        source: Some("nsr".to_string()),
        accuracy: Some("point".to_string()),
        country_a: Some(country.alpha3.clone()),
        county_gid: geography.county_gid.clone(),
        locality: geography.locality.clone(),
        locality_gid: geography.locality_gid.clone(),
        tariff_zones: tariff_zone_list,
        fare_zones: fare_zone_list,
        alt_name: join_osm_values(visible_alt),
        description,
        tags: join_osm_values(visible_cats),
        transport_mode,
        stop_place_type: stop_place_type_str,
        stop_place_role: Some(stop_place_role.to_string()),
        ..Default::default()
    }
}

pub(crate) fn convert_gosp(
    stop_place: &StopPlaceConfig,
    importance_calc: &ImportanceCalculator,
    gosp: &GroupOfStopPlacesXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_popularities: &HashMap<String, i64>,
    stop_by_id: &HashMap<&str, &StopPlaceXml>,
    is_secondary: bool,
) -> Option<NominatimPlace> {
    let centroid_xml = gosp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let group_name = gosp.name.as_deref()?;

    let GospGeography { locality, locality_gid, county, county_gid } =
        resolve_gosp_geography(gosp, topo_places, stop_by_id);

    let gosp_pop = calculate_gosp_popularity(gosp, stop_popularities);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);
    // Two-sided clamp within the Nominatim 0-1 spec: secondary GoSPs ride the configured floor
    // (never below) so they sink under real stops; non-secondary GoSPs are capped at
    // `GOSP_IMPORTANCE_CAP` (never above) so they don't swallow an exact member-name match.
    let raw_importance = if is_secondary {
        SECONDARY_GOSP_IMPORTANCE.max(IMPORTANCE_FLOOR)
    } else {
        let capped = importance_calc.calculate_importance_for(&gosp.id, gosp_pop).min(GOSP_IMPORTANCE_CAP);
        apply_foreign_penalty(capped, &country, stop_place.foreign_importance_factor)
    };
    let importance = RawNumber::from_f64_6dp(raw_importance);

    let visible_cats = vec![
        LAYER_GOSP.to_string(),
        LEGACY_LAYER_ADDRESS.to_string(),
        LEGACY_SOURCE_WHOSONFIRST.to_string(),
        format!("{LEGACY_CATEGORY_PREFIX}{GOSP}"),
    ];
    let mut indexed_cats = visible_cats.clone();
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.alpha2));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    indexed_cats.push(as_category(&gosp.id));

    // Secondary GoSPs use rank_address 0 so Photon maps them to AddressType.OTHER instead of
    // HOUSE. In the autocomplete short-query path (`SearchQueryBuilder.setupShortQuery`), every
    // doc whose `OBJECT_TYPE != "other"` earns a +0.4 function-score weight; secondary GoSPs
    // forfeit that boost. Combined with the importance cap, this is enough to push the bare
    // "Bergen" GoSP below real Bergen stops without modifying any user-visible field.
    let rank_address = if is_secondary { 0 } else { stop_place.group_of_stop_places.rank_address };

    // Only member names, not their alternative names: a group's importance beats any member's,
    // so an inherited alias makes the group outrank the stop that actually carries it.
    let mut member_names: Vec<String> = gosp.members.as_ref()
        .map(|m| m.refs.iter()
            .filter_map(|r| stop_by_id.get(r.ref_.as_str()).copied())
            .filter_map(|sp| sp.name.clone())
            .filter(|n| n != group_name)
            .collect())
        .unwrap_or_default();
    dedup_preserve_order(&mut member_names);

    // Real extent over the member stops' coordinates (plus the group's own
    // centroid). Photon serves it as a per-feature `extent`; zero-area boxes
    // (no resolvable members) are skipped at index time.
    let mut min_lon = f64::INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for c in gosp.members.as_ref()
        .map(|m| m.refs.as_slice()).unwrap_or_default().iter()
        .filter_map(|r| stop_by_id.get(r.ref_.as_str()).copied())
        .filter_map(|sp| sp.centroid.as_ref())
        .map(|c| Coordinate::new(c.location.latitude, c.location.longitude))
        .chain(std::iter::once(coord))
    {
        min_lon = min_lon.min(c.lon);
        min_lat = min_lat.min(c.lat);
        max_lon = max_lon.max(c.lon);
        max_lat = max_lat.max(c.lat);
    }
    let bbox = vec![min_lon, min_lat, max_lon, max_lat];

    Some(NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&gosp.id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(group_name.to_string()),
                name_en: None,
                alt_name: join_osm_values(&member_names),
            }),
            address: Address { city: locality.clone(), county: county.clone(), ..Default::default() },
            housenumber: None,
            postcode: None,
            country_code: Some(country.alpha2.clone()),
            centroid: coord.centroid(),
            bbox,
            extra: Extra {
                id: Some(gosp.id.clone()),
                source: Some("nsr".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.alpha3),
                county_gid,
                locality,
                locality_gid,
                tags: join_osm_values(&visible_cats),
                ..Default::default()
            },
        }],
    })
}

struct GospGeography {
    locality: Option<String>,
    locality_gid: Option<String>,
    county: Option<String>,
    county_gid: Option<String>,
}

fn resolve_gosp_geography(
    gosp: &GroupOfStopPlacesXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_by_id: &HashMap<&str, &StopPlaceXml>,
) -> GospGeography {
    let group_name = gosp.name.as_deref().unwrap_or_default();
    let mut geo = GospGeography {
        locality: Some(group_name.to_string()),
        locality_gid: None,
        county: None,
        county_gid: None,
    };

    if let Some(members) = &gosp.members {
        for sp_ref in &members.refs {
            if let Some(sp) = stop_by_id.get(sp_ref.ref_.as_str()).copied()
                && let Some(topo_ref) = sp.topographic_place_ref.as_ref()
                && let Some(tp) = topo_places.get(&topo_ref.ref_)
                && tp.topographic_place_type.as_deref() == Some("municipality")
            {
                geo.locality_gid = Some(topo_ref.ref_.clone());
                geo.locality = tp.descriptor.as_ref().and_then(|d| d.name.clone());
                geo.county_gid = tp.parent_ref.as_ref().map(|r| r.ref_.clone());
                geo.county = geo.county_gid.as_ref().and_then(|gid| {
                    topo_places.get(gid).and_then(|tp2| tp2.descriptor.as_ref()?.name.clone())
                });
                break;
            }
        }
    }

    geo
}

/// GoSP popularity is the product of its members' popularities (empty product: 1.0,
/// the importance floor for GoSPs whose members couldn't be resolved). The caller
/// applies the group's own usage boost and the `GOSP_IMPORTANCE_CAP`.
fn calculate_gosp_popularity(
    gosp: &GroupOfStopPlacesXml,
    stop_popularities: &HashMap<String, i64>,
) -> f64 {
    let Some(members) = gosp.members.as_ref() else { return 1.0 };
    members.refs.iter()
        .filter_map(|r| stop_popularities.get(&r.ref_).copied())
        .fold(1.0, |acc, p| acc * p as f64)
}

fn determine_country(
    topo_places: &HashMap<String, TopographicPlaceXml>,
    sp: &StopPlaceXml,
    coord: &Coordinate,
) -> Country {
    if let Some(topo_ref) = sp.topographic_place_ref.as_ref()
        && let Some(tp) = topo_places.get(&topo_ref.ref_)
        && let Some(cr) = &tp.country_ref
        && let Some(c) = Country::parse(&cr.ref_)
    {
        return c;
    }
    geo::get_country(coord).unwrap_or_else(Country::no)
}

pub(crate) fn dedup_preserve_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

/// `exclude_name` is the primary name of the document being built, which may be a parent or
/// group rather than `sp` itself.
pub(crate) fn alt_stop_names(
    sp: &StopPlaceXml,
    exclude_name: &str,
    name_type_filter: Option<&str>,
) -> Vec<String> {
    let Some(alt_names) = &sp.alternative_names else { return Vec::new() };
    alt_names.names.iter()
        .filter(|an| name_type_filter.is_none() || an.name_type.as_deref() == name_type_filter)
        .filter_map(|an| an.name.as_ref())
        .filter(|n| n.as_str() != exclude_name && !n.is_empty())
        .cloned()
        .collect()
}

pub(crate) fn format_transport_mode(sp: &StopPlaceXml) -> Option<String> {
    let mode = sp.transport_mode.as_ref()?;
    let submode = sp.bus_submode.as_ref()
        .or(sp.tram_submode.as_ref())
        .or(sp.rail_submode.as_ref())
        .or(sp.metro_submode.as_ref())
        .or(sp.air_submode.as_ref())
        .or(sp.water_submode.as_ref())
        .or(sp.telecabin_submode.as_ref());
    Some(match submode {
        Some(sub) => format!("{mode}:{sub}"),
        None => mode.clone(),
    })
}

pub(crate) fn collect_transport_modes(sp: &StopPlaceXml, child_stops: &[&StopPlaceXml]) -> Option<String> {
    let own = format_transport_mode(sp);
    let child_modes: Vec<String> = child_stops.iter().filter_map(|cs| format_transport_mode(cs)).collect();
    let mut all: Vec<String> = own.into_iter().chain(child_modes).collect();
    dedup_preserve_order(&mut all);
    if all.is_empty() { None } else { Some(all.join(OSM_TAG_SEPARATOR)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::helpers::*;

    static EMPTY_USAGE: std::sync::LazyLock<UsageBoost> =
        std::sync::LazyLock::new(UsageBoost::empty);

    fn stop_line<'a>(content: &'a str, stop_id: &str) -> &'a str {
        content.lines()
            .find(|l| l.contains(&format!("\"id\":\"{stop_id}\"")))
            .unwrap_or_else(|| panic!("no output line for {stop_id}"))
    }

    // ===== Foreign importance penalty tests =====

    #[test]
    fn foreign_penalty_leaves_norwegian_untouched() {
        assert_eq!(apply_foreign_penalty(0.92, &Country::no(), 0.6), 0.92);
    }

    #[test]
    fn foreign_penalty_scales_non_norwegian() {
        // A foreign GoSP at the cap (0.92) with a 0.6 factor lands below a domestic city;
        // Country::se() stands in for any non-Norwegian country.
        assert!((apply_foreign_penalty(0.92, &Country::se(), 0.6) - 0.552).abs() < 1e-9);
    }

    #[test]
    fn foreign_penalty_floors_result() {
        // A tiny foreign importance times a small factor can't fall below the floor.
        assert_eq!(apply_foreign_penalty(IMPORTANCE_FLOOR, &Country::se(), 0.1), IMPORTANCE_FLOOR);
    }

    #[test]
    fn foreign_penalty_factor_one_is_noop() {
        assert_eq!(apply_foreign_penalty(0.5, &Country::se(), 1.0), 0.5);
    }

    // ===== Transport mode formatting tests =====

    #[test]
    fn transport_mode_with_bus_submode() {
        let sp = make_stop_place_with_submode("1", "bus", Some("localBus"), None, None);
        assert_eq!(format_transport_mode(&sp), Some("bus:localBus".to_string()));
    }

    #[test]
    fn transport_mode_with_rail_submode() {
        let sp = make_stop_place_with_submode("1", "rail", None, Some("highSpeedRail"), None);
        assert_eq!(format_transport_mode(&sp), Some("rail:highSpeedRail".to_string()));
    }

    #[test]
    fn transport_mode_without_submode() {
        let sp = make_stop_place("1", "Test", Some("bus"), Some("onstreetBus"));
        assert_eq!(format_transport_mode(&sp), Some("bus".to_string()));
    }

    #[test]
    fn parent_collects_child_transport_modes() {
        let parent = make_stop_place("1", "Parent", Some("rail"), Some("railStation"));
        let child_bus = make_stop_place_with_submode("2", "bus", Some("localBus"), None, None);
        let child_tram = make_stop_place("3", "Tram", Some("tram"), None);
        let child_refs: Vec<&StopPlaceXml> = vec![&child_bus, &child_tram];
        let result = collect_transport_modes(&parent, &child_refs);
        assert_eq!(result, Some("rail;bus:localBus;tram".to_string()));
    }

    #[test]
    fn parent_preserves_duplicate_mode_keys_with_different_submodes() {
        let parent = make_stop_place_with_submode("1", "tram", None, None, Some("cityTram"));
        let child = make_stop_place("2", "Tram", Some("tram"), None);
        let child_refs: Vec<&StopPlaceXml> = vec![&child];
        let result = collect_transport_modes(&parent, &child_refs);
        assert_eq!(result, Some("tram:cityTram;tram".to_string()));
    }

    #[test]
    fn standalone_has_only_own_transport_mode() {
        let sp = make_stop_place_with_submode("1", "bus", Some("localBus"), None, None);
        let result = collect_transport_modes(&sp, &[]);
        assert_eq!(result, Some("bus:localBus".to_string()));
    }

    // ===== Alternative names tests =====

    #[test]
    fn only_label_visible_in_extra_alt_name() {
        let sp = make_stop_place_with_alt_names("1", "Oslo S", vec![
            ("Oslo Sentralstasjon", Some("label")),
            ("Oslo Central Station", Some("translation")),
            ("Jernbanetorget", None),
        ]);
        let visible = alt_stop_names(&sp, "Oslo S", Some("label"));
        let indexed = alt_stop_names(&sp, "Oslo S", None);
        assert!(visible.contains(&"Oslo Sentralstasjon".to_string()));
        assert!(!visible.contains(&"Oslo Central Station".to_string()));
        assert!(!visible.contains(&"Jernbanetorget".to_string()));
        assert!(indexed.contains(&"Oslo Sentralstasjon".to_string()));
        assert!(indexed.contains(&"Oslo Central Station".to_string()));
        assert!(indexed.contains(&"Jernbanetorget".to_string()));
    }

    #[test]
    fn alt_names_empty_when_none() {
        let sp = make_stop_place("1", "Simple Stop", None, None);
        let result = alt_stop_names(&sp, "Simple Stop", None);
        assert!(result.is_empty());
    }

    #[test]
    fn alt_names_exclude_primary_name() {
        let sp = make_stop_place_with_alt_names("1", "Oslo S", vec![
            ("Oslo S", Some("label")),
            ("Oslo Central", Some("translation")),
        ]);
        let result = alt_stop_names(&sp, "Oslo S", None);
        assert!(!result.contains(&"Oslo S".to_string()));
        assert!(result.contains(&"Oslo Central".to_string()));
    }

    #[test]
    fn parent_alt_name_inherits_child_names_and_aliases() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);
        let parent = make_stop_place("NSR:StopPlace:58404", "Nationaltheatret", None, None);
        let child_rail = make_stop_place_with_alt_names("NSR:StopPlace:288", "Nationaltheatret stasjon", vec![
            ("Nationaltheatret", Some("alias")),        // equals the parent name: dropped
            ("Nasjonalteatret", Some("alias")),
            ("National Theatre", Some("translation")), // all name types are indexed, not just aliases
        ]);
        let child_tram = make_stop_place_with_alt_names("NSR:StopPlace:4081", "Nationaltheatret", vec![
            ("Nasjonalteatret", Some("alias")),
        ]);
        let result = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &parent, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[&child_rail, &child_tram],
        ).unwrap();
        // Trailing "Nationaltheatret" is the tram child's name: child names are only deduped,
        // not filtered against the parent's name.
        assert_eq!(
            result.content[0].name.as_ref().unwrap().alt_name.as_deref(),
            Some("Nationaltheatret stasjon;Nasjonalteatret;National Theatre;Nationaltheatret"),
        );
    }

    // ===== Category tests =====

    #[test]
    fn funicular_transport_mode_included_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("funicular"), Some("other"));
        let result = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &sp, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.other"));
    }

    #[test]
    fn bus_transport_mode_not_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("bus"), Some("onstreetBus"));
        let result = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &sp, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(!cats.iter().any(|c| c == "legacy.category.bus"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
    }

    #[test]
    fn stop_place_types_indexed_as_first_class_facet() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("rail"), Some("railStation"));
        let result = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &sp, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "stop_place_type.railStation"), "{cats:?}");
    }

    #[test]
    fn parent_stop_includes_child_types_and_multimodal_category() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:Parent", "Hub", Some("funicular"), Some("other"));
        let mut child_types_map: HashMap<String, Vec<String>> = HashMap::new();
        child_types_map.insert("NSR:StopPlace:Parent".to_string(),
            vec!["onstreetBus".to_string(), "railStation".to_string(), "metroStation".to_string()]);
        let result = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &sp, &HashMap::new(),
            &child_types_map, &FareZones::empty(), 50, &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
        assert!(cats.iter().any(|c| c == "legacy.category.railStation"));
        assert!(cats.iter().any(|c| c == "legacy.category.metroStation"));
        // Child types also land in the v3 facet, so a multimodal parent hub
        // matches stopPlaceTypes filters for its children's types.
        assert!(cats.iter().any(|c| c == "stop_place_type.railStation"));
        assert!(cats.iter().any(|c| c == "stop_place_type.metroStation"));
        assert!(cats.iter().any(|c| c == "multimodal.parent"));
        // First-class per-feature role surfaced to the v3 proxy via extra.
        assert_eq!(result.content[0].extra.stop_place_role.as_deref(), Some("parent"));
    }

    #[test]
    fn extra_stop_place_role_matches_classification() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        // Standalone: no children, no parent ref.
        let standalone = make_stop_place("NSR:StopPlace:Solo", "Solo", Some("bus"), Some("onstreetBus"));
        let res = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &standalone, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[],
        ).unwrap();
        assert_eq!(res.content[0].extra.stop_place_role.as_deref(), Some("standalone"));

        // Child: carries a ParentSiteRef.
        let mut child = make_stop_place("NSR:StopPlace:Kid", "Kid", Some("bus"), Some("onstreetBus"));
        child.parent_site_ref = Some(RefAttr { ref_: "NSR:StopPlace:Parent".to_string() });
        let res = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &child, &HashMap::new(),
            &HashMap::new(), &FareZones::empty(), 50, &[],
        ).unwrap();
        assert_eq!(res.content[0].extra.stop_place_role.as_deref(), Some("child"));

        // Children win over a parent ref (not expected in NSR, but pinned).
        let mut both = make_stop_place("NSR:StopPlace:Both", "Both", Some("bus"), Some("onstreetBus"));
        both.parent_site_ref = Some(RefAttr { ref_: "NSR:StopPlace:Grandparent".to_string() });
        let mut child_types = HashMap::new();
        child_types.insert("NSR:StopPlace:Both".to_string(), vec!["onstreetBus".to_string()]);
        let res = convert_stop_place(
            config.stop_place.as_ref().unwrap(), &importance_calc, &both, &HashMap::new(),
            &child_types, &FareZones::empty(), 50, &[],
        ).unwrap();
        assert_eq!(res.content[0].extra.stop_place_role.as_deref(), Some("parent"));
    }

    // ===== Full conversion tests =====

    #[test]
    fn convert_stop_places_xml_produces_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_stopplace_convert_output.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NominatimDumpFile"));
        assert!(content.contains("NSR:StopPlace:56697"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn convert_produces_group_of_stop_places() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_gosp_convert_output.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NSR:GroupOfStopPlaces:1"));
        assert!(content.contains("NSR:GroupOfStopPlaces:72"));
        assert!(content.contains("\"name\":\"Oslo\""));
        assert!(content.contains(LAYER_GOSP));

        // stop_place_role is stop-place-only: GOSP lines must omit it, stop lines must carry it.
        // Keyed on the layer tag so GOSP member refs mentioning stop ids don't false-match.
        for line in content.lines().filter(|l| l.contains(LAYER_GOSP)) {
            assert!(!line.contains("\"stop_place_role\""), "GOSP must not carry stop_place_role: {line}");
        }
        assert!(
            content.lines().any(|l| l.contains("\"stop_place_role\"")),
            "expected at least one stop place to carry stop_place_role",
        );

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn gosp_alt_name_contains_member_stop_names_not_id() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        let oslo_s = make_stop_place("NSR:StopPlace:59872", "Oslo S", Some("rail"), Some("railStation"));
        let oslo_bus = make_stop_place("NSR:StopPlace:58366", "Oslo Bussterminal", Some("bus"), Some("busStation"));
        let stop_by_id: HashMap<&str, &StopPlaceXml> = HashMap::from([
            ("NSR:StopPlace:59872", &oslo_s),
            ("NSR:StopPlace:58366", &oslo_bus),
        ]);

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:1".to_string(),
            name: Some("Oslo".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 10.75, latitude: 59.91 } }),
            members: Some(MembersXml {
                refs: vec![
                    RefAttr { ref_: "NSR:StopPlace:59872".to_string() },
                    RefAttr { ref_: "NSR:StopPlace:58366".to_string() },
                ],
            }),
        };

        let result = convert_gosp(
            config.stop_place.as_ref().unwrap(), &importance_calc, &gosp, &HashMap::new(),
            &HashMap::new(), &stop_by_id, false,
        ).unwrap();

        let alt_name = result.content[0].name.as_ref().unwrap().alt_name.as_deref().unwrap_or("");
        assert!(alt_name.contains("Oslo S"), "alt_name should include member 'Oslo S': {alt_name}");
        assert!(alt_name.contains("Oslo Bussterminal"), "alt_name should include member 'Oslo Bussterminal': {alt_name}");
        assert!(!alt_name.contains("NSR:GroupOfStopPlaces:1"), "alt_name must not contain the GoSP id: {alt_name}");
    }

    #[test]
    fn gosp_alt_name_excludes_member_alt_names() {
        // Deliberate: inheriting "Oslo Sentralstasjon" would make the 0.92-importance group
        // outrank Oslo S itself on that query. Unlike a multimodal parent's children, members
        // are never hidden by the proxy's multimodal=parent filter, so there is nothing to fix.
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        let oslo_s = make_stop_place_with_alt_names("NSR:StopPlace:59872", "Oslo S", vec![
            ("Oslo Sentralstasjon", Some("alias")),
        ]);
        let stop_by_id: HashMap<&str, &StopPlaceXml> =
            HashMap::from([("NSR:StopPlace:59872", &oslo_s)]);

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:1".to_string(),
            name: Some("Oslo".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 10.75, latitude: 59.91 } }),
            members: Some(MembersXml {
                refs: vec![RefAttr { ref_: "NSR:StopPlace:59872".to_string() }],
            }),
        };

        let result = convert_gosp(
            config.stop_place.as_ref().unwrap(), &importance_calc, &gosp, &HashMap::new(),
            &HashMap::new(), &stop_by_id, false,
        ).unwrap();

        let alt_name = result.content[0].name.as_ref().unwrap().alt_name.as_deref();
        assert_eq!(alt_name, Some("Oslo S"));
    }

    #[test]
    fn gosp_bbox_spans_member_stops() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        let mut west = make_stop_place("NSR:StopPlace:1", "West", Some("bus"), None);
        west.centroid = Some(CentroidXml { location: LocationXml { longitude: 10.70, latitude: 59.90 } });
        let mut east = make_stop_place("NSR:StopPlace:2", "East", Some("bus"), None);
        east.centroid = Some(CentroidXml { location: LocationXml { longitude: 10.80, latitude: 59.95 } });
        let stop_by_id: HashMap<&str, &StopPlaceXml> = HashMap::from([
            ("NSR:StopPlace:1", &west),
            ("NSR:StopPlace:2", &east),
        ]);

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:9".to_string(),
            name: Some("Testby".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 10.75, latitude: 59.92 } }),
            members: Some(MembersXml {
                refs: vec![
                    RefAttr { ref_: "NSR:StopPlace:1".to_string() },
                    RefAttr { ref_: "NSR:StopPlace:2".to_string() },
                ],
            }),
        };

        let result = convert_gosp(
            config.stop_place.as_ref().unwrap(), &importance_calc, &gosp, &HashMap::new(),
            &HashMap::new(), &stop_by_id, false,
        ).unwrap();

        let bbox = &result.content[0].bbox;
        assert_eq!(bbox, &vec![10.70, 59.90, 10.80, 59.95], "bbox must span the member stops");
    }

    #[test]
    fn convert_caps_non_secondary_gosp_importance() {
        // A hub whose member popularities multiply past the importance ceiling must be pinned at
        // GOSP_IMPORTANCE_CAP, not 1.0, so an exact match on a member stop can still outrank it.
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        let a = make_stop_place("NSR:StopPlace:337", "Oslo S", Some("rail"), Some("railStation"));
        let b = make_stop_place("NSR:StopPlace:58293", "Oslo Bussterminal", Some("bus"), Some("busStation"));
        let stop_by_id: HashMap<&str, &StopPlaceXml> =
            HashMap::from([("NSR:StopPlace:337", &a), ("NSR:StopPlace:58293", &b)]);
        let pops = HashMap::from([
            ("NSR:StopPlace:337".to_string(), 250_000_i64),
            ("NSR:StopPlace:58293".to_string(), 1_000_i64),
        ]); // product = 2.5e8 -> importance saturates well above the cap

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:1".to_string(),
            name: Some("Oslo".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 10.75, latitude: 59.91 } }),
            members: Some(MembersXml {
                refs: vec![
                    RefAttr { ref_: "NSR:StopPlace:337".to_string() },
                    RefAttr { ref_: "NSR:StopPlace:58293".to_string() },
                ],
            }),
        };

        let result = convert_gosp(
            config.stop_place.as_ref().unwrap(), &importance_calc, &gosp, &HashMap::new(),
            &pops, &stop_by_id, false,
        ).unwrap();

        assert_eq!(result.content[0].importance.0, RawNumber::from_f64_6dp(GOSP_IMPORTANCE_CAP).0,
            "saturating non-secondary GoSP importance must be capped at GOSP_IMPORTANCE_CAP");
    }

    #[test]
    fn convert_gosp_applies_foreign_penalty_after_cap() {
        // End-to-end check that apply_foreign_penalty is wired into convert_gosp: a saturating
        // GoSP in a non-Norwegian country (Stockholm centroid) is capped at GOSP_IMPORTANCE_CAP
        // and THEN penalized, so the two compose: 0.92 * 0.6 = 0.552.
        let sp: StopPlaceConfig = serde_json::from_str(
            r#"{ "input": { "file": "unused" }, "defaultValue": 50, "rankAddress": 30,
                 "foreignImportanceFactor": 0.6,
                 "stopTypeFactors": { "railStation": 2.0, "busStation": 2.0 },
                 "interchangeFactors": { "preferredInterchange": 10.0 },
                 "fareZones": { "input": { "file": "unused" } } }"#,
        ).unwrap();
        let importance_calc = ImportanceCalculator::new(&EMPTY_USAGE);

        let a = make_stop_place("NSR:StopPlace:337", "T-Centralen", Some("rail"), Some("railStation"));
        let b = make_stop_place("NSR:StopPlace:58293", "Cityterminalen", Some("bus"), Some("busStation"));
        let stop_by_id: HashMap<&str, &StopPlaceXml> =
            HashMap::from([("NSR:StopPlace:337", &a), ("NSR:StopPlace:58293", &b)]);
        let pops = HashMap::from([
            ("NSR:StopPlace:337".to_string(), 250_000_i64),
            ("NSR:StopPlace:58293".to_string(), 1_000_i64),
        ]); // product saturates well above the cap

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:1".to_string(),
            name: Some("Stockholm".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 18.0686, latitude: 59.3293 } }),
            members: Some(MembersXml {
                refs: vec![
                    RefAttr { ref_: "NSR:StopPlace:337".to_string() },
                    RefAttr { ref_: "NSR:StopPlace:58293".to_string() },
                ],
            }),
        };

        let result = convert_gosp(&sp, &importance_calc, &gosp, &HashMap::new(), &pops, &stop_by_id, false).unwrap();

        assert_eq!(result.content[0].importance.0, RawNumber::from_f64_6dp(GOSP_IMPORTANCE_CAP * 0.6).0,
            "foreign GoSP importance must be the cap (0.92) times foreignImportanceFactor (0.6)");
    }

    #[test]
    fn gosp_own_usage_entry_boosts_importance() {
        // The group's own usage entry multiplies the member product, so GoSP ranking
        // doesn't hinge solely on register membership.
        let config = test_config();
        let usage = UsageBoost::from_counts(&[("NSR:GroupOfStopPlaces:174", 10_000)], 0.5, 100);
        let importance_calc = ImportanceCalculator::new(&usage);

        let a = make_stop_place("NSR:StopPlace:30859", "Byparken", Some("tram"), Some("onstreetTram"));
        let stop_by_id: HashMap<&str, &StopPlaceXml> = HashMap::from([("NSR:StopPlace:30859", &a)]);
        let pops = HashMap::from([("NSR:StopPlace:30859".to_string(), 1_000_i64)]);

        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:174".to_string(),
            name: Some("Bergen sentrum".to_string()),
            centroid: Some(CentroidXml { location: LocationXml { longitude: 5.33, latitude: 60.39 } }),
            members: Some(MembersXml {
                refs: vec![RefAttr { ref_: "NSR:StopPlace:30859".to_string() }],
            }),
        };

        let result = convert_gosp(
            config.stop_place.as_ref().unwrap(), &importance_calc, &gosp, &HashMap::new(),
            &pops, &stop_by_id, false,
        ).unwrap();

        // factor(10_000) = 1 + 0.5*log10(10_000/100) = 2.0, so importance of 2_000, not 1_000
        let expected = importance_calc.calculate_importance(2_000.0);
        assert!(expected > importance_calc.calculate_importance(1_000.0));
        assert_eq!(result.content[0].importance.0, RawNumber::from_f64_6dp(expected).0,
            "GoSP importance must include the group's own usage boost");
    }

    #[test]
    fn gosp_popularity_floors_when_no_members_resolve() {
        let gosp = GroupOfStopPlacesXml {
            id: "NSR:GroupOfStopPlaces:1".to_string(),
            name: Some("Oslo".to_string()),
            centroid: None,
            members: Some(MembersXml {
                refs: vec![RefAttr { ref_: "NSR:StopPlace:missing".to_string() }],
            }),
        };
        // No resolvable members -> MIN_POPULARITY (1.0), which normalizes to the importance floor.
        assert_eq!(calculate_gosp_popularity(&gosp, &HashMap::new()), 1.0);
    }

    #[test]
    fn convert_caps_secondary_gosp_importance() {
        // Configures GoSP:1 (Oslo) as secondary and asserts both demotion levers fire:
        // importance capped, rank_address set to 0. GoSP:72 (Hammerfest) is the control - same
        // fixture, no demotion config, must keep its full importance and configured rank_address.
        let mut config = test_config();
        config.stop_place.as_mut().expect("stopplace config present").group_of_stop_places.secondary_gosps =
            vec!["NSR:GroupOfStopPlaces:1".to_string()];
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_secondary_gosp_cap.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        let _ = std::fs::remove_file(&output);

        let line_for = |id: &str| content.lines()
            .find(|l| l.contains(id) && l.contains("\"place_id\""))
            .unwrap_or_else(|| panic!("{id} not in output")).to_string();
        let importance_of = |line: &str| -> f64 {
            let key = "\"importance\":";
            let i = line.find(key).expect("importance field") + key.len();
            let rest = &line[i..];
            let end = rest.find(|c: char| c != '.' && c != '-' && !c.is_ascii_digit())
                .unwrap_or(rest.len());
            rest[..end].parse().unwrap()
        };

        let demoted = line_for("NSR:GroupOfStopPlaces:1");
        let kept = line_for("NSR:GroupOfStopPlaces:72");
        let expected_demoted = SECONDARY_GOSP_IMPORTANCE.max(IMPORTANCE_FLOOR);
        assert_eq!(importance_of(&demoted), expected_demoted,
            "demoted GoSP must be pinned at max(SECONDARY_GOSP_IMPORTANCE, floor)");
        assert!(importance_of(&kept) >= expected_demoted,
            "non-demoted GoSP importance must be at or above the secondary-GoSP floor");
        assert!(demoted.contains("\"rank_address\":0"),
            "demoted GoSP must have rank_address=0 (forfeits Photon's short-query +0.4 boost)");
        assert!(!kept.contains("\"rank_address\":0"),
            "non-demoted GoSP must keep its configured rank_address");
    }

    #[test]
    fn output_has_valid_json_on_each_line() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_valid_json.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap().lines().map(String::from).collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.starts_with('{'));
            assert!(line.ends_with('}'));
        }
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn all_stop_places_have_coordinates() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_coords.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap().lines()
            .filter(|l| l.contains("NSR:StopPlace:"))
            .map(String::from).collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.contains("\"centroid\":["));
        }
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_fare_zone_authority_categories() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_authority.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("fare_zone_authority.FIN.Authority.FIN_ID"));
        assert!(content.contains("fare_zone_authority.RUT.Authority.RUT"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn tariff_and_fare_zone_ids_indexed_under_distinct_prefixes() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_zone_id_split.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        // NSR's <TariffZoneRef>s land under tariff_zone_id.
        assert!(content.contains("tariff_zone_id.RUT.TariffZone.1"));
        assert!(content.contains("tariff_zone_id.FIN.TariffZone.54540"));
        // Fare zones come from the fare zone export, matched by outline.
        assert!(content.contains("fare_zone_id.RUT.FareZone.4"));
        assert!(content.contains("fare_zone_id.FIN.FareZone.31"));
        // NSR's mirrored :FareZone: refs are dropped, not re-indexed as tariff zones.
        assert!(!content.contains("tariff_zone_id.RUT.FareZone"));
        assert!(!content.contains("tariff_zone_id.FIN.FareZone"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn zones_come_from_the_export_not_from_nsr() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_zone_sources.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();

        // Nydalen: NSR gives it RUT:TariffZone:1; the export adds RUT:FareZone:4 by outline
        // and RUT:FareZone:13 by membership.
        let nydalen = stop_line(&content, "NSR:StopPlace:59649");
        assert!(nydalen.contains("\"tariff_zones\":\"RUT:TariffZone:1\""), "got: {nydalen}");
        assert!(nydalen.contains("\"fare_zones\":\"RUT:FareZone:13;RUT:FareZone:4\""), "got: {nydalen}");

        // Nyland carries a stale mirrored RUT:FareZone:99 in NSR that the export disagrees
        // with, and sits inside RUT:FareZone:13's outline without being a member. It should
        // end up with the export's answer only.
        let nyland = stop_line(&content, "NSR:StopPlace:305");
        assert!(nyland.contains("\"fare_zones\":\"RUT:FareZone:4\""), "got: {nyland}");
        assert!(!nyland.contains("FareZone:99"), "NSR's mirrored ref must be dropped: {nyland}");
        assert!(!nyland.contains("FareZone.13"), "explicitStops outline must not match: {nyland}");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_place_with_bus_submode_has_transport_mode_in_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_transport_mode.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("\"transport_mode\":\"bus:localBus\""));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_county_gid_and_locality_gid() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_gid.ndjson");
        convert_all(&config, &input, &output, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("county_gid.KVE"));
        assert!(content.contains("locality_gid.KVE"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn usage_csv_lifts_named_stop_importance() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");

        let baseline_out = std::env::temp_dir().join("test_usage_baseline.ndjson");
        convert_all(&config, &input, &baseline_out, false, Some(&test_data_path("fareZones.xml")), &UsageBoost::empty()).unwrap();
        let baseline = std::fs::read_to_string(&baseline_out).unwrap();
        let _ = std::fs::remove_file(&baseline_out);

        let csv = std::env::temp_dir().join("test_usage_boost_input.csv");
        std::fs::write(&csv, "id;name;usage\nNSR:StopPlace:56697;Oslo S;5000000\n").unwrap();
        let usage = UsageBoost::load(Some(&csv), crate::common::usage::DEFAULT_ALPHA, crate::common::usage::DEFAULT_USAGE_FLOOR).unwrap();
        let boosted_out = std::env::temp_dir().join("test_usage_boosted.ndjson");
        convert_all(&config, &input, &boosted_out, false, Some(&test_data_path("fareZones.xml")), &usage).unwrap();
        let boosted = std::fs::read_to_string(&boosted_out).unwrap();
        let _ = std::fs::remove_file(&boosted_out);
        let _ = std::fs::remove_file(&csv);

        let pick = |s: &str| -> f64 {
            let line = s.lines()
                .find(|l| l.contains("\"place_id\"") && l.contains("NSR:StopPlace:56697"))
                .expect("stop in output");
            let key = "\"importance\":";
            let i = line.find(key).expect("importance field") + key.len();
            let rest = &line[i..];
            let end = rest.find(|c: char| c != '.' && !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse().unwrap()
        };
        assert!(pick(&boosted) > pick(&baseline),
            "boosted importance {} should exceed baseline {}", pick(&boosted), pick(&baseline));
    }
}
