// ---------------------------------------------------------------------------
// Category string constants for Nominatim NDJSON output.
//
// Categories are dot-separated strings stored in each place's `categories` array.
// They serve as facets for filtering/searching in the downstream Photon geocoder.
//
// String constants must stay in sync with geocoder/proxy/.../common/Category.kt.
//
// Naming convention:
//   source.*   - data source identifier (used by acceptance tests to filter by origin)
//   layer.*    - broad classification layer (used for result type filtering)
//   legacy.*   - compatibility categories matching the original converter's output
//   country.*  - ISO country code
//   *_gid.*    - geographic ID references (county, locality)
// ---------------------------------------------------------------------------

// Data source identifiers
pub const SOURCE_ADRESSE: &str = "source.kartverket.matrikkelenadresse";
pub const SOURCE_STEDSNAVN: &str = "source.kartverket.stedsnavn";
pub const SOURCE_NSR: &str = "source.nsr";
pub const SOURCE_OSM: &str = "source.openstreetmap";
pub const SOURCE_POI: &str = "source.custom.poi";
pub const SOURCE_BELAGENHET: &str = "source.lantmateriet.belagenhetsadress";

// Classification layers
pub const LAYER_ADDRESS: &str = "layer.address";
pub const LAYER_STREET: &str = "layer.street";
pub const LAYER_STOP_PLACE: &str = "layer.stopPlace";
pub const LAYER_GOSP: &str = "layer.groupOfStopPlaces";
// Name fragment for GroupOfStopPlaces, used to build its legacy.category.* tag.
pub const GOSP: &str = "GroupOfStopPlaces";
pub const LAYER_POI: &str = "layer.poi";
pub const LAYER_PLACE: &str = "layer.place";

// Category prefixes
pub const COUNTRY_PREFIX: &str = "country.";
pub const TARIFF_ZONE_ID_PREFIX: &str = "tariff_zone_id.";
pub const TARIFF_ZONE_AUTH_PREFIX: &str = "tariff_zone_authority.";
pub const FARE_ZONE_ID_PREFIX: &str = "fare_zone_id.";
pub const FARE_ZONE_AUTH_PREFIX: &str = "fare_zone_authority.";
pub const COUNTY_ID_PREFIX: &str = "county_gid.";
pub const LOCALITY_ID_PREFIX: &str = "locality_gid.";
pub const STOP_PLACE_TYPE_PREFIX: &str = "stop_place_type.";
pub const LEGACY_CATEGORY_PREFIX: &str = "legacy.category.";

// Legacy compatibility tags carried over from the original converter
pub const LEGACY_SOURCE_WHOSONFIRST: &str = "legacy.source.whosonfirst";
pub const LEGACY_SOURCE_OPENADDRESSES: &str = "legacy.source.openaddresses";
pub const LEGACY_SOURCE_OPENSTREETMAP: &str = "legacy.source.openstreetmap";
pub const LEGACY_SOURCE_GEONAMES: &str = "legacy.source.geonames";
pub const LEGACY_LAYER_ADDRESS: &str = "legacy.layer.address";
pub const LEGACY_LAYER_VENUE: &str = "legacy.layer.venue";

/// Transliteration table loaded from `transliteration.csv` - the single source
/// for char -> replacement mappings, duplicated byte-identically in the geocoder
/// repo (proxy/src/main/resources/transliteration.csv). See the CSV header for
/// the sync and reindex constraints.
static TRANSLITERATIONS: std::sync::OnceLock<std::collections::HashMap<char, String>> =
    std::sync::OnceLock::new();

fn transliterations() -> &'static std::collections::HashMap<char, String> {
    TRANSLITERATIONS.get_or_init(|| {
        include_str!("transliteration.csv")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (from, to) = l
                    .split_once(';')
                    .unwrap_or_else(|| panic!("transliteration.csv: bad line: {l}"));
                let mut chars = from.chars();
                let c = chars.next().unwrap_or_else(|| panic!("transliteration.csv: empty char: {l}"));
                assert!(chars.next().is_none(), "transliteration.csv: left side must be one char: {l}");
                (c, to.to_string())
            })
            .collect()
    })
}

/// Convert a colon-separated ID to a Photon-safe category string.
///
/// Photon's `CATEGORY_PATTERN` (`[a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)+`) drops any
/// category containing characters outside that set at both index time
/// (`PhotonDoc.categories`) and query time (`RequestFactoryBase`). To make
/// street IDs with Norwegian or other European-diacritic names queryable,
/// colons become dots (namespace separators), characters in the allowed set
/// pass through, table characters are transliterated (å -> aa, ø -> oe, etc.),
/// and anything else becomes `_`.
///
/// The geocoder proxy applies the same transform (`Category.kt::asCategory`)
/// from its copy of the same table when querying - the two must produce
/// byte-identical output.
pub fn as_category(s: &str) -> String {
    let map = transliterations();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ':' => out.push('.'),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => out.push(c),
            _ => match map.get(&c) {
                Some(replacement) => out.push_str(replacement),
                None => out.push('_'),
            },
        }
    }
    out
}

pub fn tariff_zone_id_category(ref_: &str) -> String {
    format!("{TARIFF_ZONE_ID_PREFIX}{}", as_category(ref_))
}

pub fn fare_zone_id_category(ref_: &str) -> String {
    format!("{FARE_ZONE_ID_PREFIX}{}", as_category(ref_))
}

pub fn fare_zone_authority_category(ref_: &str) -> String {
    format!("{FARE_ZONE_AUTH_PREFIX}{}", as_category(ref_))
}

pub fn county_ids_category(ref_: &str) -> String {
    format!("{COUNTY_ID_PREFIX}{}", as_category(ref_))
}

pub fn locality_ids_category(ref_: &str) -> String {
    format!("{LOCALITY_ID_PREFIX}{}", as_category(ref_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_category_replaces_colons() {
        assert_eq!(as_category("NSR:StopPlace:123"), "NSR.StopPlace.123");
    }

    #[test]
    fn test_as_category_no_colons() {
        assert_eq!(as_category("something"), "something");
    }

    #[test]
    fn test_as_category_norwegian_diacritics() {
        assert_eq!(
            as_category("KVE:TopographicPlace:3907-Årfuglveien"),
            "KVE.TopographicPlace.3907-Aarfuglveien"
        );
        assert_eq!(as_category("Bjølsen"), "Bjoelsen");
        assert_eq!(as_category("Lærdal"), "Laerdal");
        assert_eq!(as_category("Tromsø"), "Tromsoe");
        assert_eq!(as_category("Ålesund"), "Aalesund");
    }

    #[test]
    fn test_as_category_street_with_spaces() {
        assert_eq!(
            as_category("KVE:TopographicPlace:0301-Karl Johans gate"),
            "KVE.TopographicPlace.0301-Karl_Johans_gate"
        );
    }

    #[test]
    fn test_as_category_fallback_and_unicode() {
        // Chars not in the table become a single `_` - including astral-plane
        // chars, which must match the Kotlin side's code-point iteration.
        assert_eq!(as_category("São Tomé"), "S_o_Tome");
        assert_eq!(as_category("Kárášjohka"), "Kara_johka");
        assert_eq!(as_category("emoji 🚀 char"), "emoji___char");
    }

    #[test]
    fn test_as_category_passes_photon_pattern() {
        // PhotonDoc.CATEGORY_PATTERN: [a-zA-Z0-9_-]+(\.[a-zA-Z0-9_-]+)+
        fn matches_photon_pattern(s: &str) -> bool {
            let segments: Vec<&str> = s.split('.').collect();
            if segments.len() < 2 { return false; }
            segments.iter().all(|seg| {
                !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
        }
        for input in [
            "KVE:TopographicPlace:0301-Karl Johans gate",
            "KVE:TopographicPlace:3407-Fahlstrøms plass",
            "KVE:PlaceName:434810",
            "KVE:Borough:34200205",
            "NSR:StopPlace:337",
            "OSM:TopographicPlace:545260792",
        ] {
            let out = as_category(input);
            assert!(matches_photon_pattern(&out), "as_category({input}) = {out} does not match Photon CATEGORY_PATTERN");
        }
    }

    #[test]
    fn test_tariff_zone_id_category() {
        assert_eq!(
            tariff_zone_id_category("RUT:TariffZone:1"),
            "tariff_zone_id.RUT.TariffZone.1"
        );
    }

    #[test]
    fn test_fare_zone_id_category() {
        assert_eq!(
            fare_zone_id_category("RUT:FareZone:4"),
            "fare_zone_id.RUT.FareZone.4"
        );
    }

    #[test]
    fn test_fare_zone_authority_category() {
        assert_eq!(
            fare_zone_authority_category("RUT:Authority:RUT"),
            "fare_zone_authority.RUT.Authority.RUT"
        );
    }

    #[test]
    fn test_county_ids_category() {
        assert_eq!(
            county_ids_category("KVE:TopographicPlace:03"),
            "county_gid.KVE.TopographicPlace.03"
        );
    }

    #[test]
    fn test_locality_ids_category() {
        assert_eq!(
            locality_ids_category("KVE:TopographicPlace:0301"),
            "locality_gid.KVE.TopographicPlace.0301"
        );
    }
}
