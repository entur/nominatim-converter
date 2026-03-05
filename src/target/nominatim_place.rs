use crate::common::extra::Extra;
use serde::{Serialize, Serializer};

mod f64_6dp {
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(val: &f64, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let formatted = format!("{:.6}", val);
        let raw: Box<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string(formatted).map_err(S::Error::custom)?;
        (*raw).serialize(s)
    }
}

mod vec_f64_6dp {
    use serde::ser::SerializeSeq;
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(vals: &[f64], s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut seq = s.serialize_seq(Some(vals.len()))?;
        for val in vals {
            let formatted = format!("{:.6}", val);
            let raw: Box<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(formatted).map_err(S::Error::custom)?;
            seq.serialize_element(&*raw)?;
        }
        seq.end()
    }
}

#[derive(Debug, Serialize)]
pub struct NominatimPlace {
    #[serde(rename = "type")]
    pub type_: String,
    pub content: Vec<PlaceContent>,
}

#[derive(Debug, Serialize)]
pub struct PlaceContent {
    pub place_id: i64,
    pub object_type: String,
    pub object_id: i64,
    pub categories: Vec<String>,
    pub rank_address: i32,
    #[serde(serialize_with = "f64_6dp::serialize")]
    pub importance: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_place_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,
    pub address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub housenumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postcode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(serialize_with = "vec_f64_6dp::serialize")]
    pub centroid: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "vec_f64_6dp::serialize")]
    pub bbox: Vec<f64>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Address {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub county: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Name {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "name:en", skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NominatimHeader {
    #[serde(rename = "type")]
    pub type_: String,
    pub content: HeaderContent,
}

#[derive(Debug, Serialize)]
pub struct HeaderContent {
    pub version: String,
    pub generator: String,
    pub database_version: String,
    pub data_timestamp: String,
    pub features: Features,
}

#[derive(Debug, Serialize)]
pub struct Features {
    pub sorted_by_country: bool,
    pub has_addresslines: bool,
}
