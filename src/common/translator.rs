use std::collections::HashMap;
use std::sync::LazyLock;

static DICTIONARY: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("allé", "avenue"); m.insert("alléen", "avenue");
    m.insert("ankomsthall", "arrival hall"); m.insert("anløpes", "served");
    m.insert("arrangement", "event"); m.insert("austgåande", "eastbound");
    m.insert("av", "by"); m.insert("avgangshall", "departure hall");
    m.insert("avgangshallen", "departure hall"); m.insert("avgangssted", "departure point");
    m.insert("avkjøring", "exit"); m.insert("avkjøringsrampe", "exit ramp");
    m.insert("avstigning", "exit"); m.insert("avvik", "deviation");
    m.insert("avviksstopp", "deviation stop"); m.insert("bak", "behind");
    m.insert("bane", "rail"); m.insert("bare", "only");
    m.insert("beltebil", "tracked vehicle"); m.insert("bensinstasjon", "gas station");
    m.insert("bensinstasjonen", "gas station"); m.insert("benyttes", "used");
    m.insert("bestillingsrute", "on-demand route"); m.insert("bestillingsruter", "on-demand routes");
    m.insert("betjenes", "served"); m.insert("betjent", "served");
    m.insert("bru", "bridge"); m.insert("brua", "bridge");
    m.insert("bruk", "use"); m.insert("brukes", "used");
    m.insert("brygga", "pier"); m.insert("brygge", "pier"); m.insert("bryggen", "pier");
    m.insert("buss", "bus"); m.insert("bussterminal", "bus terminal"); m.insert("busstopp", "bus stop");
    m.insert("bygg", "building"); m.insert("bygning", "building");
    m.insert("drosje", "taxi"); m.insert("egen", "separate");
    m.insert("ekspressbuss", "express bus"); m.insert("eller", "or");
    m.insert("etasje", "floor"); m.insert("ferjekai", "ferry pier");
    m.insert("flybuss", "airport bus"); m.insert("flybussen", "airport bus");
    m.insert("for", "for"); m.insert("foran", "in front of");
    m.insert("fotballbane", "football field"); m.insert("fotballbanen", "football field");
    m.insert("fotballstadion", "football stadium"); m.insert("fra", "from");
    m.insert("gamle", "old"); m.insert("gammel", "old");
    m.insert("gata", "street"); m.insert("gate", "street");
    m.insert("gravplass", "cemetery"); m.insert("gravplassen", "cemetery");
    m.insert("hall", "hall"); m.insert("hallen", "hall");
    m.insert("holdeplass", "stop"); m.insert("hovedinngang", "main entrance");
    m.insert("hovedinngangen", "main entrance"); m.insert("hurtigbåt", "express boat");
    m.insert("høyre", "right"); m.insert("i", "in");
    m.insert("idrettsbane", "sports field"); m.insert("idrettsbanen", "sports field");
    m.insert("ikke", "not"); m.insert("ikkje", "not");
    m.insert("indre", "inner"); m.insert("info", "info"); m.insert("informasjon", "information");
    m.insert("inngang", "entrance"); m.insert("inngangen", "entrance");
    m.insert("kai", "quay"); m.insert("kaia", "quay");
    m.insert("kirke", "church"); m.insert("kirkegård", "cemetery"); m.insert("kirkegården", "cemetery");
    m.insert("kirken", "church"); m.insert("kjøpesenter", "shopping center");
    m.insert("kryss", "intersection"); m.insert("krysset", "intersection");
    m.insert("kun", "only"); m.insert("kyrkja", "church");
    m.insert("langs", "along"); m.insert("lenger", "longer"); m.insert("ligger", "located");
    m.insert("lokal", "local"); m.insert("med", "with"); m.insert("mellom", "between");
    m.insert("midl", "temporary"); m.insert("midlertidig", "temporary"); m.insert("midtre", "middle");
    m.insert("mot", "towards"); m.insert("motsiden", "opposite side");
    m.insert("nattbuss", "night bus"); m.insert("nedenfor", "below");
    m.insert("nedre", "lower"); m.insert("nedsiden", "lower side");
    m.insert("nord", "north"); m.insert("nordgående", "northbound"); m.insert("nordsiden", "north side");
    m.insert("ny", "new"); m.insert("nye", "new"); m.insert("når", "when"); m.insert("nøyaktig", "exact");
    m.insert("og", "and"); m.insert("omkjøring", "detour"); m.insert("oppdatert", "updated");
    m.insert("over", "across"); m.insert("parkeringa", "parking");
    m.insert("parkeringshus", "parking garage"); m.insert("parkeringshuset", "parking garage");
    m.insert("parkeringsplass", "parking lot"); m.insert("parkeringsplassen", "parking lot");
    m.insert("plan", "floor"); m.insert("plass", "square"); m.insert("plassen", "square");
    m.insert("på", "at"); m.insert("påkjøringsrampe", "entrance ramp"); m.insert("påstigning", "entry");
    m.insert("rampe", "ramp"); m.insert("rampen", "ramp");
    m.insert("reserve", "backup"); m.insert("reserveplass", "backup location");
    m.insert("retning", "direction"); m.insert("ring", "ring road");
    m.insert("ringvei", "ring road"); m.insert("ringveien", "ring road");
    m.insert("rundkjøring", "roundabout"); m.insert("rundkjøringen", "roundabout");
    m.insert("rutebuss", "scheduled bus"); m.insert("se", "see");
    m.insert("senter", "center"); m.insert("senteret", "center"); m.insert("sentrum", "center");
    m.insert("servicerute", "service route"); m.insert("shuttlebuss", "shuttle bus");
    m.insert("skole", "school"); m.insert("skolebuss", "school bus");
    m.insert("skoleelever", "pupils"); m.insert("skolen", "school");
    m.insert("skoleplass", "school yard"); m.insert("skoleplassen", "school yard");
    m.insert("skuleplassen", "school yard");
    m.insert("snuplass", "turning area"); m.insert("snuplassen", "turning area");
    m.insert("som", "as"); m.insert("spor", "track"); m.insert("sporet", "track");
    m.insert("stasjon", "station"); m.insert("stasjonen", "station");
    m.insert("stopp", "stop"); m.insert("stoppested", "stop"); m.insert("stormarked", "supermarket");
    m.insert("sydsiden", "south side"); m.insert("sør", "south");
    m.insert("sørgående", "southbound"); m.insert("sørsiden", "south side");
    m.insert("t-bane", "metro"); m.insert("terminal", "terminal"); m.insert("terminalen", "terminal");
    m.insert("til", "to"); m.insert("tog", "train"); m.insert("trikk", "tram");
    m.insert("tunnel", "tunnel"); m.insert("tunnelen", "tunnel"); m.insert("under", "under");
    m.insert("utgang", "exit"); m.insert("utsiden", "outside");
    m.insert("ved", "at"); m.insert("veg", "road"); m.insert("vegen", "road");
    m.insert("vei", "road"); m.insert("veien", "road");
    m.insert("vendt", "facing"); m.insert("venstre", "left"); m.insert("vest", "west");
    m.insert("vestgående", "westbound"); m.insert("vestsiden", "west side");
    m.insert("ytre", "outer"); m.insert("øst", "east");
    m.insert("østgående", "eastbound"); m.insert("østsiden", "east side"); m.insert("øvre", "upper");
    m
});

/// Translate Norwegian text to English word-by-word, preserving capitalization and punctuation.
pub fn translate(norwegian: &str) -> String {
    if norwegian.trim().is_empty() {
        return norwegian.to_string();
    }
    let mut result = String::with_capacity(norwegian.len());
    let mut word_start = 0;

    for (i, c) in norwegian.char_indices() {
        if c.is_whitespace() || c == ',' || c == '.' {
            if i > word_start {
                result.push_str(&translate_word(&norwegian[word_start..i]));
            }
            result.push(c);
            word_start = i + c.len_utf8();
        }
    }
    if word_start < norwegian.len() {
        result.push_str(&translate_word(&norwegian[word_start..]));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_simple_word() {
        assert_eq!(translate("stasjon"), "station");
    }

    #[test]
    fn test_translate_preserves_capitalization() {
        assert_eq!(translate("Stasjon"), "Station");
    }

    #[test]
    fn test_translate_all_caps() {
        assert_eq!(translate("STASJON"), "STATION");
    }

    #[test]
    fn test_translate_multi_word() {
        assert_eq!(translate("Oslo stasjon"), "Oslo station");
    }

    #[test]
    fn test_translate_unknown_word_preserved() {
        assert_eq!(translate("Skøyen"), "Skøyen");
    }

    #[test]
    fn test_translate_mixed_known_unknown() {
        assert_eq!(translate("Skøyen stasjon"), "Skøyen station");
    }

    #[test]
    fn test_translate_empty() {
        assert_eq!(translate(""), "");
        assert_eq!(translate("  "), "  ");
    }

    #[test]
    fn test_translate_with_punctuation() {
        assert_eq!(translate("stasjon, nord"), "station, north");
    }

    #[test]
    fn test_translate_direction_words() {
        assert_eq!(translate("nord"), "north");
        assert_eq!(translate("sør"), "south");
        assert_eq!(translate("øst"), "east");
        assert_eq!(translate("vest"), "west");
    }

    #[test]
    fn test_translate_transport_words() {
        assert_eq!(translate("buss"), "bus");
        assert_eq!(translate("tog"), "train");
        assert_eq!(translate("trikk"), "tram");
        assert_eq!(translate("t-bane"), "metro");
        assert_eq!(translate("hurtigbåt"), "express boat");
    }

    #[test]
    fn test_translate_common_place_words() {
        assert_eq!(translate("kirke"), "church");
        assert_eq!(translate("skole"), "school");
        assert_eq!(translate("senter"), "center");
        assert_eq!(translate("brygge"), "pier");
        assert_eq!(translate("kai"), "quay");
    }
}

fn translate_word(word: &str) -> String {
    let lower = word.to_lowercase();
    match DICTIONARY.get(lower.as_str()) {
        Some(translation) => {
            if word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()) {
                translation.to_uppercase()
            } else if word.starts_with(|c: char| c.is_uppercase()) {
                let mut chars = translation.chars();
                match chars.next() {
                    Some(first) => {
                        let upper: String = first.to_uppercase().collect();
                        format!("{upper}{}", chars.as_str())
                    }
                    None => String::new(),
                }
            } else {
                translation.to_string()
            }
        }
        None => word.to_string(),
    }
}
