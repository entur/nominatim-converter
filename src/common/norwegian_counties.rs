/// A Norwegian county as used in Geonorge download URLs.
/// `slug` is "{code}_{name}" (ASCII-folded) as it appears in URLs.
pub struct County {
    pub code: &'static str,
    pub name: &'static str,
    pub slug: &'static str,
}

/// Norwegian county codes and names as used in Geonorge download URLs.
pub const COUNTIES: &[County] = &[
    County { code: "03", name: "Oslo", slug: "03_Oslo" },
    County { code: "11", name: "Rogaland", slug: "11_Rogaland" },
    County { code: "15", name: "Møre og Romsdal", slug: "15_More-og-Romsdal" },
    County { code: "18", name: "Nordland", slug: "18_Nordland" },
    County { code: "21", name: "Svalbard", slug: "21_Svalbard" },
    County { code: "31", name: "Østfold", slug: "31_Ostfold" },
    County { code: "32", name: "Akershus", slug: "32_Akershus" },
    County { code: "33", name: "Buskerud", slug: "33_Buskerud" },
    County { code: "34", name: "Innlandet", slug: "34_Innlandet" },
    County { code: "38", name: "Vestfold", slug: "38_Vestfold" },
    County { code: "39", name: "Telemark", slug: "39_Telemark" },
    County { code: "40", name: "Agder", slug: "40_Agder" },
    County { code: "42", name: "Vestland", slug: "42_Vestland" },
    County { code: "50", name: "Trøndelag", slug: "50_Trondelag" },
    County { code: "55", name: "Troms", slug: "55_Troms" },
    County { code: "56", name: "Finnmark", slug: "56_Finnmark" },
];

/// Resolve a region argument to a Geonorge URL slug.
/// Accepts: county code ("03"), county name ("Oslo"), or "0000"/"all" for all of Norway.
pub fn resolve_geonorge_region(arg: &str) -> Result<String, String> {
    let lower = arg.to_lowercase();

    if lower == "all" || arg == "0000" {
        return Ok("0000_Norge".to_string());
    }

    // Try exact code match
    if let Some(county) = COUNTIES.iter().find(|c| c.code == arg) {
        return Ok(county.slug.to_string());
    }

    // Try case-insensitive name match
    if let Some(county) = COUNTIES.iter().find(|c| c.name.to_lowercase() == lower) {
        return Ok(county.slug.to_string());
    }

    Err(format!("Unknown region '{arg}'. Use a county code (e.g. 03), name (e.g. Oslo), or 'all' for all of Norway."))
}

pub fn list_regions() {
    // A listing the user reads or pipes, so it goes to stdout.
    println!("Available regions for Geonorge download:");
    println!("  all / 0000  All of Norway (large download)");
    for county in COUNTIES {
        println!("  {:<10}{}", county.code, county.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_all() {
        assert_eq!(resolve_geonorge_region("all").unwrap(), "0000_Norge");
        assert_eq!(resolve_geonorge_region("0000").unwrap(), "0000_Norge");
    }

    #[test]
    fn resolve_by_code() {
        assert_eq!(resolve_geonorge_region("03").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("50").unwrap(), "50_Trondelag");
    }

    #[test]
    fn resolve_by_name() {
        assert_eq!(resolve_geonorge_region("Oslo").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("oslo").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("Trøndelag").unwrap(), "50_Trondelag");
    }

    #[test]
    fn resolve_unknown() {
        assert!(resolve_geonorge_region("99").is_err());
        assert!(resolve_geonorge_region("Narnia").is_err());
    }

    #[test]
    fn has_all_counties() {
        assert_eq!(COUNTIES.len(), 16);
    }
}
