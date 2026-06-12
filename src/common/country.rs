use iso3166_static::{Alpha2, Alpha3};

#[derive(Debug, Clone, PartialEq)]
pub struct Country {
    pub alpha2: String, // 2-letter lowercase ISO 3166-1 code (e.g. "no")
    pub alpha3: String, // 3-letter uppercase ISO 3166-1 code (e.g. "NOR")
}

impl Country {
    pub fn no() -> Self {
        Self { alpha2: "no".to_string(), alpha3: "NOR".to_string() }
    }

    pub fn se() -> Self {
        Self { alpha2: "se".to_string(), alpha3: "SWE".to_string() }
    }

    pub fn parse(code: &str) -> Option<Self> {
        let upper = code.to_uppercase();
        let alpha2 = Alpha2::try_from(upper.as_str()).ok()?;
        let alpha3 = Alpha3::try_from(alpha2).ok()?;
        Some(Country {
            alpha2: code.to_lowercase(),
            alpha3: alpha3.as_str().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_country_no() {
        let c = Country::no();
        assert_eq!(c.alpha2, "no");
        assert_eq!(c.alpha3, "NOR");
    }

    #[test]
    fn test_parse_norway() {
        let c = Country::parse("no").unwrap();
        assert_eq!(c.alpha2, "no");
        assert_eq!(c.alpha3, "NOR");
    }

    #[test]
    fn test_parse_uppercase() {
        let c = Country::parse("NO").unwrap();
        assert_eq!(c.alpha2, "no");
        assert_eq!(c.alpha3, "NOR");
    }

    #[test]
    fn test_parse_sweden() {
        let c = Country::parse("se").unwrap();
        assert_eq!(c.alpha2, "se");
        assert_eq!(c.alpha3, "SWE");
    }

    #[test]
    fn test_parse_invalid() {
        assert!(Country::parse("xx").is_none());
    }

    #[test]
    fn test_parse_empty() {
        assert!(Country::parse("").is_none());
    }
}
