use super::xml::StopPlaceXml;

pub(crate) fn calculate_stop_popularity(
    config: &crate::config::StopPlaceConfig,
    sp: &StopPlaceXml,
    children: &[&StopPlaceXml],
    usage_boost: f64,
) -> i64 {
    let mut pop = config.default_value;
    let sum: f64 = children
        .iter()
        .filter_map(|c| c.stop_place_type.as_deref())
        .chain(sp.stop_place_type.as_deref())
        .map(|t| config.stop_type_factors.get(t).copied().unwrap_or(1.0))
        .sum();
    if sum > 0.0 {
        pop = (pop as f64 * sum) as i64;
    }
    // A parent sums its children's stop place types, so it must take their interchange weighting
    // too. Oslo bussterminal's parent is only `interchangeAllowed` while the busStation child under
    // it is `preferredInterchange`, which left the parent - the record the proxy returns under
    // multimodal=parent - below its own child.
    let interchange = children
        .iter()
        .filter_map(|c| c.weighting.as_deref())
        .chain(sp.weighting.as_deref())
        .filter_map(|w| config.interchange_factors.get(w))
        .copied()
        .reduce(f64::max)
        .unwrap_or(1.0);
    pop = (pop as f64 * interchange) as i64;
    (pop as f64 * usage_boost) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::helpers::{make_stop_place, test_config};

    fn child(id: &str, stop_place_type: Option<&str>, weighting: Option<&str>) -> StopPlaceXml {
        let mut sp = make_stop_place(id, "Child", None, stop_place_type);
        sp.weighting = weighting.map(str::to_string);
        sp
    }

    #[test]
    fn basic_stop_returns_default_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        assert_eq!(pop, config.stop_place.as_ref().unwrap().default_value);
    }

    #[test]
    fn bus_station_has_higher_popularity_than_basic() {
        let config = test_config();
        let basic = make_stop_place("NSR:StopPlace:1", "Test", None, Some("onstreetBus"));
        let bus_station = make_stop_place("NSR:StopPlace:2", "Test", None, Some("busStation"));
        let basic_pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &basic, &[], 1.0);
        let bus_pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &bus_station, &[], 1.0);
        assert!(bus_pop > basic_pop);
    }

    #[test]
    fn metro_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("metroStation"));
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn rail_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn recommended_interchange_multiplies_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("recommendedInterchange".to_string());
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        // 50 * 2 (rail) * 3 (interchange) = 300
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0 * 3.0) as i64);
    }

    #[test]
    fn preferred_interchange_gives_high_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("preferredInterchange".to_string());
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        // 50 * 2 * 10 = 1000
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0 * 10.0) as i64);
    }

    #[test]
    fn interchange_factor_below_one_demotes() {
        // A seeded max would floor the multiplier at 1.0 and silently ignore a demoting factor.
        let mut config = test_config();
        config.stop_place.as_mut().unwrap()
            .interchange_factors.insert("noInterchange".to_string(), 0.5);
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("noInterchange".to_string());
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0);
        // 50 * 2 * 0.5 = 50
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0 * 0.5) as i64);
    }

    #[test]
    fn popularity_values_strictly_ordered() {
        let config = test_config();
        let pops: Vec<i64> = vec![
            calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &make_stop_place("1", "T", None, None), &[], 1.0),
            calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &make_stop_place("2", "T", None, Some("busStation")), &[], 1.0),
            {
                let mut sp = make_stop_place("3", "T", None, Some("railStation"));
                sp.weighting = Some("recommendedInterchange".to_string());
                calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0)
            },
            {
                let mut sp = make_stop_place("4", "T", None, Some("railStation"));
                sp.weighting = Some("preferredInterchange".to_string());
                calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[], 1.0)
            },
        ];
        for i in 0..pops.len() - 1 {
            assert!(pops[i] < pops[i + 1], "Expected {} < {}", pops[i], pops[i + 1]);
        }
    }

    // ===== Multimodal parent tests =====

    #[test]
    fn multimodal_parent_uses_sum_of_child_types() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let rail = child("NSR:StopPlace:2", Some("railStation"), None);
        let metro = child("NSR:StopPlace:3", Some("metroStation"), None);
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[&rail, &metro], 1.0);
        // 50 * (2 + 2) = 200
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 4.0) as i64);
    }

    #[test]
    fn multimodal_parent_sums_factors_not_multiplies() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let rail = child("NSR:StopPlace:2", Some("railStation"), None);
        let metro = child("NSR:StopPlace:3", Some("metroStation"), None);
        let bus = child("NSR:StopPlace:4", Some("busStation"), None);
        let pop = calculate_stop_popularity(
            config.stop_place.as_ref().unwrap(), &sp, &[&rail, &metro, &bus], 1.0);
        // 50 * (2+2+2) = 300, NOT 50 * 2*2*2 = 400
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 6.0) as i64);
    }

    #[test]
    fn multimodal_parent_unconfigured_child_defaults_to_factor_1() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let ferry = child("NSR:StopPlace:2", Some("ferryStop"), None);
        let tram = child("NSR:StopPlace:3", Some("tramStation"), None);
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[&ferry, &tram], 1.0);
        // 50 * (1+1) = 100
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn multimodal_parent_inherits_strongest_child_interchange() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        sp.weighting = Some("interchangeAllowed".to_string());
        let bus = child("NSR:StopPlace:2", Some("busStation"), Some("preferredInterchange"));
        let quay = child("NSR:StopPlace:3", None, Some("recommendedInterchange"));
        let pop = calculate_stop_popularity(
            config.stop_place.as_ref().unwrap(), &sp, &[&bus, &quay], 1.0);
        // 50 * 2 * 10 (strongest of the children, not the parent's own weighting)
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0 * 10.0) as i64);
    }

    #[test]
    fn multimodal_parent_keeps_own_interchange_when_stronger_than_childrens() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        sp.weighting = Some("preferredInterchange".to_string());
        let bus = child("NSR:StopPlace:2", Some("busStation"), Some("recommendedInterchange"));
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[&bus], 1.0);
        // 50 * 2 * 10, not the child's 3
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 2.0 * 10.0) as i64);
    }

    #[test]
    fn multimodal_parent_with_interchange() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        sp.weighting = Some("preferredInterchange".to_string());
        let rail = child("NSR:StopPlace:2", Some("railStation"), None);
        let metro = child("NSR:StopPlace:3", Some("metroStation"), None);
        let pop = calculate_stop_popularity(
            config.stop_place.as_ref().unwrap(), &sp, &[&rail, &metro], 1.0);
        // 50 * (2+2) * 10 = 2000
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 4.0 * 10.0) as i64);
    }

    #[test]
    fn duplicate_child_types_are_summed() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let a = child("NSR:StopPlace:2", Some("railStation"), None);
        let b = child("NSR:StopPlace:3", Some("railStation"), None);
        let c = child("NSR:StopPlace:4", Some("railStation"), None);
        let pop = calculate_stop_popularity(config.stop_place.as_ref().unwrap(), &sp, &[&a, &b, &c], 1.0);
        // 50 * (2+2+2) = 300
        assert_eq!(pop, (config.stop_place.as_ref().unwrap().default_value as f64 * 6.0) as i64);
    }

    // GoSP popularity is exercised end-to-end by the integration tests in convert.rs.
}
