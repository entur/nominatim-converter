/// Title-case a string: capitalize first letter of each word.
pub fn titleize(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    let lower: String = chars.map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
                    format!("{upper}{lower}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Round to 6 decimal places.
pub fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}
