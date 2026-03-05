#[derive(Debug, Clone, Copy)]
pub enum NominatimId {
    Address,
    Street,
    Stedsnavn,
    StopPlace,
    Gosp,
    Osm,
    Poi,
}

impl NominatimId {
    fn prefix(self) -> i64 {
        match self {
            NominatimId::Address => 100,
            NominatimId::Street => 200,
            NominatimId::Stedsnavn => 300,
            NominatimId::StopPlace => 400,
            NominatimId::Gosp => 450,
            NominatimId::Osm => 500,
            NominatimId::Poi => 600,
        }
    }

    pub fn create(self, id: &str) -> i64 {
        let num = match id.parse::<i64>() {
            Ok(n) => n.unsigned_abs() as i64,
            Err(_) => java_string_hashcode(id).unsigned_abs() as i64,
        };
        format!("{}{}", self.prefix(), num).parse::<i64>().unwrap_or(self.prefix())
    }

    pub fn create_from_i64(self, id: i64) -> i64 {
        self.create(&id.to_string())
    }
}

/// Matches Java/Kotlin `String.hashCode()`: s[0]*31^(n-1) + s[1]*31^(n-2) + ... + s[n-1]
fn java_string_hashcode(s: &str) -> i64 {
    let mut h: i32 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as i32);
    }
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_java_hashcode() {
        // Known Java hashCode values
        assert_eq!(java_string_hashcode(""), 0);
        assert_eq!(java_string_hashcode("a"), 97);
        assert_eq!(java_string_hashcode("abc"), 96354);
    }

    #[test]
    fn test_create_numeric() {
        assert_eq!(NominatimId::StopPlace.create("123"), 400123);
    }

    #[test]
    fn test_gosp_known_id() {
        let id = NominatimId::Gosp.create("NSR:GroupOfStopPlaces:1");
        assert_eq!(id, 4501627834738);
    }
}
