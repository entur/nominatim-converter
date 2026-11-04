use crate::common::country::Country;
use crate::common::usage::UsageBoost;
use crate::common::util::round6;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Popularity-normalization constants for the Photon importance (0-1) scale. Every region's
/// import has always used these exact values, so they are fixed constants rather than per-config
/// knobs. `MIN`/`MAX` bound the log10 popularity range; `FLOOR` is the minimum emitted importance
/// (Photon's `function_score` drops docs whose importance collapses to zero).
const MIN_POPULARITY: f64 = 1.0;
const MAX_POPULARITY: f64 = 1_000_000_000.0;
/// Minimum emitted importance. Also the floor other sources clamp their own values to (poi)
/// and the lower bound for demoted GoSPs (stopplace), so it is public.
pub const IMPORTANCE_FLOOR: f64 = 0.001;

/// Importance multiplier per country (alpha-2 key) with an optional `*` for every other known
/// country, e.g. `{"*": 0.5, "no": 1.0, "se": 0.6}`. Factors must be in (0, 1]. Entities with no
/// country are unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CountryFactors(BTreeMap<String, f64>);

const ANY_COUNTRY: &str = "*";

impl CountryFactors {
    pub const fn empty() -> Self {
        Self(BTreeMap::new())
    }

    fn get(&self, country: Option<&Country>) -> f64 {
        let Some(c) = country else { return 1.0 };
        self.0.get(&c.alpha2).or_else(|| self.0.get(ANY_COUNTRY)).copied().unwrap_or(1.0)
    }
}

impl<'de> Deserialize<'de> for CountryFactors {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = BTreeMap::<String, f64>::deserialize(deserializer)?;
        let mut out = BTreeMap::new();
        for (key, factor) in raw {
            let key = if key == ANY_COUNTRY {
                key
            } else {
                Country::parse(&key).ok_or_else(|| D::Error::custom(format!("unknown country code `{key}`")))?.alpha2
            };
            if !(factor > 0.0 && factor <= 1.0) {
                return Err(D::Error::custom(format!("factor for `{key}` must be in (0, 1], got {factor}")));
            }
            out.insert(key, factor);
        }
        Ok(Self(out))
    }
}

static NO_FACTORS: CountryFactors = CountryFactors::empty();

/// Normalizes raw popularity scores to Photon importance values in the 0-1 range.
///
/// Uses log10 normalization because popularity values span many orders of magnitude
/// (e.g. 1 for a small address to 1 billion for a major city). Log scaling compresses
/// this range so that differences between low-popularity items are still visible.
///
/// Holds a reference to a [`UsageBoost`] so callers can apply the popularity-by-id
/// boost in a single call via [`Self::calculate_importance_for`]. When no `--usage`
/// CSV is configured the boost factor is 1.0 for every id, leaving output unchanged.
pub struct ImportanceCalculator<'a> {
    usage: &'a UsageBoost,
    country_factors: &'a CountryFactors,
}

impl<'a> ImportanceCalculator<'a> {
    pub fn new(usage: &'a UsageBoost) -> Self {
        Self { usage, country_factors: &NO_FACTORS }
    }

    pub fn with_country_factors(mut self, factors: &'a CountryFactors) -> Self {
        self.country_factors = factors;
        self
    }

    /// Scale an already normalized importance by its country's factor, floored at `IMPORTANCE_FLOOR`.
    pub fn apply_country_factor(&self, importance: f64, country: Option<&Country>) -> f64 {
        let f = self.country_factors.get(country);
        if f == 1.0 { importance } else { (importance * f).max(IMPORTANCE_FLOOR) }
    }

    /// [`Self::calculate_importance_for`], then [`Self::apply_country_factor`].
    pub fn calculate_importance_in(&self, id: &str, popularity: f64, country: Option<&Country>) -> f64 {
        self.apply_country_factor(self.calculate_importance_for(id, popularity), country)
    }

    /// Normalize popularity to Photon importance (0-1 range) using log10 normalization.
    pub fn calculate_importance(&self, popularity: f64) -> f64 {
        round6(self.scaled_importance(popularity).clamp(IMPORTANCE_FLOOR, 1.0))
    }

    /// Apply the usage boost for `id` to `popularity`, then normalize. Equivalent to
    /// `calculate_importance(popularity * usage.factor(id))`. Use this for sources
    /// where each entity has a single popularity value plus an id; sources whose
    /// popularity is multi-factor (e.g. stopplace) apply the boost themselves and
    /// call [`Self::calculate_importance`] directly.
    pub fn calculate_importance_for(&self, id: &str, popularity: f64) -> f64 {
        self.calculate_importance(popularity * self.usage.factor(id))
    }

    fn scaled_importance(&self, popularity: f64) -> f64 {
        let log_pop = popularity.log10();
        let normalized = (log_pop - MIN_POPULARITY.log10()) / (MAX_POPULARITY.log10() - MIN_POPULARITY.log10());
        IMPORTANCE_FLOOR + (normalized * (1.0 - IMPORTANCE_FLOOR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static EMPTY: std::sync::LazyLock<UsageBoost> =
        std::sync::LazyLock::new(UsageBoost::empty);

    // Expected values use the production constants (floor 0.001, min 1, max 1e9).

    #[test]
    fn test_min_popularity_returns_floor() {
        let calc = ImportanceCalculator::new(&EMPTY);
        assert_eq!(calc.calculate_importance(1.0), 0.001);
    }

    #[test]
    fn test_max_popularity_returns_one() {
        let calc = ImportanceCalculator::new(&EMPTY);
        assert_eq!(calc.calculate_importance(1_000_000_000.0), 1.0);
    }

    #[test]
    fn test_mid_popularity() {
        let calc = ImportanceCalculator::new(&EMPTY);
        let imp = calc.calculate_importance(1000.0);
        // log10(1000)=3, normalized=3/9=0.333..., scaled=0.001+(0.333...*0.999)=0.334
        assert_eq!(imp, 0.334);
    }

    #[test]
    fn test_below_min_clamps_to_floor() {
        let calc = ImportanceCalculator::new(&EMPTY);
        // popularity of 0.1 would give negative normalized => clamp to floor
        let imp = calc.calculate_importance(0.1);
        assert_eq!(imp, 0.001);
    }

    #[test]
    fn test_above_max_clamps_to_one() {
        let calc = ImportanceCalculator::new(&EMPTY);
        let imp = calc.calculate_importance(10_000_000_000.0);
        assert_eq!(imp, 1.0);
    }

    #[test]
    fn test_known_stop_place_importance() {
        // StopPlace default = 50
        let calc = ImportanceCalculator::new(&EMPTY);
        assert_eq!(calc.calculate_importance(50.0), 0.189586);
    }

    fn factors(json: &str) -> CountryFactors {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn country_factor_prefers_exact_key_over_wildcard() {
        let f = factors(r#"{"*": 0.5, "NO": 1.0, "se": 0.6}"#);
        let calc = ImportanceCalculator::new(&EMPTY).with_country_factors(&f);
        assert_eq!(calc.apply_country_factor(0.5, Some(&Country::no())), 0.5);
        assert_eq!(calc.apply_country_factor(0.5, Some(&Country::se())), 0.3);
        assert_eq!(calc.apply_country_factor(0.5, Country::parse("de").as_ref()), 0.25);
        assert_eq!(calc.apply_country_factor(0.5, None), 0.5);
    }

    #[test]
    fn country_factor_floors_result() {
        let f = factors(r#"{"se": 0.1}"#);
        let calc = ImportanceCalculator::new(&EMPTY).with_country_factors(&f);
        assert_eq!(calc.apply_country_factor(IMPORTANCE_FLOOR, Some(&Country::se())), IMPORTANCE_FLOOR);
    }

    #[test]
    fn country_factors_reject_bad_keys_and_factors() {
        for bad in [r#"{"swe": 0.5}"#, r#"{"xx": 0.5}"#, r#"{"se": 0}"#, r#"{"se": 1.5}"#, r#"{"se": -1}"#] {
            assert!(serde_json::from_str::<CountryFactors>(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn test_known_matrikkel_importance() {
        // Matrikkel address popularity = 20
        let calc = ImportanceCalculator::new(&EMPTY);
        assert_eq!(calc.calculate_importance(20.0), 0.145414);
    }
}
