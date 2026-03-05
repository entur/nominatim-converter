use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Extra {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locality_gid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tariff_zones: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub county_gid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub borough: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub borough_gid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_place_type: Option<String>,
}
