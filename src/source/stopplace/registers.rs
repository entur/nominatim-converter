//! Stops one register has inside a country another register owns, e.g. SSR's copies of Norwegian
//! stations. Such a stop is dropped only when an owner stop lies within `MATCH_RADIUS_M`, so a
//! stop the owner lacks survives. So does one with a keep mode no owner stop that near has, e.g.
//! NSR's airports in Sweden, where SSR has only the bus stop outside.

use super::convert::register;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::geo;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

const MATCH_RADIUS_M: f64 = 200.0;

pub struct StopPoint {
    pub id: String,
    pub coord: Coordinate,
    /// Its own transport mode and its children's.
    pub modes: Vec<String>,
}

/// Country (alpha-2) to the register that owns its stops, by id codespace: `{"no": "nsr"}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CountryRegisters(BTreeMap<String, String>);

impl CountryRegisters {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for CountryRegisters {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut out = BTreeMap::new();
        for (key, owner) in BTreeMap::<String, String>::deserialize(deserializer)? {
            let country = Country::parse(&key).ok_or_else(|| D::Error::custom(format!("unknown country code `{key}`")))?;
            if owner.trim().is_empty() {
                return Err(D::Error::custom(format!("empty register for `{key}`")));
            }
            out.insert(country.alpha2, owner.to_ascii_lowercase());
        }
        Ok(Self(out))
    }
}

/// Ids to leave out of the output, given every stop place the inputs will emit.
pub fn skip_set(
    stops: &[StopPoint],
    registers: &CountryRegisters,
    keep_modes: &[String],
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    if let Some(m) = keep_modes.iter().find(|m| !stops.iter().any(|s| s.modes.contains(m))) {
        return Err(format!("countryRegistersKeepModes: no stop places in the inputs with mode `{m}`").into());
    }
    let mut skip = HashSet::new();
    for (country, owner) in &registers.0 {
        let owned: Vec<&StopPoint> = stops.iter().filter(|s| register(&s.id) == *owner).collect();
        if owned.is_empty() {
            return Err(format!("countryRegisters: no {owner} stop places in the inputs to own {country}").into());
        }
        let (mut dropped, mut kept, mut kept_for_mode) = (Vec::new(), Vec::new(), Vec::new());
        for s in stops.iter().filter(|s| register(&s.id) != *owner) {
            if geo::get_country(&s.coord).is_none_or(|c| c.alpha2 != *country) {
                continue;
            }
            let near: Vec<&StopPoint> =
                owned.iter().copied().filter(|o| geo::distance_meters(&o.coord, &s.coord) <= MATCH_RADIUS_M).collect();
            if near.is_empty() {
                kept.push(s.id.as_str());
            } else if s.modes.iter().any(|m| keep_modes.contains(m) && !near.iter().any(|o| o.modes.contains(m))) {
                kept_for_mode.push(s.id.as_str());
            } else {
                dropped.push(s.id.as_str());
            }
        }
        dropped.sort_unstable();
        kept.sort_unstable();
        kept_for_mode.sort_unstable();
        if dropped.is_empty() {
            eprintln!("countryRegisters: no stop places in {country} with a {owner} stop within {MATCH_RADIUS_M} m");
        } else {
            eprintln!(
                "countryRegisters: leaving out {} stop places in {country} with a {owner} stop within {MATCH_RADIUS_M} m: {}",
                dropped.len(),
                dropped.join(", ")
            );
        }
        if !kept.is_empty() {
            eprintln!("countryRegisters: kept {} stop places in {country} with no {owner} stop nearby: {}", kept.len(), kept.join(", "));
        }
        if !kept_for_mode.is_empty() {
            eprintln!(
                "countryRegisters: kept {} stop places in {country} with a keep mode no nearby {owner} stop has: {}",
                kept_for_mode.len(),
                kept_for_mode.join(", ")
            );
        }
        skip.extend(dropped.into_iter().map(str::to_string));
    }
    Ok(skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(id: &str, lat: f64, lon: f64) -> StopPoint {
        stop_with_modes(id, lat, lon, &["bus"])
    }

    fn stop_with_modes(id: &str, lat: f64, lon: f64, modes: &[&str]) -> StopPoint {
        StopPoint { id: id.to_string(), coord: Coordinate::new(lat, lon), modes: modes.iter().map(|m| m.to_string()).collect() }
    }

    fn norway_nsr() -> CountryRegisters {
        serde_json::from_str(r#"{"NO": "NSR"}"#).unwrap()
    }

    #[test]
    fn drops_copies_near_an_owner_stop_and_keeps_the_rest() {
        let stops = [
            stop("NSR:StopPlace:59872", 59.9104, 10.7531),
            stop("SSR:StopPlace:1", 59.9110, 10.7535), // Oslo S, ~70 m away
            stop("SSR:StopPlace:2", 63.33701, 12.05964), // Storlien gräns, no NSR stop nearby
            stop("SSR:StopPlace:3", 57.709, 11.973), // Göteborg, outside Norway
            stop("NSR:StopPlace:4", 58.936, 11.172), // Strömstad, NSR in Sweden
            stop("SSR:StopPlace:5", 59.91175, 10.7531), // ~150 m away
            stop("SSR:StopPlace:6", 59.91265, 10.7531), // ~250 m away
        ];
        let expected = HashSet::from(["SSR:StopPlace:1".to_string(), "SSR:StopPlace:5".to_string()]);
        assert_eq!(skip_set(&stops, &norway_nsr(), &[]).unwrap(), expected);
    }

    #[test]
    fn keeps_a_stop_with_a_keep_mode_no_nearby_owner_stop_has() {
        let sweden_ssr: CountryRegisters = serde_json::from_str(r#"{"se": "ssr"}"#).unwrap();
        let stops = [
            stop_with_modes("SSR:StopPlace:1623", 57.667548, 12.295569, &["bus"]), // Landvetter flygplats
            stop_with_modes("NSR:StopPlace:60331", 57.668021, 12.295525, &["air", "bus"]), // Göteborg flygplats
            stop_with_modes("NSR:StopPlace:60169", 57.667974, 12.295839, &["bus"]),
            stop_with_modes("SSR:StopPlace:9", 58.3161, 12.3511, &["air"]), // Trollhättan
            stop_with_modes("NSR:StopPlace:9", 58.3165, 12.3511, &["air"]),
            stop_with_modes("SSR:StopPlace:13722", 68.4205, 18.6842, &["bus"]), // Björkliden
            stop_with_modes("NSR:StopPlace:58612", 68.4209, 18.6829, &["rail"]),
        ];
        let air = ["air".to_string()];
        let expected = HashSet::from(["NSR:StopPlace:60169", "NSR:StopPlace:9", "NSR:StopPlace:58612"].map(String::from));
        assert_eq!(skip_set(&stops, &sweden_ssr, &air).unwrap(), expected);
        assert!(skip_set(&stops, &sweden_ssr, &[]).unwrap().contains("NSR:StopPlace:60331"));
    }

    #[test]
    fn unused_keep_mode_is_an_error() {
        let stops = [stop("NSR:StopPlace:1", 59.911, 10.753)];
        let err = skip_set(&stops, &norway_nsr(), &["airport".to_string()]).unwrap_err().to_string();
        assert!(err.contains("mode `airport`"), "{err}");
    }

    #[test]
    fn missing_owner_register_is_an_error() {
        let stops = [stop("SSR:StopPlace:1", 59.911, 10.753)];
        let typo: CountryRegisters = serde_json::from_str(r#"{"no": "nrs"}"#).unwrap();
        let err = skip_set(&stops, &typo, &[]).unwrap_err().to_string();
        assert!(err.contains("no nrs stop places"), "{err}");
    }

    #[test]
    fn rejects_unknown_country_and_empty_register() {
        for bad in [r#"{"nor": "nsr"}"#, r#"{"no": " "}"#] {
            assert!(serde_json::from_str::<CountryRegisters>(bad).is_err(), "{bad} should be rejected");
        }
    }
}
