use crate::common::util::round6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

impl Coordinate {
    pub const ZERO: Coordinate = Coordinate { lat: 0.0, lon: 0.0 };

    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    pub fn centroid(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat)]
    }

    pub fn bbox(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat), round6(self.lon), round6(self.lat)]
    }
}
