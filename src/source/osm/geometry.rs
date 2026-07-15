use super::coordinate::Coordinate;

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl BoundingBox {
    pub fn contains(&self, coord: &Coordinate) -> bool {
        coord.lat >= self.min_lat
            && coord.lat <= self.max_lat
            && coord.lon >= self.min_lon
            && coord.lon <= self.max_lon
    }

    pub fn area(&self) -> f64 {
        (self.max_lat - self.min_lat) * (self.max_lon - self.min_lon)
    }

    pub fn from_coordinates(coords: &[Coordinate]) -> Option<Self> {
        if coords.is_empty() {
            return None;
        }
        let mut min_lat = f64::MAX;
        let mut max_lat = f64::MIN;
        let mut min_lon = f64::MAX;
        let mut max_lon = f64::MIN;
        for c in coords {
            if c.lat < min_lat {
                min_lat = c.lat;
            }
            if c.lat > max_lat {
                max_lat = c.lat;
            }
            if c.lon < min_lon {
                min_lon = c.lon;
            }
            if c.lon > max_lon {
                max_lon = c.lon;
            }
        }
        Some(Self {
            min_lat,
            max_lat,
            min_lon,
            max_lon,
        })
    }
}

pub(crate) fn calculate_centroid(coords: &[Coordinate]) -> Option<Coordinate> {
    if coords.is_empty() {
        return None;
    }
    let n = coords.len() as f64;
    let lat = coords.iter().map(|c| c.lat).sum::<f64>() / n;
    let lon = coords.iter().map(|c| c.lon).sum::<f64>() / n;
    Some(Coordinate { lat, lon })
}

/// Even-odd ray casting: is `coord` inside the closed ring? Rings with fewer than 3 points
/// are never inside. Operates on raw lat/lon; exact for containment (only the crossing count
/// matters, not distances).
pub(crate) fn ray_cast_contains(ring: &[Coordinate], coord: &Coordinate) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let n = ring.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let ci = &ring[i];
        let cj = &ring[j];
        if (ci.lon > coord.lon) != (cj.lon > coord.lon)
            && coord.lat < (cj.lat - ci.lat) * (coord.lon - ci.lon) / (cj.lon - ci.lon) + ci.lat
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box() {
        let bbox = BoundingBox {
            min_lat: 59.0,
            max_lat: 61.0,
            min_lon: 10.0,
            max_lon: 12.0,
        };
        assert!(bbox.contains(&Coordinate { lat: 60.0, lon: 11.0 }));
        assert!(!bbox.contains(&Coordinate { lat: 62.0, lon: 11.0 }));
        assert!((bbox.area() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn test_bounding_box_from_coordinates() {
        let coords = vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 61.0, lon: 12.0 },
            Coordinate { lat: 60.0, lon: 11.0 },
        ];
        let bbox = BoundingBox::from_coordinates(&coords).unwrap();
        assert!((bbox.min_lat - 59.0).abs() < 1e-9);
        assert!((bbox.max_lat - 61.0).abs() < 1e-9);
        assert!((bbox.min_lon - 10.0).abs() < 1e-9);
        assert!((bbox.max_lon - 12.0).abs() < 1e-9);
    }

    #[test]
    fn test_calculate_centroid() {
        let coords = vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 61.0, lon: 12.0 },
        ];
        let c = calculate_centroid(&coords).unwrap();
        assert!((c.lat - 60.0).abs() < 1e-9);
        assert!((c.lon - 11.0).abs() < 1e-9);
    }

    #[test]
    fn test_calculate_centroid_empty() {
        assert!(calculate_centroid(&[]).is_none());
    }
}
