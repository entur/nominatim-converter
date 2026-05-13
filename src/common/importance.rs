use crate::common::usage::UsageBoost;
use crate::common::util::round6;
use crate::config::ImportanceConfig;

/// Normalizes raw popularity scores to Photon importance values in the 0-1 range.
///
/// Uses log10 normalization because popularity values span many orders of magnitude
/// (e.g. 1 for a small address to 1 billion for a major city). Log scaling compresses
/// this range so that differences between low-popularity items are still visible.
/// The `floor` config value sets the minimum importance (typically 0.1).
///
/// Holds a reference to a [`UsageBoost`] so callers can apply the popularity-by-id
/// boost in a single call via [`Self::calculate_importance_for`]. When no `--usage`
/// CSV is configured the boost factor is 1.0 for every id, leaving output unchanged.
pub struct ImportanceCalculator<'a> {
    config: ImportanceConfig,
    usage: &'a UsageBoost,
}

impl<'a> ImportanceCalculator<'a> {
    pub fn new(config: &ImportanceConfig, usage: &'a UsageBoost) -> Self {
        Self { config: *config, usage }
    }

    /// Normalize popularity to Photon importance (0-1 range) using log10 normalization.
    pub fn calculate_importance(&self, popularity: f64) -> f64 {
        round6(self.scaled_importance(popularity).clamp(self.config.floor, 1.0))
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
        let log_min = self.config.min_popularity.log10();
        let log_max = self.config.max_popularity.log10();
        let normalized = (log_pop - log_min) / (log_max - log_min);
        self.config.floor + (normalized * (1.0 - self.config.floor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static EMPTY: std::sync::LazyLock<UsageBoost> =
        std::sync::LazyLock::new(UsageBoost::empty);

    fn prod_config() -> ImportanceConfig {
        ImportanceConfig {
            min_popularity: 1.0,
            max_popularity: 1_000_000_000.0,
            floor: 0.1,
        }
    }

    #[test]
    fn test_min_popularity_returns_floor() {
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        assert_eq!(calc.calculate_importance(1.0), 0.1);
    }

    #[test]
    fn test_max_popularity_returns_one() {
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        assert_eq!(calc.calculate_importance(1_000_000_000.0), 1.0);
    }

    #[test]
    fn test_mid_popularity() {
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        let imp = calc.calculate_importance(1000.0);
        // log10(1000) = 3, log10(1) = 0, log10(1e9) = 9
        // normalized = 3/9 = 0.333...
        // scaled = 0.1 + (0.333... * 0.9) = 0.1 + 0.3 = 0.4
        assert_eq!(imp, 0.4);
    }

    #[test]
    fn test_below_min_clamps_to_floor() {
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        // popularity of 0.1 would give negative normalized => clamp to floor
        let imp = calc.calculate_importance(0.1);
        assert_eq!(imp, 0.1);
    }

    #[test]
    fn test_above_max_clamps_to_one() {
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        let imp = calc.calculate_importance(10_000_000_000.0);
        assert_eq!(imp, 1.0);
    }

    #[test]
    fn test_known_stop_place_importance() {
        // StopPlace default = 50
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        let imp = calc.calculate_importance(50.0);
        // log10(50)=1.699, normalized=1.699/9=0.1888, scaled=0.1+0.1888*0.9=0.2699
        assert_eq!(imp, 0.269897);
    }

    #[test]
    fn test_known_matrikkel_importance() {
        // Matrikkel address popularity = 20
        let calc = ImportanceCalculator::new(&prod_config(), &EMPTY);
        let imp = calc.calculate_importance(20.0);
        assert_eq!(imp, 0.230103);
    }

}
