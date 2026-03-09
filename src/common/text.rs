pub const OSM_TAG_SEPARATOR: &str = ";";

pub fn join_osm_values(values: &[String]) -> Option<String> {
    let mut iter = values.iter().filter(|s| !s.is_empty()).peekable();
    iter.peek()?;
    let mut result = String::new();
    for (i, s) in iter.enumerate() {
        if i > 0 {
            result.push_str(OSM_TAG_SEPARATOR);
        }
        result.push_str(s);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_osm_values() {
        let vals = vec!["bus".to_string(), "tram".to_string()];
        assert_eq!(join_osm_values(&vals), Some("bus;tram".to_string()));
    }

    #[test]
    fn test_join_osm_values_filters_empty() {
        let vals = vec!["bus".to_string(), "".to_string(), "tram".to_string()];
        assert_eq!(join_osm_values(&vals), Some("bus;tram".to_string()));
    }

    #[test]
    fn test_join_osm_values_all_empty() {
        let vals = vec!["".to_string(), "".to_string()];
        assert_eq!(join_osm_values(&vals), None);
    }

    #[test]
    fn test_join_osm_values_empty_vec() {
        let vals: Vec<String> = vec![];
        assert_eq!(join_osm_values(&vals), None);
    }

    #[test]
    fn test_join_osm_values_single() {
        let vals = vec!["bus".to_string()];
        assert_eq!(join_osm_values(&vals), Some("bus".to_string()));
    }
}
