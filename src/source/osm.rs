//! OSM PBF to Nominatim NDJSON converter.
//!
//! Converts OSM PBF files in 4 passes:
//! 1. Relations pass: Collect admin boundaries and POI relation member way IDs
//! 2. Ways pass: Collect all needed node IDs (streets, admin ways, POI ways, relation member ways)
//! 3. Nodes pass: Fetch node coordinates for all needed nodes
//! 4. Ways+Relations pass: Build indexes, calculate centroids, and convert POIs

mod address_index;
mod admin;
mod coordinate;
mod entity;
mod entrance;
mod geometry;
mod grid;
mod indexing;
mod pass4;
mod passes;
mod popularity;
mod street;

use std::path::Path;

use crate::config::Config;

use passes::OsmConverter;

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
    usage: &crate::common::usage::UsageBoost,
) -> Result<usize, Box<dyn std::error::Error>> {
    config.osm.as_ref().ok_or("config is missing the required `osm` section")?;
    let converter = OsmConverter::new(config.clone());
    converter.convert(input, output, is_appending, usage)
}

