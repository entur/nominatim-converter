use serde::Deserialize;
use std::path::{Path, PathBuf};

/// A source section is present in the config only when that source should be imported, and a
/// present section must carry an `input` (a missing `input` is a parse error). To skip a
/// source, omit its section entirely. `groupOfStopPlaces` and `usage` are tuning, not sources,
/// so they default when omitted.
#[derive(Deserialize, Clone, Default)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    pub osm: Option<OsmConfig>,
    pub stedsnavn: Option<StedsnavnConfig>,
    pub matrikkel: Option<MatrikkelConfig>,
    pub poi: Option<PoiConfig>,
    pub stop_place: Option<StopPlaceConfig>,
    pub belagenhet: Option<BelagenhetConfig>,
    /// Usage-driven popularity boost. Like a source section: present only to enable the
    /// boost, and when present it must carry an `input` (where the CSV lives). Omit to skip.
    pub usage: Option<UsageConfig>,
}

/// Where a source's data comes from, for the config-driven `build` command.
///
/// Externally tagged: `{ "url": "..." }`, `{ "file": "..." }`, `{ "region": "all" }`,
/// `{ "municipality": "all" }`. `Region` is only valid on matrikkel/stedsnavn (Geonorge
/// download); `Municipality` only on belagenhet (Lantmäteriet). `build` validates this and
/// errors clearly rather than encoding it in the type. Required on every present source
/// section - omit the section to skip the source.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceInput {
    /// Remote URL (http/https); downloaded and cached, ZIPs auto-extracted.
    Url(String),
    /// Local file path; ZIPs auto-extracted.
    File(PathBuf),
    /// Geonorge region: county code ("03"), name ("Oslo"), or "all" (matrikkel, stedsnavn).
    Region(String),
    /// Lantmäteriet municipality spec: "all", a 2-digit county ("03"), or a code ("0180").
    Municipality(String),
}

// Each source config carries an optional `minLines` sanity threshold (see the `min_lines`
// field on each *Config struct below): if a conversion writes fewer entries, it aborts.
// Unset means no check; the `--min-lines` CLI flag overrides it per run.

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageConfig {
    /// Where the usage CSV lives. Required when the `usage` section is present.
    pub input: SourceInput,
    #[serde(default = "default_usage_alpha")]
    pub alpha: f64,
    #[serde(default = "default_usage_floor")]
    pub usage_floor: u64,
}

fn default_usage_alpha() -> f64 { crate::common::usage::DEFAULT_ALPHA }
fn default_usage_floor() -> u64 { crate::common::usage::DEFAULT_USAGE_FLOOR }

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OsmConfig {
    pub input: SourceInput,
    pub default_value: f64,
    pub rank_address: RankAddress,
    pub filters: Vec<PoiFilter>,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct RankAddress {
    pub boundary: i32,
    pub place: i32,
    pub road: i32,
    pub building: i32,
    pub poi: i32,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PoiFilter {
    pub key: String,
    pub value: String,
    pub priority: i32,
    /// When true, substitute an associated entrance/gate coordinate for the polygon centroid of
    /// matching large-area features (those at least `MIN_AREA_SIZE_METERS` across). Off by default.
    #[serde(default)]
    pub use_entrance: bool,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StedsnavnConfig {
    pub input: SourceInput,
    pub default_value: f64,
    /// Per-place-type popularity keyed by SSR `navneobjekttype` (e.g. "by", "tettsted",
    /// "bydel"). Lets a city outrank a hamlet instead of every place name collapsing to the
    /// same importance. Types absent from the map fall back to `default_value`. The value is a
    /// popularity fed to the shared log10 normalization in `common::importance`, not an
    /// importance directly.
    #[serde(default)]
    pub type_popularity: std::collections::HashMap<String, f64>,
    pub rank_address: i32,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MatrikkelConfig {
    pub input: SourceInput,
    pub address_popularity: f64,
    pub street_popularity: f64,
    pub rank_address: i32,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PoiConfig {
    pub input: SourceInput,
    pub importance: f64,
    pub rank_address: i32,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StopPlaceConfig {
    pub input: SourceInput,
    pub default_value: i64,
    pub rank_address: i32,
    /// Multiplier applied to the importance of stops/GoSPs that resolve to a country other than
    /// Norway, so foreign hubs (e.g. the Berlin stop group) don't outrank Norwegian places on a
    /// bare prefix like "ber". `1.0` = no penalty. Applied after normalization and the GoSP cap,
    /// then clamped to the valid importance range. Importance is only one additive term in
    /// Photon's final rank, so this mainly breaks near-ties; ~0.6-0.7 puts a capped foreign hub
    /// (0.92) below a Norwegian city (~0.72). Validate changes with an autocomplete acceptance test.
    #[serde(default = "default_foreign_importance_factor")]
    pub foreign_importance_factor: f64,
    pub stop_type_factors: std::collections::HashMap<String, f64>,
    pub interchange_factors: std::collections::HashMap<String, f64>,
    /// Group-of-stop-places tuning. GoSPs are parsed from the same StopPlace NeTEx input and
    /// only this converter consumes them, so their config lives here. Defaults when omitted.
    #[serde(default)]
    pub group_of_stop_places: GroupOfStopPlacesConfig,
    /// Omit to build without fare zones; the run then warns and every zone filter is empty.
    #[serde(default)]
    pub fare_zones: Option<FareZonesConfig>,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

/// The fare zone NeTEx export (`https://api.entur.io/distance/netex/fare-zones`), the sole
/// source of fare zones. Optional, but leaving it out yields a full-size index whose zone
/// filters all return nothing, so any real build should set it.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FareZonesConfig {
    pub input: SourceInput,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GroupOfStopPlacesConfig {
    #[serde(default = "default_gosp_rank")]
    pub rank_address: i32,
    /// Explicit list of GoSP IDs to demote in autocomplete. Each listed GoSP gets its
    /// importance capped to `SECONDARY_GOSP_IMPORTANCE` and its `rank_address` set to 0,
    /// which forfeits the +0.4 weight Photon's `setupShortQuery` gives non-"other" docs.
    /// Use this to silence redundant aggregators like NSR:GroupOfStopPlaces:7 "Bergen", which
    /// coexists with the more useful NSR:GroupOfStopPlaces:174 "Bergen sentrum". Add new IDs
    /// here when they're identified in production - automatic detection was tried (member
    /// count, then name=locality match) and rejected, because the only known false-positive
    /// class (a canonical city GoSP that happens to have a sibling) is hard to distinguish
    /// from a real redundant aggregator.
    #[serde(default)]
    pub secondary_gosps: Vec<String>,
}

fn default_gosp_rank() -> i32 { 30 }

fn default_foreign_importance_factor() -> f64 { 1.0 }

impl Default for GroupOfStopPlacesConfig {
    fn default() -> Self {
        Self { rank_address: default_gosp_rank(), secondary_gosps: Vec::new() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BelagenhetConfig {
    pub input: SourceInput,
    #[serde(default = "default_belagenhet_address_pop")]
    pub address_popularity: f64,
    #[serde(default = "default_belagenhet_street_pop")]
    pub street_popularity: f64,
    #[serde(default = "default_belagenhet_rank")]
    pub rank_address: i32,
    #[serde(default)]
    pub min_lines: Option<usize>,
}

fn default_belagenhet_address_pop() -> f64 { 20.0 }
fn default_belagenhet_street_pop() -> f64 { 20.0 }
fn default_belagenhet_rank() -> i32 { 26 }

impl Config {
    /// Load and parse the converter configuration file (defaults to `converter.json`).
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.unwrap_or_else(|| Path::new("converter.json"));
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read config file '{}': {e}", path.display()))?;
        let config: Config = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid config '{}': {e}", path.display()))?;
        eprintln!("Loaded configuration from: {}", path.display());
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A full config: every source section present, each with an `input` (now required).
    const TEST_CONFIG: &str = r#"{
        "osm": {
            "input": { "file": "x.osm.pbf" },
            "defaultValue": 1.0,
            "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
            "filters": [
                {"key": "amenity", "value": "hospital", "priority": 9}
            ]
        },
        "stedsnavn": { "input": { "region": "03" }, "defaultValue": 40.0, "rankAddress": 16 },
        "matrikkel": { "input": { "region": "03" }, "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
        "poi": { "input": { "url": "https://example.com/poi.xml" }, "importance": 0.5, "rankAddress": 30 },
        "stopPlace": {
            "input": { "url": "https://example.com/stops.zip" },
            "defaultValue": 50,
            "rankAddress": 30,
            "stopTypeFactors": { "busStation": 2.0 },
            "interchangeFactors": { "preferredInterchange": 10.0 },
            "groupOfStopPlaces": { "rankAddress": 30 },
            "fareZones": { "input": { "url": "https://api.entur.io/distance/netex/fare-zones" } }
        }
    }"#;

    #[test]
    fn test_config_deserializes() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        let osm = config.osm.as_ref().unwrap();
        assert_eq!(osm.default_value, 1.0);
        assert_eq!(osm.rank_address.boundary, 10);
        assert_eq!(osm.rank_address.poi, 30);
        assert_eq!(osm.filters.len(), 1);
        assert_eq!(osm.filters[0].key, "amenity");
        assert_eq!(osm.filters[0].priority, 9);
    }

    #[test]
    fn test_config_stop_place_factors() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        let sp = config.stop_place.as_ref().unwrap();
        assert_eq!(sp.default_value, 50);
        assert_eq!(*sp.stop_type_factors.get("busStation").unwrap(), 2.0);
        assert_eq!(*sp.interchange_factors.get("preferredInterchange").unwrap(), 10.0);
    }

    #[test]
    fn test_config_fare_zones_optional() {
        let sp: StopPlaceConfig = serde_json::from_str(
            r#"{ "input": { "file": "x.zip" }, "defaultValue": 50, "rankAddress": 30,
                 "stopTypeFactors": {}, "interchangeFactors": {} }"#,
        ).unwrap();
        assert!(sp.fare_zones.is_none());
    }

    #[test]
    fn test_config_matrikkel() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        let m = config.matrikkel.as_ref().unwrap();
        assert_eq!(m.address_popularity, 20.0);
        assert_eq!(m.street_popularity, 20.0);
        assert_eq!(m.rank_address, 26);
    }

    #[test]
    fn test_input_parses_per_section() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.osm.as_ref().unwrap().input, SourceInput::File("x.osm.pbf".into()));
        assert_eq!(config.matrikkel.as_ref().unwrap().input, SourceInput::Region("03".into()));
        assert_eq!(config.poi.as_ref().unwrap().input, SourceInput::Url("https://example.com/poi.xml".into()));
    }

    #[test]
    fn test_min_lines_absent_defaults_to_none() {
        // No source section in TEST_CONFIG sets "minLines", so every threshold is None (no check).
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.osm.as_ref().unwrap().min_lines, None);
        assert_eq!(config.matrikkel.as_ref().unwrap().min_lines, None);
        assert_eq!(config.stop_place.as_ref().unwrap().min_lines, None);
    }

    #[test]
    fn test_min_lines_parses_per_source() {
        // Add a minLines threshold inside the osm and stopPlace sections only.
        let json = TEST_CONFIG
            .replace(
                r#""rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },"#,
                r#""rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
                   "minLines": 30000,"#,
            )
            .replace(
                r#""interchangeFactors": { "preferredInterchange": 10.0 }"#,
                r#""interchangeFactors": { "preferredInterchange": 10.0 }, "minLines": 40000"#,
            );
        let config: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(config.osm.as_ref().unwrap().min_lines, Some(30000));
        assert_eq!(config.stop_place.as_ref().unwrap().min_lines, Some(40000));
        // Sources without a minLines key stay None.
        assert_eq!(config.matrikkel.as_ref().unwrap().min_lines, None);
        assert_eq!(config.stedsnavn.as_ref().unwrap().min_lines, None);
        assert_eq!(config.poi.as_ref().unwrap().min_lines, None);
    }

    #[test]
    fn test_config_load_missing_file() {
        let result = Config::load(Some(Path::new("/nonexistent/config.json")));
        assert!(result.is_err());
    }

    #[test]
    fn test_source_input_variants_deserialize() {
        let url: SourceInput = serde_json::from_str(r#"{"url":"https://example.com/x.zip"}"#).unwrap();
        assert_eq!(url, SourceInput::Url("https://example.com/x.zip".into()));

        let file: SourceInput = serde_json::from_str(r#"{"file":"data/x.gpkg"}"#).unwrap();
        assert_eq!(file, SourceInput::File("data/x.gpkg".into()));

        let region: SourceInput = serde_json::from_str(r#"{"region":"all"}"#).unwrap();
        assert_eq!(region, SourceInput::Region("all".into()));

        let muni: SourceInput = serde_json::from_str(r#"{"municipality":"0180"}"#).unwrap();
        assert_eq!(muni, SourceInput::Municipality("0180".into()));
    }

    #[test]
    fn test_omitted_section_is_none() {
        // An empty config is valid: every source is simply absent (nothing to import).
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.osm.is_none());
        assert!(config.matrikkel.is_none());
        assert!(config.belagenhet.is_none());
        assert!(config.usage.is_none());
        // TEST_CONFIG omits belagenhet entirely; groupOfStopPlaces rides along with stopPlace
        // and defaults when that sub-block is omitted.
        let full: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert!(full.belagenhet.is_none());
        assert_eq!(full.stop_place.as_ref().unwrap().group_of_stop_places.rank_address, 30);
    }

    #[test]
    fn test_present_section_without_input_fails() {
        // A source section in the config but missing its `input` is a hard error - the
        // signal that you declared a source but forgot to say where its data comes from.
        let json = r#"{ "poi": { "importance": 0.5, "rankAddress": 30 } }"#;
        let result: Result<Config, _> = serde_json::from_str(json);
        assert!(result.is_err(), "poi section without input should fail to parse");
    }
}
