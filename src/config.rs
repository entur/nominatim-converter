use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub osm: OsmConfig,
    pub stedsnavn: StedsnavnConfig,
    pub matrikkel: MatrikkelConfig,
    pub poi: PoiConfig,
    #[serde(rename = "stopPlace")]
    pub stop_place: StopPlaceConfig,
    #[serde(rename = "groupOfStopPlaces")]
    pub group_of_stop_places: GroupOfStopPlacesConfig,
    pub importance: ImportanceConfig,
}

#[derive(Deserialize, Clone)]
pub struct OsmConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: RankAddress,
    pub filters: Vec<PoiFilter>,
}

#[derive(Deserialize, Clone)]
pub struct RankAddress {
    pub boundary: i32,
    pub place: i32,
    pub road: i32,
    pub building: i32,
    pub poi: i32,
}

#[derive(Deserialize, Clone)]
pub struct PoiFilter {
    pub key: String,
    pub value: String,
    pub priority: i32,
}

#[derive(Deserialize, Clone)]
pub struct StedsnavnConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct MatrikkelConfig {
    #[serde(rename = "addressPopularity")]
    pub address_popularity: f64,
    #[serde(rename = "streetPopularity")]
    pub street_popularity: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct PoiConfig {
    pub importance: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct StopPlaceConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: i64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
    #[serde(rename = "stopTypeFactors")]
    pub stop_type_factors: std::collections::HashMap<String, f64>,
    #[serde(rename = "interchangeFactors")]
    pub interchange_factors: std::collections::HashMap<String, f64>,
}

#[derive(Deserialize, Clone)]
pub struct GroupOfStopPlacesConfig {
    #[serde(rename = "gosBoostFactor")]
    pub gos_boost_factor: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct ImportanceConfig {
    #[serde(rename = "minPopularity")]
    pub min_popularity: f64,
    #[serde(rename = "maxPopularity")]
    pub max_popularity: f64,
    pub floor: f64,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.unwrap_or_else(|| Path::new("converter.json"));
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read config file '{}': {e}", path.display()))?;
        let config: Config = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid config '{}': {e}", path.display()))?;
        println!("Loaded configuration from: {}", path.display());
        Ok(config)
    }
}
