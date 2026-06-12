/// Title-case a string: capitalize the first letter of each word, lowercase
/// the rest. Words are re-joined with single spaces, so runs of whitespace
/// collapse -- output depends on this (it matches the original converter),
/// don't "fix" it with a whitespace-preserving rewrite.
pub fn titleize(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for (i, word) in s.split_whitespace().enumerate() {
        if i > 0 {
            result.push(' ');
        }
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            result.extend(c.to_uppercase());
            for c in chars {
                result.extend(c.to_lowercase());
            }
        }
    }
    result
}

pub const OSM_TAG_SEPARATOR: &str = ";";

pub fn join_osm_values(values: &[String]) -> Option<String> {
    let parts: Vec<&str> = values.iter().filter(|s| !s.is_empty()).map(String::as_str).collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(OSM_TAG_SEPARATOR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_titleize_simple() {
        assert_eq!(titleize("hello world"), "Hello World");
    }

    #[test]
    fn test_titleize_uppercase_input() {
        assert_eq!(titleize("OSLO SENTRUM"), "Oslo Sentrum");
    }

    #[test]
    fn test_titleize_mixed_case() {
        assert_eq!(titleize("sKøYeN"), "Skøyen");
    }

    #[test]
    fn test_titleize_single_word() {
        assert_eq!(titleize("oslo"), "Oslo");
    }

    #[test]
    fn test_titleize_empty() {
        assert_eq!(titleize(""), "");
    }

    #[test]
    fn test_titleize_norwegian_chars() {
        assert_eq!(titleize("ØRSTA SENTRUM"), "Ørsta Sentrum");
        assert_eq!(titleize("ålesund"), "Ålesund");
    }

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
