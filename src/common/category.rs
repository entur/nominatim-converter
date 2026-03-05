pub const OSM_ADDRESS: &str = "osm.public_transport.address";
pub const OSM_STREET: &str = "osm.public_transport.street";
pub const OSM_STOP_PLACE: &str = "osm.public_transport.stop_place";
pub const OSM_POI: &str = "osm.public_transport.poi";
pub const OSM_CUSTOM_POI: &str = "osm.public_transport.custom_poi";
pub const OSM_GOSP: &str = "osm.public_transport.group_of_stop_places";

pub const SOURCE_ADRESSE: &str = "source.kartverket.matrikkelenadresse";
pub const SOURCE_STEDSNAVN: &str = "source.kartverket.stedsnavn";
pub const SOURCE_NSR: &str = "source.nsr";

pub const GOSP: &str = "GroupOfStopPlaces";

pub const COUNTRY_PREFIX: &str = "country.";
pub const TARIFF_ZONE_ID_PREFIX: &str = "tariff_zone_id.";
pub const TARIFF_ZONE_AUTH_PREFIX: &str = "tariff_zone_authority.";
pub const FARE_ZONE_PREFIX: &str = "fare_zone_authority.";
pub const COUNTY_ID_PREFIX: &str = "county_gid.";
pub const LOCALITY_ID_PREFIX: &str = "locality_gid.";
pub const LEGACY_CATEGORY_PREFIX: &str = "legacy.category.";

pub fn as_category(s: &str) -> String {
    s.replace(':', ".")
}

pub fn tariff_zone_id_category(ref_: &str) -> String {
    format!("{TARIFF_ZONE_ID_PREFIX}{}", as_category(ref_))
}

pub fn fare_zone_authority_category(ref_: &str) -> String {
    format!("{FARE_ZONE_PREFIX}{}", as_category(ref_))
}

pub fn county_ids_category(ref_: &str) -> String {
    format!("{COUNTY_ID_PREFIX}{}", as_category(ref_))
}

pub fn locality_ids_category(ref_: &str) -> String {
    format!("{LOCALITY_ID_PREFIX}{}", as_category(ref_))
}
