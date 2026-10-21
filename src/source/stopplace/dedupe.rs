//! Stops registered in two registers, from a CSV of `id_a,id_b,preferred_id[,...]` rows after a
//! header. Drops the id that isn't preferred, unless the preferred one is missing from the inputs.

use std::collections::HashSet;
use std::path::Path;

/// Ids to leave out of the output, given the stop place ids the inputs will emit.
pub fn skip_set(csv: &Path, present: &HashSet<String>) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(csv)
        .map_err(|e| format!("Cannot read dedupe CSV '{}': {e}", csv.display()))?;
    let (skip, kept) = parse(&content, present)?;
    eprintln!("Dedupe: leaving out {} stop places", skip.len());
    if !kept.is_empty() {
        eprintln!("warning: dedupe kept {} stop places whose preferred duplicate is missing: {}", kept.len(), kept.join(", "));
    }
    Ok(skip)
}

/// Returns the ids to skip, and the ids kept because their preferred duplicate is missing.
fn parse(content: &str, present: &HashSet<String>) -> Result<(HashSet<String>, Vec<String>), Box<dyn std::error::Error>> {
    let mut preferred_ids = HashSet::new();
    let mut dropped_ids = HashSet::new();
    let mut skip = HashSet::new();
    let mut kept = Vec::new();
    for (lineno, line) in content.lines().enumerate().skip(1) {
        let line_err = |msg: String| format!("dedupe CSV line {}: {msg}: {line}", lineno + 1);
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.splitn(4, ',').map(|f| f.trim().trim_matches('"')).collect();
        let [a, b, preferred, ..] = fields[..] else {
            return Err(line_err("expected at least 3 fields".into()).into());
        };
        if a == b {
            return Err(line_err(format!("{a} is paired with itself")).into());
        }
        let other = match preferred {
            p if p == a => b,
            p if p == b => a,
            _ => return Err(line_err(format!("preferred id {preferred} is neither {a} nor {b}")).into()),
        };
        preferred_ids.insert(preferred);
        dropped_ids.insert(other);
        match (present.contains(preferred), present.contains(other)) {
            (true, true) => { skip.insert(other.to_string()); }
            (false, true) => kept.push(other.to_string()),
            _ => {}
        }
    }
    // Otherwise a chain (A over B, B over C) or contradicting rows drop every stop involved.
    if let Some(id) = preferred_ids.intersection(&dropped_ids).next() {
        return Err(format!("dedupe CSV: {id} is preferred in one row and dropped in another").into());
    }
    kept.sort();
    kept.dedup();
    Ok((skip, kept))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "no_stop_place_id,se_stop_place_id,preferred_name_stop_place_id,preferred_name
NSR:StopPlace:127,SSR:StopPlace:1,NSR:StopPlace:127,Ski stasjon
NSR:StopPlace:127,SSR:StopPlace:2,NSR:StopPlace:127,Ski stasjon
NSR:StopPlace:107,SSR:StopPlace:3,SSR:StopPlace:3,\"Grums, Nyängen\"
NSR:StopPlace:374,SSR:StopPlace:4,SSR:StopPlace:4,Göteborg Centralstation
";

    fn ids(v: &[&str]) -> HashSet<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn drops_the_non_preferred_side() {
        let present = ids(&["NSR:StopPlace:127", "SSR:StopPlace:1", "SSR:StopPlace:2", "NSR:StopPlace:107", "SSR:StopPlace:3"]);
        let (skip, kept) = parse(CSV, &present).unwrap();
        assert_eq!(skip, ids(&["SSR:StopPlace:1", "SSR:StopPlace:2", "NSR:StopPlace:107"]));
        assert!(kept.is_empty());
    }

    #[test]
    fn keeps_a_stop_whose_preferred_duplicate_is_missing() {
        let present = ids(&["NSR:StopPlace:374"]);
        let (skip, kept) = parse(CSV, &present).unwrap();
        assert!(skip.is_empty());
        assert_eq!(kept, ["NSR:StopPlace:374"]);
    }

    #[test]
    fn tolerates_crlf_blank_lines_and_quoted_ids() {
        let csv = "h\r\n\r\n\"NSR:StopPlace:1\",\"SSR:StopPlace:2\",\"SSR:StopPlace:2\"\r\n";
        let (skip, _) = parse(csv, &ids(&["NSR:StopPlace:1", "SSR:StopPlace:2"])).unwrap();
        assert_eq!(skip, ids(&["NSR:StopPlace:1"]));
    }

    #[test]
    fn rejects_bad_rows() {
        for csv in [
            "h\nNSR:StopPlace:1,SSR:StopPlace:2,SSR:StopPlace:3\n",
            "h\nNSR:StopPlace:1,NSR:StopPlace:1,NSR:StopPlace:1\n",
            "h\nNSR:StopPlace:1,SSR:StopPlace:2\n",
            "h\nA,B,A\nA,B,B\n",
            "h\nA,B,A\nB,C,B\n",
        ] {
            assert!(parse(csv, &HashSet::new()).is_err(), "{csv}");
        }
    }
}
