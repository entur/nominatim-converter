/// Round to 6 decimal places.
pub fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round6() {
        assert_eq!(round6(59.912345678), 59.912346);
        assert_eq!(round6(10.0), 10.0);
        assert_eq!(round6(0.1234565), 0.123457); // rounds up
        assert_eq!(round6(0.1234564), 0.123456); // rounds down
    }

    #[test]
    fn test_round6_negative() {
        assert_eq!(round6(-10.123456789), -10.123457);
    }
}
