use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::{join_osm_values, OSM_TAG_SEPARATOR};
use crate::common::translator;
use crate::common::usage::UsageBoost;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Importance assigned to GoSPs listed in `secondaryGosps`. Must be strictly positive: Photon
/// adds importance to the document score after wrapping the query in a `function_score` with
/// `boostMode=Sum`. Empirically, a negative importance can drive the combined score below zero,
/// at which point Lucene clamps it and the document disappears from results. 0.001 is small
/// enough to be dwarfed by real stops' importance (~0.4-0.5) but stays comfortably positive.
/// Tied to Photon's scoring behavior, not deployment-specific, so kept as a constant.
const SECONDARY_GOSP_IMPORTANCE: f64 = 0.001;

use super::popularity::calculate_stop_popularity;
use super::xml::*;

pub fn convert_all(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
    usage: &UsageBoost,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(input)?;
    let result = parse_netex(&xml)?;
    let importance_calc = ImportanceCalculator::new(&config.importance, usage);

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
    // GoSPs inherit the signal automatically through the member-product propagation
    // in `calculate_gosp_popularity` so they don't need a separate lookup.
    let stop_popularities: HashMap<String, i64> = result.stop_places.iter().map(|sp| {
        let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
        let pop = calculate_stop_popularity(&config.stop_place, sp, &child_types, usage.factor(&sp.id));
        (sp.id.clone(), pop)
    }).collect();

    // Build child stop names and child stops maps
    let mut child_names: HashMap<String, Vec<String>> = HashMap::new();
    let mut child_stops: HashMap<String, Vec<&StopPlaceXml>> = HashMap::new();
    for sp in &result.stop_places {
        if let Some(parent_ref) = &sp.parent_site_ref {
            if let Some(name) = &sp.name {
                child_names.entry(parent_ref.ref_.clone()).or_default().push(name.clone());
            }
            child_stops.entry(parent_ref.ref_.clone()).or_default().push(sp);
        }
    }

    let mut entries = Vec::new();

    // Convert stop places
    for sp in &result.stop_places {
        let pop = stop_popularities.get(&sp.id).copied().unwrap_or(0);
        let child_stop_names = child_names.get(&sp.id).cloned().unwrap_or_default();
        let my_child_stops = child_stops.get(&sp.id).cloned().unwrap_or_default();

        if let Some(entry) = convert_stop_place(
            config, &importance_calc, sp, &result.topo_places,
            &stop_place_types, &result.fare_zones, pop,
            &child_stop_names, &my_child_stops,
        ) {
            entries.push(entry);
        }
    }

    let stop_by_id: HashMap<&str, &StopPlaceXml> =
        result.stop_places.iter().map(|sp| (sp.id.as_str(), sp)).collect();

    let secondary_gosps: HashSet<&str> = config.group_of_stop_places.secondary_gosps
        .iter().map(String::as_str).collect();
    if !secondary_gosps.is_empty() {
        let gosp_ids: HashSet<&str> = result.groups.iter().map(|g| g.id.as_str()).collect();
        let n = secondary_gosps.len();
        eprintln!("Demoting {n} configured secondary GoSP{}:", if n == 1 { "" } else { "s" });
        for gosp in &result.groups {
            if secondary_gosps.contains(gosp.id.as_str()) {
                eprintln!("  {} \"{}\"", gosp.id, gosp.name.as_deref().unwrap_or(""));
            }
        }
        for id in &secondary_gosps {
            if !gosp_ids.contains(id) {
                eprintln!("  warning: configured secondary GoSP {id} not found in input - typo?");
            }
        }
    }

    // Convert groups of stop places
    for gosp in &result.groups {
        let is_secondary = secondary_gosps.contains(gosp.id.as_str());
        if let Some(entry) = convert_gosp(
            config, &importance_calc, gosp, &result.topo_places,
            &stop_popularities, &stop_by_id, is_secondary,
        ) {
            entries.push(entry);
        }
    }

    JsonWriter::export(&entries, output, is_appending)?;
    Ok(())
}

/// A stop place's role in the parent-child hierarchy. Affects which source category
/// and multimodal marker are assigned in the output.
#[derive(Debug, PartialEq)]
enum StopPlaceRole {
    Child,
    Parent,
    Standalone,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_stop_place(
    config: &Config,
    importance_calc: &ImportanceCalculator,
    sp: &StopPlaceXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_place_types: &HashMap<String, Vec<String>>,
    fare_zones: &HashMap<String, FareZoneXml>,
    popularity: i64,
    child_stop_names: &[String],
    child_stops: &[&StopPlaceXml],
) -> Option<NominatimPlace> {
    let centroid_xml = sp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let sp_name = sp.name.as_deref()?;

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
    let country = determine_country(topo_places, sp, &coord);
    let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(popularity as f64));
    let role = classify_role(&child_types, sp.parent_site_ref.is_some());

    let inferred_types: Vec<String> = child_types
        .iter()
        .cloned()
        .chain(sp.stop_place_type.iter().cloned())
        .collect();

    let (visible_cats, indexed_cats) = build_stop_categories(
        sp, &role, &inferred_types, &country, &county_gid, &locality_gid, fare_zones,
    );

    let alt_names = build_stop_alt_names(sp, sp_name, child_stop_names);
    let visible_alt: Vec<String> = alt_stop_names(sp, sp_name, Some("label"));

    let entry = NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&sp.id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address: config.stop_place.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(sp_name.to_string()),
                name_en: None,
                alt_name: join_osm_values(&alt_names),
            }),
            address: Address { city: locality.clone(), county: county.clone(), ..Default::default() },
            housenumber: None,
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: build_stop_extra(
                sp, &country, &county_gid, &locality, &locality_gid,
                &visible_alt, &visible_cats, &inferred_types, child_stops,
            ),
        }],
    };
    Some(entry)
}

/// Determine role: if this stop has children → Parent, if it references a parent → Child,
/// otherwise → Standalone.
fn classify_role(child_types: &[String], has_parent: bool) -> StopPlaceRole {
    if !child_types.is_empty() {
        StopPlaceRole::Parent
    } else if has_parent {
        StopPlaceRole::Child
    } else {
        StopPlaceRole::Standalone
    }
}

fn build_stop_categories(
    sp: &StopPlaceXml,
    role: &StopPlaceRole,
    inferred_types: &[String],
    country: &Country,
    county_gid: &Option<String>,
    locality_gid: &Option<String>,
    fare_zones: &HashMap<String, FareZoneXml>,
) -> (Vec<String>, Vec<String>) {
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
    }
    indexed_cats.push(format!("{SOURCE_NSR}.{}", match role {
        StopPlaceRole::Child => "child",
        StopPlaceRole::Parent => "parent",
        StopPlaceRole::Standalone => "standalone",
    }));
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(LAYER_STOP_PLACE.to_string());
    append_tariff_zone_categories(&mut indexed_cats, sp, fare_zones);
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    if let Some(mc) = multimodal_cat { indexed_cats.push(mc); }
    indexed_cats.push(as_category(&sp.id));

    (visible_cats, indexed_cats)
}

/// Append tariff/fare zone categories in 3 passes:
/// 1. Zone IDs, split by ref shape:
///    - `:TariffZone:` refs go to `tariff_zone_id.RUT.TariffZone.1`
///    - `:FareZone:` refs go to `fare_zone_id.RUT.FareZone.4`
/// 2. Zone authorities extracted from `:TariffZone:` ref prefixes (e.g. `tariff_zone_authority.RUT`)
/// 3. Fare zone authorities from the FareZone → AuthorityRef lookup (e.g. `fare_zone_authority.RUT.Authority.RUT`)
fn append_tariff_zone_categories(
    indexed_cats: &mut Vec<String>,
    sp: &StopPlaceXml,
    fare_zones: &HashMap<String, FareZoneXml>,
) {
    let Some(tz) = &sp.tariff_zones else { return };
    // Pass 1: zone IDs - split by ref shape so callers can filter the two namespaces separately.
    // NeTEx codespace types are a stable, finite vocabulary; `TariffZone` and `FareZone` are the
    // two shapes that appear under <TariffZones>. Anything else falls through to tariff_zone_id.
    //
    // The `:FareZone:` substring branch MUST stay in sync with
    // geocoder/proxy/src/main/kotlin/no/entur/geocoder/proxy/photon/PhotonFilterBuilder.kt
    // (v2 tariffZones routing) - both decide TariffZone vs FareZone the same way.
    for tz_ref in &tz.refs {
        if let Some(ref_) = &tz_ref.ref_ {
            if ref_.contains(":FareZone:") {
                indexed_cats.push(fare_zone_id_category(ref_));
            } else {
                indexed_cats.push(tariff_zone_id_category(ref_));
            }
        }
    }
    // Pass 2: tariff zone authorities (deduplicated).
    // Zone refs follow the pattern "AUTHORITY:TariffZone:NUMBER", so the authority
    // is the prefix before the first colon.
    let mut seen_tz_auth = std::collections::HashSet::new();
    for tz_ref in &tz.refs {
        if let Some(ref_) = &tz_ref.ref_
            && ref_.contains(":TariffZone:")
            && let Some(auth) = ref_.split(':').next()
        {
            let cat = format!("{TARIFF_ZONE_AUTH_PREFIX}{auth}");
            if seen_tz_auth.insert(cat.clone()) {
                indexed_cats.push(cat);
            }
        }
    }
    // Pass 3: fare zone authorities (deduplicated)
    let mut seen_fz_auth = std::collections::HashSet::new();
    for tz_ref in &tz.refs {
        if let Some(ref_) = &tz_ref.ref_
            && let Some(fz) = fare_zones.get(ref_.as_str())
            && let Some(auth_ref) = fz.authority_ref.as_ref().map(|a| a.ref_.as_str())
        {
            let cat = fare_zone_authority_category(auth_ref);
            if seen_fz_auth.insert(cat.clone()) {
                indexed_cats.push(cat);
            }
        }
    }
}

fn build_stop_alt_names(sp: &StopPlaceXml, sp_name: &str, child_stop_names: &[String]) -> Vec<String> {
    let mut alt_names: Vec<String> = alt_stop_names(sp, sp_name, None);
    alt_names.extend_from_slice(child_stop_names);
    dedup_preserve_order(&mut alt_names);
    alt_names
}

#[allow(clippy::too_many_arguments)]
fn build_stop_extra(
    sp: &StopPlaceXml,
    country: &Country,
    county_gid: &Option<String>,
    locality: &Option<String>,
    locality_gid: &Option<String>,
    visible_alt: &[String],
    visible_cats: &[String],
    inferred_types: &[String],
    child_stops: &[&StopPlaceXml],
) -> Extra {
    // Split the stop place's <TariffZoneRef>s by ref shape into two output fields, mirroring the
    // category-prefix split in `append_tariff_zone_categories` so downstream consumers can read
    // each namespace cleanly without substring inspection.
    let (tariff_zone_list, fare_zone_list) = sp.tariff_zones.as_ref()
        .map(|tz| {
            let (fare_refs, tariff_refs): (Vec<String>, Vec<String>) = tz.refs.iter()
                .filter_map(|r| r.ref_.clone())
                .partition(|ref_| ref_.contains(":FareZone:"));
            (join_osm_values(&tariff_refs), join_osm_values(&fare_refs))
        })
        .unwrap_or((None, None));

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
        country_a: Some(country.three_letter_code.clone()),
        county_gid: county_gid.clone(),
        locality: locality.clone(),
        locality_gid: locality_gid.clone(),
        tariff_zones: tariff_zone_list,
        fare_zones: fare_zone_list,
        alt_name: join_osm_values(visible_alt),
        description,
        tags: join_osm_values(visible_cats),
        transport_mode,
        stop_place_type: stop_place_type_str,
        ..Default::default()
    }
}

pub(crate) fn convert_gosp(
    config: &Config,
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

    let gos_pop = calculate_gosp_popularity(gosp, stop_popularities);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);
    // GoSP popularity grows multiplicatively with member count and easily exceeds
    // `importance.maxPopularity`. For home-country GoSPs we use the unclamped variant and
    // apply the configured multiplier so major Norwegian cities (Bergen, Trondheim) outrank
    // near-focus streets that share the same name prefix. Foreign GoSPs (e.g. NSR's Berlin ZOB
    // entry for international bus routes) keep the clamped 0-1 importance so they don't
    // outrank Norwegian cities for users searching in Norway. Secondary GoSPs (configured in
    // `secondaryGosps`) get a hard floor so they sink below real stops in autocomplete.
    let raw_importance = if is_secondary {
        SECONDARY_GOSP_IMPORTANCE
    } else if country.name == config.group_of_stop_places.home_country {
        importance_calc.calculate_importance_unclamped(gos_pop)
            * config.group_of_stop_places.importance_multiplier
    } else {
        importance_calc.calculate_importance(gos_pop)
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
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    indexed_cats.push(as_category(&gosp.id));

    // Secondary GoSPs use rank_address 0 so Photon maps them to AddressType.OTHER instead of
    // HOUSE. In the autocomplete short-query path (`SearchQueryBuilder.setupShortQuery`), every
    // doc whose `OBJECT_TYPE != "other"` earns a +0.4 function-score weight; secondary GoSPs
    // forfeit that boost. Combined with the importance cap, this is enough to push the bare
    // "Bergen" GoSP below real Bergen stops without modifying any user-visible field.
    let rank_address = if is_secondary { 0 } else { config.group_of_stop_places.rank_address };

    let mut member_names: Vec<String> = gosp.members.as_ref()
        .map(|m| m.refs.iter()
            .filter_map(|r| stop_by_id.get(r.ref_.as_str()).copied())
            .filter_map(|sp| sp.name.clone())
            .filter(|n| n != group_name)
            .collect())
        .unwrap_or_default();
    dedup_preserve_order(&mut member_names);

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
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(gosp.id.clone()),
                source: Some("nsr".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
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

/// GoSP popularity is the product of its members' popularities. Empty product is 1.0,
/// which lands on the importance floor for GoSPs whose members couldn't be resolved.
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
        && let Some(c) = Country::parse(Some(&cr.ref_))
    {
        return c;
    }
    geo::get_country(coord).unwrap_or_else(Country::no)
}

pub(crate) fn dedup_preserve_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

pub(crate) fn alt_stop_names(
    sp: &StopPlaceXml,
    primary_name: &str,
    name_type_filter: Option<&str>,
) -> Vec<String> {
    let Some(alt_names) = &sp.alternative_names else { return Vec::new() };
    alt_names.names.iter()
        .filter(|an| name_type_filter.is_none() || an.name_type.as_deref() == name_type_filter)
        .filter_map(|an| an.name.as_ref())
        .filter(|n| n.as_str() != primary_name && !n.is_empty())
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

    // ===== Category tests =====

    #[test]
    fn funicular_transport_mode_included_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("funicular"), Some("other"));
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &HashMap::new(),
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.other"));
    }

    #[test]
    fn bus_transport_mode_not_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("bus"), Some("onstreetBus"));
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &HashMap::new(),
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(!cats.iter().any(|c| c == "legacy.category.bus"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
    }

    #[test]
    fn parent_stop_includes_child_types_and_multimodal_category() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
        let sp = make_stop_place("NSR:StopPlace:Parent", "Hub", Some("funicular"), Some("other"));
        let mut child_types_map: HashMap<String, Vec<String>> = HashMap::new();
        child_types_map.insert("NSR:StopPlace:Parent".to_string(),
            vec!["onstreetBus".to_string(), "railStation".to_string(), "metroStation".to_string()]);
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &child_types_map,
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
        assert!(cats.iter().any(|c| c == "legacy.category.railStation"));
        assert!(cats.iter().any(|c| c == "legacy.category.metroStation"));
        assert!(cats.iter().any(|c| c == "multimodal.parent"));
    }

    // ===== Full conversion tests =====

    #[test]
    fn convert_stop_places_xml_produces_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_stopplace_convert_output.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NSR:GroupOfStopPlaces:1"));
        assert!(content.contains("NSR:GroupOfStopPlaces:72"));
        assert!(content.contains("\"name\":\"Oslo\""));
        assert!(content.contains(LAYER_GOSP));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn gosp_alt_name_contains_member_stop_names_not_id() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);

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
            &config, &importance_calc, &gosp, &HashMap::new(),
            &HashMap::new(), &stop_by_id, false,
        ).unwrap();

        let alt_name = result.content[0].name.as_ref().unwrap().alt_name.as_deref().unwrap_or("");
        assert!(alt_name.contains("Oslo S"), "alt_name should include member 'Oslo S': {alt_name}");
        assert!(alt_name.contains("Oslo Bussterminal"), "alt_name should include member 'Oslo Bussterminal': {alt_name}");
        assert!(!alt_name.contains("NSR:GroupOfStopPlaces:1"), "alt_name must not contain the GoSP id: {alt_name}");
    }

    #[test]
    fn convert_caps_secondary_gosp_importance() {
        // Configures GoSP:1 (Oslo) as secondary and asserts both demotion levers fire:
        // importance capped, rank_address set to 0. GoSP:72 (Hammerfest) is the control - same
        // fixture, no demotion config, must keep its full importance and configured rank_address.
        let mut config = test_config();
        config.group_of_stop_places.secondary_gosps =
            vec!["NSR:GroupOfStopPlaces:1".to_string()];
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_secondary_gosp_cap.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        assert_eq!(importance_of(&demoted), SECONDARY_GOSP_IMPORTANCE,
            "demoted GoSP must be pinned at the secondary-GoSP importance constant");
        assert!(importance_of(&kept) > SECONDARY_GOSP_IMPORTANCE,
            "non-demoted GoSP importance must exceed the secondary-GoSP floor");
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
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("fare_zone_authority.FIN.Authority.FIN_ID"));
        assert!(content.contains("fare_zone_authority.RUT.Authority.RUT_ID"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn tariff_and_fare_zone_ids_indexed_under_distinct_prefixes() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_zone_id_split.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        // :TariffZone: refs land under tariff_zone_id.
        assert!(content.contains("tariff_zone_id.RUT.TariffZone.1"));
        assert!(content.contains("tariff_zone_id.FIN.TariffZone.54540"));
        // :FareZone: refs land under fare_zone_id.
        assert!(content.contains("fare_zone_id.RUT.FareZone.4"));
        assert!(content.contains("fare_zone_id.FIN.FareZone.31"));
        // FareZone refs should NOT also be indexed under tariff_zone_id.
        assert!(!content.contains("tariff_zone_id.RUT.FareZone"));
        assert!(!content.contains("tariff_zone_id.FIN.FareZone"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn extra_splits_tariff_zones_and_fare_zones_by_ref_shape() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_extra_split.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        // The stop place's <TariffZones> mixes RUT:TariffZone:1 and RUT:FareZone:4; each should
        // surface in its own extra field, not the other.
        let rut_line = content
            .lines()
            .find(|l| l.contains("\"id\":\"NSR:StopPlace:") && l.contains("RUT:FareZone:4"))
            .expect("expected an NSR stop place line carrying the RUT FareZone ref");
        assert!(rut_line.contains("\"tariff_zones\":\"RUT:TariffZone:1\""), "got: {rut_line}");
        assert!(rut_line.contains("\"fare_zones\":\"RUT:FareZone:4\""), "got: {rut_line}");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_place_with_bus_submode_has_transport_mode_in_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_transport_mode.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("\"transport_mode\":\"bus:localBus\""));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_county_gid_and_locality_gid() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_gid.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        convert_all(&config, &input, &baseline_out, false, &UsageBoost::empty()).unwrap();
        let baseline = std::fs::read_to_string(&baseline_out).unwrap();
        let _ = std::fs::remove_file(&baseline_out);

        let csv = std::env::temp_dir().join("test_usage_boost_input.csv");
        std::fs::write(&csv, "id;name;usage\nNSR:StopPlace:56697;Oslo S;5000000\n").unwrap();
        let usage = UsageBoost::load(Some(&csv), &crate::config::UsageConfig::default()).unwrap();
        let boosted_out = std::env::temp_dir().join("test_usage_boosted.ndjson");
        convert_all(&config, &input, &boosted_out, false, &usage).unwrap();
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
