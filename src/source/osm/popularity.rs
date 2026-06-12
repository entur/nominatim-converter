use std::collections::{BTreeMap, HashMap};

use crate::config::Config;

struct POIFilter {
    key: String,
    value: String,
    priority: i32,
    use_entrance: bool,
}

pub struct OsmPopularityCalculator {
    filters: Vec<POIFilter>,
    default_value: f64,
}

impl OsmPopularityCalculator {
    pub fn new(config: &Config) -> Self {
        let osm = config.osm.as_ref().expect("osm config present when converting osm");
        let filters = osm
            .filters
            .iter()
            .map(|f| POIFilter {
                key: f.key.clone(),
                value: f.value.clone(),
                priority: f.priority,
                use_entrance: f.use_entrance,
            })
            .collect();
        Self {
            filters,
            default_value: osm.default_value,
        }
    }

    /// True if any filter opts into entrance handling. Used to gate the (otherwise free) extra
    /// entrance collection in pass 4 so it does no work unless configured.
    pub fn any_entrance_filter(&self) -> bool {
        self.filters.iter().any(|f| f.use_entrance)
    }

    /// True if any filter matching these tags has `useEntrance`.
    pub fn use_entrance(&self, tags: &HashMap<&str, &str>) -> bool {
        self.filters
            .iter()
            .any(|f| f.use_entrance && tags.get(f.key.as_str()) == Some(&f.value.as_str()))
    }

    /// Returns `default_value * highest_matching_priority`, or 0.0 if nothing matches.
    pub fn calculate_popularity(&self, tags: &BTreeMap<&str, &str>) -> f64 {
        let highest = self
            .filters
            .iter()
            .filter(|f| tags.get(f.key.as_str()) == Some(&f.value.as_str()))
            .map(|f| f.priority)
            .max();

        match highest {
            Some(p) => self.default_value * p as f64,
            None => 0.0,
        }
    }

    /// Returns true if this key/value pair is in the filter list.
    pub fn has_filter(&self, key: &str, value: &str) -> bool {
        self.filters
            .iter()
            .any(|f| f.key == key && f.value == value)
    }

    /// True if the entity is a POI we emit: it has a "name" tag and at least one tag matching a
    /// configured filter.
    pub(crate) fn is_poi(&self, tags: &HashMap<&str, &str>) -> bool {
        tags.contains_key("name") && tags.iter().any(|(k, v)| self.has_filter(k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::test_helpers::test_config_with_osm_filters;

    #[test]
    fn popularity_base_times_priority() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        let hospital = BTreeMap::from([("amenity", "hospital")]); // priority 9
        let cinema = BTreeMap::from([("amenity", "cinema")]); // priority 1
        let h_pop = calc.calculate_popularity(&hospital);
        let c_pop = calc.calculate_popularity(&cinema);
        assert!(h_pop > 0.0);
        assert!(c_pop > 0.0);
        assert_eq!(h_pop / c_pop, 9.0);
    }

    #[test]
    fn multiple_matching_tags_use_highest_priority() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        let high_only = BTreeMap::from([("amenity", "hospital")]); // 9
        let both = BTreeMap::from([("amenity", "hospital"), ("tourism", "attraction")]); // 9, 1
        assert_eq!(calc.calculate_popularity(&high_only), calc.calculate_popularity(&both));
    }

    #[test]
    fn unmatched_tags_return_zero() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("amenity", "bench")])), 0.0);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("shop", "convenience")])), 0.0);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("foo", "bar")])), 0.0);
    }

    #[test]
    fn empty_tags_return_zero() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        assert_eq!(calc.calculate_popularity(&BTreeMap::new()), 0.0);
    }

    #[test]
    fn has_filter_requires_exact_match() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        assert!(calc.has_filter("amenity", "hospital"));
        assert!(!calc.has_filter("amenity", "bench"));
        assert!(!calc.has_filter("amenity", "hospitals")); // plural
        assert!(!calc.has_filter("building", "hospital")); // wrong key
    }

    #[test]
    fn different_poi_types_have_different_priorities() {
        let config = test_config_with_osm_filters();
        let calc = OsmPopularityCalculator::new(&config);
        let hospital = calc.calculate_popularity(&BTreeMap::from([("amenity", "hospital")]));
        let hotel = calc.calculate_popularity(&BTreeMap::from([("tourism", "hotel")]));
        let cinema = calc.calculate_popularity(&BTreeMap::from([("amenity", "cinema")]));
        assert!(hospital > 0.0 && hotel > 0.0 && cinema > 0.0);
        assert_ne!(hospital, hotel);
        assert_ne!(hotel, cinema);
        assert_ne!(hospital, cinema);
    }
}
