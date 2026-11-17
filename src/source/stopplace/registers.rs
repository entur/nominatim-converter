//! Stops one register has inside a country another register owns, e.g. SSR's copies of Norwegian
//! stations. Such a stop is dropped when an owner stop is near, unless it has a keep mode no near
//! owner stop has (NSR's airports in Sweden). A dropped `TRUSTED_REGISTER` stop hands its names and
//! ranking factors to its counterpart, the near owner stop that stands in for it.

use super::convert::register;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::geo;
use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

const MATCH_RADIUS_M: f64 = 200.0;
/// For an owner stop with the same name: NSR and SSR put Stockholm C 242 m apart.
const NAME_MATCH_RADIUS_M: f64 = 500.0;
/// Entur's register. Only its dropped stops hand anything on: SSR's names for Norwegian stops are
/// often stale ("Moss Lufthavn Rygge"), and its `recommendedInterchange` on Halden must not lift NSR's.
const TRUSTED_REGISTER: &str = "nsr";

pub struct StopPoint {
    pub id: String,
    pub coord: Coordinate,
    pub parent: Option<String>,
    /// Its own transport mode and its children's.
    pub modes: Vec<String>,
    /// Its name, then its alternative names: what it hands on.
    pub names: Vec<String>,
    /// Every name it is indexed with, its children's included, folded; its own name first.
    pub indexed: Vec<String>,
    pub factors: Factors,
    pub popularity: i64,
}

impl StopPoint {
    fn has_name(&self, folded: &str) -> bool {
        self.indexed.iter().any(|n| n == folded)
    }
}

/// The interchange and usage factors in a stop's popularity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Factors {
    pub interchange: f64,
    pub usage: f64,
}

impl Factors {
    pub fn max(self, other: Self) -> Self {
        Self { interchange: self.interchange.max(other.interchange), usage: self.usage.max(other.usage) }
    }
}

/// Stop places to leave out, and what they hand to the stops that stand in for them.
#[derive(Debug, Default)]
pub struct SkipSet {
    pub ids: HashSet<String>,
    /// Stops dropped for an owner stop in another register; their groups stay.
    pub replaced: HashSet<String>,
    /// Counterpart id -> names of its dropped stops that no near owner stop has.
    pub carried_names: HashMap<String, Vec<String>>,
    /// Counterpart id -> the highest factors of its dropped stops.
    pub carried_factors: HashMap<String, Factors>,
}

impl SkipSet {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Drops each key too, handing what was carried to it on to its value.
    pub fn drop_for_preferred(&mut self, preferred: BTreeMap<String, String>) {
        for (dropped, kept) in preferred {
            if let Some(names) = self.carried_names.remove(&dropped) {
                self.carried_names.entry(kept.clone()).or_default().extend(names);
            }
            if let Some(f) = self.carried_factors.remove(&dropped) {
                self.carried_factors.entry(kept).and_modify(|k| *k = k.max(f)).or_insert(f);
            }
            self.ids.insert(dropped);
        }
    }
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

/// Stops to leave out of the output, given every stop place the inputs will emit.
pub fn skip_set(
    stops: &[StopPoint],
    registers: &CountryRegisters,
    keep_modes: &[String],
) -> Result<SkipSet, Box<dyn std::error::Error>> {
    if let Some(m) = keep_modes.iter().find(|m| !stops.iter().any(|s| s.modes.contains(m))) {
        return Err(format!("countryRegistersKeepModes: no stop places in the inputs with mode `{m}`").into());
    }
    let mut skip = SkipSet::default();
    let mut dropped_with_near = Vec::new();
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
            let near = near_owner_stops(s, &owned);
            if near.is_empty() {
                kept.push(s.id.clone());
            } else if let Some(m) = s.modes.iter().find(|m| keep_modes.contains(m) && !near.iter().any(|o| o.modes.contains(m))) {
                kept_for_mode.push(format!("{} ({m})", s.id));
            } else {
                dropped.push(s.id.clone());
                dropped_with_near.push((s, near));
            }
        }
        let list = |what: String, mut ids: Vec<String>| {
            if !ids.is_empty() {
                ids.sort_unstable();
                eprintln!("countryRegisters: {what}: {}", ids.join(", "));
            }
        };
        if dropped.is_empty() {
            eprintln!("countryRegisters: no stop places in {country} with a {owner} stop nearby");
        }
        list(format!("leaving out {} stop places in {country} with a {owner} stop nearby", dropped.len()), dropped.clone());
        list(format!("kept {} stop places in {country} with no {owner} stop nearby", kept.len()), kept);
        list(format!("kept {} stop places in {country} for a keep mode no nearby {owner} stop has", kept_for_mode.len()), kept_for_mode);
        skip.ids.extend(dropped);
    }
    // Once every drop is known: a child whose parent stays hands nothing on, its parent indexes it.
    let mut handed = 0;
    for (s, near) in dropped_with_near {
        skip.replaced.insert(s.id.clone());
        let parent_stays = s.parent.as_ref().is_some_and(|p| !skip.ids.contains(p));
        if register(&s.id) == TRUSTED_REGISTER && !parent_stays {
            hand_over(s, &near, &mut skip);
            handed += 1;
        }
    }
    if handed > 0 {
        eprintln!("countryRegisters: {handed} dropped {TRUSTED_REGISTER} stop places hand names and ranking factors to their counterparts");
    }
    Ok(skip)
}

/// Owner stops within `MATCH_RADIUS_M`, or within `NAME_MATCH_RADIUS_M` with `s`'s name.
fn near_owner_stops<'a>(s: &StopPoint, owned: &[&'a StopPoint]) -> Vec<&'a StopPoint> {
    owned
        .iter()
        .copied()
        .filter(|o| {
            let d = geo::distance_meters(&o.coord, &s.coord);
            d <= MATCH_RADIUS_M || (d <= NAME_MATCH_RADIUS_M && o.has_name(&s.indexed[0]))
        })
        .collect()
}

/// Hands `dropped`'s names that no near stop has to its counterpart: the near stop with its name,
/// else one sharing a mode, else any; nearest first, then the most popular, as SSR often has a
/// bus and a rail stop at one point. A counterpart with its name or a shared mode also takes its
/// ranking factors where higher.
fn hand_over(dropped: &StopPoint, near: &[&StopPoint], skip: &mut SkipSet) {
    let main = &dropped.indexed[0];
    let shares_mode = |o: &StopPoint| o.modes.iter().any(|m| dropped.modes.contains(m));
    let rank = |o: &StopPoint| (!o.has_name(main), !shares_mode(o), geo::distance_meters(&o.coord, &dropped.coord), Reverse(o.popularity));
    let Some(&target) = near.iter().min_by(|a, b| rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal)) else {
        return;
    };
    if target.has_name(main) || shares_mode(target) {
        skip.carried_factors.entry(target.id.clone()).and_modify(|f| *f = f.max(dropped.factors)).or_insert(dropped.factors);
    }
    let mut known: Vec<String> = near
        .iter()
        .flat_map(|o| o.indexed.iter().cloned().chain(skip.carried_names.get(&o.id).into_iter().flatten().map(|n| fold(n))))
        .collect();
    for name in &dropped.names {
        let folded = fold(name);
        if !known.iter().any(|k| k == &folded || k.starts_with(&format!("{folded} "))) {
            skip.carried_names.entry(target.id.clone()).or_default().push(name.clone());
            known.push(folded);
        }
    }
}

/// Lowercase, Swedish ä/ö as æ/ø, punctuation as single spaces.
pub(crate) fn fold(name: &str) -> String {
    let mapped: String = name
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ä' => 'æ',
            'ö' => 'ø',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(id: &str, lat: f64, lon: f64) -> StopPoint {
        StopPoint {
            id: id.to_string(),
            coord: Coordinate::new(lat, lon),
            parent: None,
            modes: vec!["bus".to_string()],
            names: vec![id.to_string()],
            indexed: vec![fold(id)],
            factors: Factors { interchange: 1.0, usage: 1.0 },
            popularity: 0,
        }
    }

    impl StopPoint {
        fn modes(mut self, modes: &[&str]) -> Self {
            self.modes = modes.iter().map(|m| m.to_string()).collect();
            self
        }

        fn names(mut self, names: &[&str]) -> Self {
            self.names = names.iter().map(|n| n.to_string()).collect();
            self.indexed = names.iter().map(|n| fold(n)).collect();
            self
        }

        fn factors(mut self, interchange: f64, usage: f64) -> Self {
            self.factors = Factors { interchange, usage };
            self
        }

        fn popularity(mut self, popularity: i64) -> Self {
            self.popularity = popularity;
            self
        }

        fn parent(mut self, parent: &str) -> Self {
            self.parent = Some(parent.to_string());
            self
        }
    }

    fn sweden_ssr() -> CountryRegisters {
        serde_json::from_str(r#"{"se": "ssr"}"#).unwrap()
    }

    fn norway_nsr() -> CountryRegisters {
        serde_json::from_str(r#"{"NO": "NSR"}"#).unwrap()
    }

    fn carried(skip: &SkipSet, id: &str) -> Vec<String> {
        skip.carried_names.get(id).cloned().unwrap_or_default()
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
        assert_eq!(skip_set(&stops, &norway_nsr(), &[]).unwrap().ids, expected);
    }

    #[test]
    fn keeps_a_stop_with_a_keep_mode_no_nearby_owner_stop_has() {
        let stops = [
            stop("SSR:StopPlace:1623", 57.667548, 12.295569), // Landvetter flygplats
            stop("NSR:StopPlace:60331", 57.668021, 12.295525).modes(&["air", "bus"]), // Göteborg flygplats
            stop("NSR:StopPlace:60169", 57.667974, 12.295839),
            stop("SSR:StopPlace:9", 58.3161, 12.3511).modes(&["air"]), // Trollhättan
            stop("NSR:StopPlace:9", 58.3165, 12.3511).modes(&["air"]),
            stop("SSR:StopPlace:13722", 68.4205, 18.6842), // Björkliden
            stop("NSR:StopPlace:58612", 68.4209, 18.6829).modes(&["rail"]),
        ];
        let air = ["air".to_string()];
        let expected = HashSet::from(["NSR:StopPlace:60169", "NSR:StopPlace:9", "NSR:StopPlace:58612"].map(String::from));
        assert_eq!(skip_set(&stops, &sweden_ssr(), &air).unwrap().ids, expected);
        assert!(skip_set(&stops, &sweden_ssr(), &[]).unwrap().contains("NSR:StopPlace:60331"));
    }

    #[test]
    fn drops_a_stop_with_a_same_name_owner_stop_past_the_match_radius() {
        let stops = [
            stop("SSR:StopPlace:53410", 59.3830, 13.8530).names(&["Väse station"]).modes(&["rail"]),
            stop("NSR:StopPlace:579", 59.3851, 13.8530).names(&["Väse station"]).modes(&["rail"]), // 234 m
            stop("NSR:StopPlace:2", 59.3851, 13.8540).names(&["Väse kyrka"]), // 240 m, another name
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(skip.ids, HashSet::from(["NSR:StopPlace:579".to_string()]));
    }

    #[test]
    fn carries_new_names_to_the_nearest_owner_stop_sharing_a_mode() {
        let stops = [
            stop("SSR:StopPlace:1", 62.5641, 15.5664).names(&["Gällö busshållplats"]),
            stop("SSR:StopPlace:2", 62.5642, 15.5670).names(&["Gällö järnvägsstation"]).modes(&["rail"]),
            stop("NSR:StopPlace:1", 62.5641, 15.5665).names(&["Gällö station", "Gjellø", "Gällö"]).modes(&["rail"]),
            stop("NSR:StopPlace:2", 62.5641, 15.5666).names(&["Gallo station", "Gjellø"]).modes(&["rail"]),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(carried(&skip, "SSR:StopPlace:2"), ["Gällö station", "Gjellø", "Gallo station"]);
        assert_eq!(skip.carried_names.len(), 1);
    }

    #[test]
    fn carries_to_the_owner_stop_with_the_name() {
        let stops = [
            stop("SSR:StopPlace:18525", 55.7060, 13.1880).names(&["Lund Clemenstorget"]),
            stop("SSR:StopPlace:25845", 55.7055, 13.1872).names(&["Lund Centralstation"]),
            stop("NSR:StopPlace:58010", 55.7058, 13.1880).names(&["Lund Centralstation", "Lund station"]),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(carried(&skip, "SSR:StopPlace:25845"), ["Lund station"]);
        assert_eq!(skip.carried_names.len(), 1);
    }

    #[test]
    fn a_name_is_had_only_up_to_a_word() {
        let stops = [
            stop("SSR:StopPlace:206", 59.3311, 18.0598).names(&["Stockholm Centralstation"]),
            stop("NSR:StopPlace:1", 59.3312, 18.0598).names(&["Stockholm Centralstation", "Stockholm C", "Stockholm"]),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(carried(&skip, "SSR:StopPlace:206"), ["Stockholm C"]);
    }

    #[test]
    fn same_name_counterpart_past_the_match_radius_takes_names_and_factors() {
        let stops = [
            stop("SSR:StopPlace:22734", 59.3315, 18.0588).names(&["Stockholm City"]).modes(&["rail"]).factors(3.0, 1.0), // 150 m
            stop("SSR:StopPlace:206", 59.3311, 18.0598).names(&["Stockholm Centralstation"]).modes(&["rail"]), // 240 m
            stop("SSR:StopPlace:9", 59.3335, 18.0563).names(&["Stockholm Centralstationsgatan"]), // 310 m
            stop("NSR:StopPlace:58635", 59.3307, 18.0559)
                .names(&["Stockholm Centralstation", "Stockholm Central Station"])
                .modes(&["rail"])
                .factors(10.0, 1.5),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert!(skip.contains("NSR:StopPlace:58635"));
        assert_eq!(carried(&skip, "SSR:StopPlace:206"), ["Stockholm Central Station"]);
        assert_eq!(skip.carried_factors, HashMap::from([("SSR:StopPlace:206".to_string(), Factors { interchange: 10.0, usage: 1.5 })]));
    }

    #[test]
    fn of_two_stops_at_one_point_the_more_popular_is_the_counterpart() {
        let stops = [
            stop("SSR:StopPlace:975", 55.6090, 13.0000).names(&["Malmö Centralstation"]).popularity(1_000),
            stop("SSR:StopPlace:1053", 55.6090, 13.0000).names(&["Malmö Centralstation"]).modes(&["rail"]).popularity(25_000),
            stop("NSR:StopPlace:58393", 55.6091, 13.0000).names(&["Malmö Centralstation", "Malmø sentralstasjon"]).modes(&["bus", "rail"]),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(carried(&skip, "SSR:StopPlace:1053"), ["Malmø sentralstasjon"]);
    }

    #[test]
    fn a_counterpart_sharing_no_name_or_mode_takes_names_only() {
        let stops = [
            stop("SSR:StopPlace:1", 62.0, 15.0).names(&["Ånge bstn"]),
            stop("NSR:StopPlace:1", 62.0005, 15.0).names(&["Ånge station"]).modes(&["rail"]).factors(10.0, 2.0),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &[]).unwrap();
        assert_eq!(carried(&skip, "SSR:StopPlace:1"), ["Ånge station"]);
        assert!(skip.carried_factors.is_empty());
    }

    #[test]
    fn a_child_whose_parent_stays_hands_nothing_on() {
        let stops = [
            stop("SSR:StopPlace:1623", 57.667548, 12.295569).names(&["Landvetter flygplats"]),
            stop("NSR:StopPlace:60331", 57.668021, 12.295525).names(&["Göteborg flygplats"]).modes(&["air", "bus"]),
            stop("NSR:StopPlace:60169", 57.667974, 12.295839).names(&["Landvetter flygplats", "Gothenburg Airport"]).parent("NSR:StopPlace:60331"),
        ];
        let skip = skip_set(&stops, &sweden_ssr(), &["air".to_string()]).unwrap();
        assert!(skip.contains("NSR:StopPlace:60169"));
        assert!(skip.carried_names.is_empty());
    }

    #[test]
    fn only_nsr_stops_hand_anything_on() {
        let stops = [
            stop("NSR:StopPlace:198", 59.1189, 11.3890).names(&["Halden stasjon"]),
            stop("SSR:StopPlace:1", 59.1190, 11.3890).names(&["Halden station"]).factors(3.0, 1.0),
        ];
        let skip = skip_set(&stops, &norway_nsr(), &[]).unwrap();
        assert!(skip.replaced.contains("SSR:StopPlace:1"));
        assert!(skip.carried_names.is_empty() && skip.carried_factors.is_empty());
    }

    #[test]
    fn drop_for_preferred_hands_carried_names_and_factors_on() {
        let mut skip = SkipSet {
            carried_names: HashMap::from([("SSR:StopPlace:1".to_string(), vec!["Gjellø".to_string()])]),
            carried_factors: HashMap::from([
                ("SSR:StopPlace:1".to_string(), Factors { interchange: 10.0, usage: 1.0 }),
                ("NSR:StopPlace:2".to_string(), Factors { interchange: 3.0, usage: 1.5 }),
            ]),
            ..SkipSet::default()
        };
        skip.drop_for_preferred(BTreeMap::from([("SSR:StopPlace:1".to_string(), "NSR:StopPlace:2".to_string())]));
        assert!(skip.contains("SSR:StopPlace:1"));
        assert_eq!(carried(&skip, "NSR:StopPlace:2"), ["Gjellø"]);
        assert_eq!(skip.carried_factors, HashMap::from([("NSR:StopPlace:2".to_string(), Factors { interchange: 10.0, usage: 1.5 })]));
    }

    #[test]
    fn fold_spots_names_across_spellings() {
        assert_eq!(fold("Rödsveien"), fold("Rødsveien"));
        assert_eq!(fold("Grums, Nyängen"), fold("Grums Nyängen"));
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
