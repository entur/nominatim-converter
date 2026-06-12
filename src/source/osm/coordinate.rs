/// Geographic coordinate (latitude, longitude).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

impl Coordinate {
    pub fn centroid(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat)]
    }

    pub fn bbox(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat), round6(self.lon), round6(self.lat)]
    }
}

pub(crate) fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

// ---------------------------------------------------------------------------
// CoordinateStore -- open-addressing hash map storing coords as delta-encoded ints
// ---------------------------------------------------------------------------

const BASE_LAT: f64 = -90.0;
const BASE_LON: f64 = -180.0;
/// Scaling factor for coordinate storage. At 1e5, one integer unit ≈ 1.1 meters,
/// which is sufficient precision for centroid calculations while halving memory
/// usage compared to storing raw f64 pairs.
const COORD_SCALE: f64 = 1e5;
const LOAD_FACTOR: f64 = 0.7;

/// A memory-efficient coordinate lookup table mapping OSM node IDs to lat/lon.
///
/// Uses an open-addressing hash map with linear probing instead of `HashMap` to
/// reduce memory overhead -- OSM files can contain millions of nodes. Coordinates
/// are stored as delta-encoded integers (offset from -90/-180) at 1e5 scale
/// (~1.1 m precision) to use i32 instead of f64, further reducing memory.
pub struct CoordinateStore {
    ids: Vec<i64>,
    delta_lats: Vec<i32>,
    delta_lons: Vec<i32>,
    size: usize,
}

impl CoordinateStore {
    pub fn new(initial_capacity: usize) -> Self {
        // hash() takes the id modulo the capacity, so a zero capacity would divide by zero on
        // the first put/get.
        assert!(initial_capacity > 0, "CoordinateStore capacity must be positive");
        Self {
            ids: vec![0; initial_capacity],
            delta_lats: vec![0; initial_capacity],
            delta_lons: vec![0; initial_capacity],
            size: 0,
        }
    }

    pub fn put(&mut self, id: i64, coord: Coordinate) {
        if self.size as f64 >= self.ids.len() as f64 * LOAD_FACTOR {
            self.resize();
        }
        let capacity = self.ids.len();
        let mut index = Self::hash(id, capacity);
        // id 0 marks an empty slot; OSM ids are always positive.
        while self.ids[index] != 0 && self.ids[index] != id {
            index = (index + 1) % capacity;
        }
        if self.ids[index] == 0 {
            self.size += 1;
        }
        self.ids[index] = id;
        self.delta_lats[index] = ((coord.lat - BASE_LAT) * COORD_SCALE) as i32;
        self.delta_lons[index] = ((coord.lon - BASE_LON) * COORD_SCALE) as i32;
    }

    pub fn get(&self, id: i64) -> Option<Coordinate> {
        let capacity = self.ids.len();
        let mut index = Self::hash(id, capacity);
        // id 0 marks an empty slot; OSM ids are always positive.
        while self.ids[index] != 0 {
            if self.ids[index] == id {
                let lat = BASE_LAT + self.delta_lats[index] as f64 / COORD_SCALE;
                let lon = BASE_LON + self.delta_lons[index] as f64 / COORD_SCALE;
                return Some(Coordinate { lat, lon });
            }
            index = (index + 1) % capacity;
        }
        None
    }

    fn hash(id: i64, capacity: usize) -> usize {
        (id.wrapping_mul(2_654_435_761).rem_euclid(capacity as i64)) as usize
    }

    fn resize(&mut self) {
        let old_ids = std::mem::take(&mut self.ids);
        let old_lats = std::mem::take(&mut self.delta_lats);
        let old_lons = std::mem::take(&mut self.delta_lons);
        let new_cap = old_ids.len() * 2;

        self.ids = vec![0; new_cap];
        self.delta_lats = vec![0; new_cap];
        self.delta_lons = vec![0; new_cap];
        self.size = 0;

        for i in 0..old_ids.len() {
            if old_ids[i] != 0 {
                let lat = BASE_LAT + old_lats[i] as f64 / COORD_SCALE;
                let lon = BASE_LON + old_lons[i] as f64 / COORD_SCALE;
                self.put(old_ids[i], Coordinate { lat, lon });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinate_store() {
        let mut store = CoordinateStore::new(16);
        store.put(1, Coordinate { lat: 59.9, lon: 10.7 });
        store.put(2, Coordinate { lat: 60.0, lon: 11.0 });

        let c1 = store.get(1).unwrap();
        assert!((c1.lat - 59.9).abs() < 0.001);
        assert!((c1.lon - 10.7).abs() < 0.001);

        let c2 = store.get(2).unwrap();
        assert!((c2.lat - 60.0).abs() < 0.001);
        assert!((c2.lon - 11.0).abs() < 0.001);

        assert!(store.get(999).is_none());
    }
}
