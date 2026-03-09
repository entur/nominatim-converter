use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Country {
    pub name: String,              // 2-letter lowercase (e.g. "no")
    pub three_letter_code: String, // 3-letter uppercase (e.g. "NOR")
}

/// Complete ISO 3166-1 alpha-2 to alpha-3 mapping (matching Java's Locale.getISOCountries()).
static COUNTRIES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("ad", "AND");
    m.insert("ae", "ARE");
    m.insert("af", "AFG");
    m.insert("ag", "ATG");
    m.insert("ai", "AIA");
    m.insert("al", "ALB");
    m.insert("am", "ARM");
    m.insert("ao", "AGO");
    m.insert("aq", "ATA");
    m.insert("ar", "ARG");
    m.insert("as", "ASM");
    m.insert("at", "AUT");
    m.insert("au", "AUS");
    m.insert("aw", "ABW");
    m.insert("ax", "ALA");
    m.insert("az", "AZE");
    m.insert("ba", "BIH");
    m.insert("bb", "BRB");
    m.insert("bd", "BGD");
    m.insert("be", "BEL");
    m.insert("bf", "BFA");
    m.insert("bg", "BGR");
    m.insert("bh", "BHR");
    m.insert("bi", "BDI");
    m.insert("bj", "BEN");
    m.insert("bl", "BLM");
    m.insert("bm", "BMU");
    m.insert("bn", "BRN");
    m.insert("bo", "BOL");
    m.insert("bq", "BES");
    m.insert("br", "BRA");
    m.insert("bs", "BHS");
    m.insert("bt", "BTN");
    m.insert("bv", "BVT");
    m.insert("bw", "BWA");
    m.insert("by", "BLR");
    m.insert("bz", "BLZ");
    m.insert("ca", "CAN");
    m.insert("cc", "CCK");
    m.insert("cd", "COD");
    m.insert("cf", "CAF");
    m.insert("cg", "COG");
    m.insert("ch", "CHE");
    m.insert("ci", "CIV");
    m.insert("ck", "COK");
    m.insert("cl", "CHL");
    m.insert("cm", "CMR");
    m.insert("cn", "CHN");
    m.insert("co", "COL");
    m.insert("cr", "CRI");
    m.insert("cu", "CUB");
    m.insert("cv", "CPV");
    m.insert("cw", "CUW");
    m.insert("cx", "CXR");
    m.insert("cy", "CYP");
    m.insert("cz", "CZE");
    m.insert("de", "DEU");
    m.insert("dj", "DJI");
    m.insert("dk", "DNK");
    m.insert("dm", "DMA");
    m.insert("do", "DOM");
    m.insert("dz", "DZA");
    m.insert("ec", "ECU");
    m.insert("ee", "EST");
    m.insert("eg", "EGY");
    m.insert("eh", "ESH");
    m.insert("er", "ERI");
    m.insert("es", "ESP");
    m.insert("et", "ETH");
    m.insert("fi", "FIN");
    m.insert("fj", "FJI");
    m.insert("fk", "FLK");
    m.insert("fm", "FSM");
    m.insert("fo", "FRO");
    m.insert("fr", "FRA");
    m.insert("ga", "GAB");
    m.insert("gb", "GBR");
    m.insert("gd", "GRD");
    m.insert("ge", "GEO");
    m.insert("gf", "GUF");
    m.insert("gg", "GGY");
    m.insert("gh", "GHA");
    m.insert("gi", "GIB");
    m.insert("gl", "GRL");
    m.insert("gm", "GMB");
    m.insert("gn", "GIN");
    m.insert("gp", "GLP");
    m.insert("gq", "GNQ");
    m.insert("gr", "GRC");
    m.insert("gs", "SGS");
    m.insert("gt", "GTM");
    m.insert("gu", "GUM");
    m.insert("gw", "GNB");
    m.insert("gy", "GUY");
    m.insert("hk", "HKG");
    m.insert("hm", "HMD");
    m.insert("hn", "HND");
    m.insert("hr", "HRV");
    m.insert("ht", "HTI");
    m.insert("hu", "HUN");
    m.insert("id", "IDN");
    m.insert("ie", "IRL");
    m.insert("il", "ISR");
    m.insert("im", "IMN");
    m.insert("in", "IND");
    m.insert("io", "IOT");
    m.insert("iq", "IRQ");
    m.insert("ir", "IRN");
    m.insert("is", "ISL");
    m.insert("it", "ITA");
    m.insert("je", "JEY");
    m.insert("jm", "JAM");
    m.insert("jo", "JOR");
    m.insert("jp", "JPN");
    m.insert("ke", "KEN");
    m.insert("kg", "KGZ");
    m.insert("kh", "KHM");
    m.insert("ki", "KIR");
    m.insert("km", "COM");
    m.insert("kn", "KNA");
    m.insert("kp", "PRK");
    m.insert("kr", "KOR");
    m.insert("kw", "KWT");
    m.insert("ky", "CYM");
    m.insert("kz", "KAZ");
    m.insert("la", "LAO");
    m.insert("lb", "LBN");
    m.insert("lc", "LCA");
    m.insert("li", "LIE");
    m.insert("lk", "LKA");
    m.insert("lr", "LBR");
    m.insert("ls", "LSO");
    m.insert("lt", "LTU");
    m.insert("lu", "LUX");
    m.insert("lv", "LVA");
    m.insert("ly", "LBY");
    m.insert("ma", "MAR");
    m.insert("mc", "MCO");
    m.insert("md", "MDA");
    m.insert("me", "MNE");
    m.insert("mf", "MAF");
    m.insert("mg", "MDG");
    m.insert("mh", "MHL");
    m.insert("mk", "MKD");
    m.insert("ml", "MLI");
    m.insert("mm", "MMR");
    m.insert("mn", "MNG");
    m.insert("mo", "MAC");
    m.insert("mp", "MNP");
    m.insert("mq", "MTQ");
    m.insert("mr", "MRT");
    m.insert("ms", "MSR");
    m.insert("mt", "MLT");
    m.insert("mu", "MUS");
    m.insert("mv", "MDV");
    m.insert("mw", "MWI");
    m.insert("mx", "MEX");
    m.insert("my", "MYS");
    m.insert("mz", "MOZ");
    m.insert("na", "NAM");
    m.insert("nc", "NCL");
    m.insert("ne", "NER");
    m.insert("nf", "NFK");
    m.insert("ng", "NGA");
    m.insert("ni", "NIC");
    m.insert("nl", "NLD");
    m.insert("no", "NOR");
    m.insert("np", "NPL");
    m.insert("nr", "NRU");
    m.insert("nu", "NIU");
    m.insert("nz", "NZL");
    m.insert("om", "OMN");
    m.insert("pa", "PAN");
    m.insert("pe", "PER");
    m.insert("pf", "PYF");
    m.insert("pg", "PNG");
    m.insert("ph", "PHL");
    m.insert("pk", "PAK");
    m.insert("pl", "POL");
    m.insert("pm", "SPM");
    m.insert("pn", "PCN");
    m.insert("pr", "PRI");
    m.insert("ps", "PSE");
    m.insert("pt", "PRT");
    m.insert("pw", "PLW");
    m.insert("py", "PRY");
    m.insert("qa", "QAT");
    m.insert("re", "REU");
    m.insert("ro", "ROU");
    m.insert("rs", "SRB");
    m.insert("ru", "RUS");
    m.insert("rw", "RWA");
    m.insert("sa", "SAU");
    m.insert("sb", "SLB");
    m.insert("sc", "SYC");
    m.insert("sd", "SDN");
    m.insert("se", "SWE");
    m.insert("sg", "SGP");
    m.insert("sh", "SHN");
    m.insert("si", "SVN");
    m.insert("sj", "SJM");
    m.insert("sk", "SVK");
    m.insert("sl", "SLE");
    m.insert("sm", "SMR");
    m.insert("sn", "SEN");
    m.insert("so", "SOM");
    m.insert("sr", "SUR");
    m.insert("ss", "SSD");
    m.insert("st", "STP");
    m.insert("sv", "SLV");
    m.insert("sx", "SXM");
    m.insert("sy", "SYR");
    m.insert("sz", "SWZ");
    m.insert("tc", "TCA");
    m.insert("td", "TCD");
    m.insert("tf", "ATF");
    m.insert("tg", "TGO");
    m.insert("th", "THA");
    m.insert("tj", "TJK");
    m.insert("tk", "TKL");
    m.insert("tl", "TLS");
    m.insert("tm", "TKM");
    m.insert("tn", "TUN");
    m.insert("to", "TON");
    m.insert("tr", "TUR");
    m.insert("tt", "TTO");
    m.insert("tv", "TUV");
    m.insert("tw", "TWN");
    m.insert("tz", "TZA");
    m.insert("ua", "UKR");
    m.insert("ug", "UGA");
    m.insert("um", "UMI");
    m.insert("us", "USA");
    m.insert("uy", "URY");
    m.insert("uz", "UZB");
    m.insert("va", "VAT");
    m.insert("vc", "VCT");
    m.insert("ve", "VEN");
    m.insert("vg", "VGB");
    m.insert("vi", "VIR");
    m.insert("vn", "VNM");
    m.insert("vu", "VUT");
    m.insert("wf", "WLF");
    m.insert("ws", "WSM");
    m.insert("ye", "YEM");
    m.insert("yt", "MYT");
    m.insert("za", "ZAF");
    m.insert("zm", "ZMB");
    m.insert("zw", "ZWE");
    m
});

impl Country {
    pub fn no() -> Self {
        Self { name: "no".to_string(), three_letter_code: "NOR".to_string() }
    }

    pub fn parse(code: Option<&str>) -> Option<Self> {
        let code = code?.to_lowercase();
        COUNTRIES.get(code.as_str()).map(|&three| Country {
            name: code,
            three_letter_code: three.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_country_no() {
        let c = Country::no();
        assert_eq!(c.name, "no");
        assert_eq!(c.three_letter_code, "NOR");
    }

    #[test]
    fn test_parse_norway() {
        let c = Country::parse(Some("no")).unwrap();
        assert_eq!(c.name, "no");
        assert_eq!(c.three_letter_code, "NOR");
    }

    #[test]
    fn test_parse_uppercase() {
        let c = Country::parse(Some("NO")).unwrap();
        assert_eq!(c.name, "no");
        assert_eq!(c.three_letter_code, "NOR");
    }

    #[test]
    fn test_parse_sweden() {
        let c = Country::parse(Some("se")).unwrap();
        assert_eq!(c.name, "se");
        assert_eq!(c.three_letter_code, "SWE");
    }

    #[test]
    fn test_parse_none() {
        assert!(Country::parse(None).is_none());
    }

    #[test]
    fn test_parse_invalid() {
        assert!(Country::parse(Some("xx")).is_none());
    }

    #[test]
    fn test_parse_empty() {
        assert!(Country::parse(Some("")).is_none());
    }

    #[test]
    fn test_all_countries_have_two_letter_name() {
        for (key, three) in COUNTRIES.iter() {
            assert_eq!(key.len(), 2, "key '{key}' should be 2 chars");
            assert_eq!(three.len(), 3, "three_letter_code '{three}' should be 3 chars");
            assert!(three.chars().all(|c| c.is_ascii_uppercase()), "three_letter_code '{three}' should be uppercase");
        }
    }
}
