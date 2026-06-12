use crate::common::category::sanitize_with_transliteration;
use crate::common::util::fnv1a_64;

/// Convert a structured ID like `KVE:PostalAddress:123` into a place_id string.
///
/// Photon accepts `[0-9a-zA-Z_-]{1,60}` for place_id, and uses it directly as
/// the OpenSearch document id -- so two entries mapping to the same place_id
/// means one of them is silently dropped at import. Uniqueness matters more
/// than looks here.
///
/// ASCII IDs are sanitized in place: colons become dashes, other invalid
/// characters become underscores. IDs containing non-ASCII (e.g. Norwegian
/// Å, Ø, Æ) are first transliterated with the shared table (Å -> Aa,
/// ø -> oe, ...) so the result stays readable, then suffixed with an FNV-1a
/// hash of the original ID. The hash guards against collisions that
/// transliteration alone would introduce: literal `Aa` spellings are common
/// in Norwegian proper nouns (e.g. a street named after Ivar Aasen), so
/// `Åsenvegen` -> `Aasenvegen` could otherwise collide with a real
/// `Aasenvegen` in the same municipality. FNV-1a is pinned by test vectors
/// (`common::util`), so place_ids are stable across Rust releases.
pub fn as_place_id(id: &str) -> String {
    let sanitized = sanitize_with_transliteration(id, '-');

    if id.is_ascii() {
        // No transliteration possible (the table only maps non-ASCII chars),
        // so this is plain sanitization, as before.
        sanitized.chars().take(60).collect()
    } else {
        // Budget: 43 chars for the transliterated prefix + 1 dash + 16 hex chars = 60 max.
        let prefix: String = sanitized.chars().take(43).collect();
        let hash = fnv1a_64(id.as_bytes());
        format!("{prefix}-{hash:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_id() {
        assert_eq!(as_place_id("NSR:StopPlace:59977"), "NSR-StopPlace-59977");
    }

    #[test]
    fn address_id() {
        assert_eq!(as_place_id("KVE:PostalAddress:225678815"), "KVE-PostalAddress-225678815");
    }

    #[test]
    fn street_id_with_spaces() {
        assert_eq!(
            as_place_id("KVE:TopographicPlace:0301-Karl Johans gate"),
            "KVE-TopographicPlace-0301-Karl_Johans_gate"
        );
    }

    #[test]
    fn norwegian_chars_are_transliterated_with_hash_suffix() {
        let id = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        assert!(id.starts_with("KVE-TopographicPlace-3907-Aarfuglveien-"), "got {id}");
        assert!(id.len() <= 60);
    }

    #[test]
    fn non_ascii_place_ids_are_stable_across_builds() {
        // Pinned end-to-end: transliterated prefix + FNV-1a(original id).
        // If this changes, every non-ASCII place_id in the index changes.
        assert_eq!(
            as_place_id("KVE:TopographicPlace:3907-Årfuglveien"),
            "KVE-TopographicPlace-3907-Aarfuglveien-f3d3129a810b417d"
        );
    }

    #[test]
    fn different_norwegian_chars_produce_different_ids() {
        let a = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        let b = as_place_id("KVE:TopographicPlace:3907-Ørfuglveien");
        assert_ne!(a, b);
    }

    #[test]
    fn transliterated_id_does_not_collide_with_literal_spelling() {
        // Åsenvegen transliterates to Aasenvegen; a street literally named
        // Aasenvegen (after e.g. Ivar Aasen) must keep a distinct place_id.
        let transliterated = as_place_id("KVE:TopographicPlace:1577-Åsenvegen");
        let literal = as_place_id("KVE:TopographicPlace:1577-Aasenvegen");
        assert_ne!(transliterated, literal);
        assert!(transliterated.starts_with("KVE-TopographicPlace-1577-Aasenvegen-"));
        assert_eq!(literal, "KVE-TopographicPlace-1577-Aasenvegen");
    }

    #[test]
    fn truncates_at_60_chars() {
        let long_id = "A".repeat(70);
        assert_eq!(as_place_id(&long_id).len(), 60);
    }

    #[test]
    fn long_norwegian_id_within_60_chars() {
        let id = as_place_id(&format!("KVE:TopographicPlace:3442-{}", "Ø".repeat(50)));
        assert!(id.len() <= 60, "got len {}", id.len());
    }

    #[test]
    fn norwegian_id_within_60_chars() {
        let id = as_place_id("KVE:TopographicPlace:3442-Steinsjøvegen");
        assert!(id.len() <= 60);
    }

    #[test]
    fn plain_numeric_id() {
        assert_eq!(as_place_id("12345"), "12345");
    }

    #[test]
    fn deterministic() {
        let a = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        let b = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        assert_eq!(a, b);
    }
}
