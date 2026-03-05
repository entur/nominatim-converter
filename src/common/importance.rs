use crate::common::util::round6;
use crate::config::ImportanceConfig;

pub struct ImportanceCalculator {
    config: ImportanceConfig,
}

impl ImportanceCalculator {
    pub fn new(config: &ImportanceConfig) -> Self {
        Self { config: config.clone() }
    }

    /// Normalize popularity to Photon importance (0-1 range) using log10 normalization.
    pub fn calculate_importance(&self, popularity: f64) -> f64 {
        let log_pop = popularity.log10();
        let log_min = self.config.min_popularity.log10();
        let log_max = self.config.max_popularity.log10();

        let normalized = (log_pop - log_min) / (log_max - log_min);
        let scaled = self.config.floor + (normalized * (1.0 - self.config.floor));

        round6(scaled.clamp(self.config.floor, 1.0))
    }
}
