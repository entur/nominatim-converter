pub(crate) use crate::common::geo::distance_meters;

/// Grid cell size in degrees (~550 m at 60°N latitude). Street segments are bucketed
/// into cells of this size for fast spatial lookup.
pub(crate) const GRID_SIZE: f64 = 0.005;
