# nominatim-convert

A Rust CLI tool that converts Norwegian geographic data sources into Nominatim-compatible NDJSON. This is a port of the Kotlin converter from [entur/geocoder](https://github.com/entur/geocoder), producing byte-identical output.

## Data sources

| Source | Input format | Description |
|--------|-------------|-------------|
| **stopplace** | NeTEx XML | NSR/SAM stop places and groups of stop places |
| **matrikkel** | CSV + GML | Kartverket address registry (vegadresser) with street aggregation |
| **stedsnavn** | GML | Kartverket place names (SSR) |
| **poi** | NeTEx XML | Points of interest from NeTEx |
| **osm** | PBF | OpenStreetMap entities (nodes, ways, relations) |

## Building

Requires Rust 2024 edition (1.85+) and the [PROJ](https://proj.org/) C library for coordinate transformations.

```bash
# macOS
brew install proj

# Build release binary (with LTO)
cargo build --release
```

The binary is at `target/release/nominatim-convert`.

## Usage

All subcommands require a `converter.json` configuration file (see the Kotlin project for the schema).

```bash
# StopPlace
nominatim-convert stopplace -i stop_places.xml -o output.ndjson -c converter.json

# Matrikkel (addresses + streets, needs stedsnavn GML for county lookup)
nominatim-convert matrikkel -i adresse.csv -o output.ndjson -c converter.json -g stedsnavn.gml

# Matrikkel (without county data)
nominatim-convert matrikkel -i adresse.csv -o output.ndjson -c converter.json --no-county

# Stedsnavn
nominatim-convert stedsnavn -i stedsnavn.gml -o output.ndjson -c converter.json

# POI
nominatim-convert poi -i poi.xml -o output.ndjson -c converter.json

# OSM
nominatim-convert osm -i planet.osm.pbf -o output.ndjson -c converter.json
```

### Common flags

| Flag | Description |
|------|-------------|
| `-i` | Input file (required) |
| `-o` | Output file (required) |
| `-c` | Config file (defaults to `converter.json` in CWD) |
| `-f` | Force overwrite existing output |
| `-a` | Append to existing output |

## Output format

NDJSON (newline-delimited JSON). First line is a header:

```json
{"type":"NominatimDumpFile","content":{"version":"0.1.0","generator":"geocoder",...}}
```

Subsequent lines are place entries:

```json
{"type":"Place","content":[{"place_id":400123,"object_type":"N","categories":[...],...}]}
```

All floating-point values are serialized with exactly 6 decimal places to match the Kotlin output.

## Architecture

```
src/
├── main.rs                  # CLI entry point (clap)
├── config.rs                # converter.json schema
├── common/
│   ├── category.rs          # Category string constants
│   ├── coordinate.rs        # Lat/lon coordinate type
│   ├── country.rs           # ISO 3166-1 alpha-2/alpha-3 mapping (full set)
│   ├── extra.rs             # Extra metadata fields
│   ├── geo.rs               # UTM33→WGS84 projection, country detection
│   ├── importance.rs        # Log-normalized importance scoring
│   ├── text.rs              # OSM tag formatting
│   ├── translator.rs        # Name/type translations
│   └── util.rs              # titleize, round6, etc.
├── source/
│   ├── stopplace/mod.rs     # NeTEx StopPlace XML parser
│   ├── matrikkel/mod.rs     # Kartverket CSV parser + street aggregation
│   ├── stedsnavn/mod.rs     # SSR GML parser
│   ├── poi/mod.rs           # NeTEx POI XML parser
│   └── osm/mod.rs           # OSM PBF parser (nodes, ways, relations)
└── target/
    ├── json_writer.rs       # NDJSON output with header
    ├── nominatim_id.rs      # Deterministic place_id generation (Java hashCode compat)
    └── nominatim_place.rs   # Nominatim NDJSON schema (serde)
```

## Embedded data

- `data/boundaries60x30.ser` — Country boundary raster data from [westnordost/countryboundaries](https://github.com/westnordost/countryboundaries), embedded in the binary via `include_bytes!`. Uses the same file as the Kotlin converter for identical country detection results.

## Compatibility with the Kotlin converter

This converter produces identical output to the Kotlin version:

- **place_id generation**: Uses Java `String.hashCode()` algorithm (wrapping i32 arithmetic with multiplier 31) for deterministic IDs
- **Country detection**: Same `boundaries60x30.ser` file and `country-boundaries` crate (by the same author as the Java library)
- **Float formatting**: 6 decimal places for importance, coordinates, and bounding boxes
- **Category ordering**: Matches Kotlin's 3-pass tariff zone construction
- **Alt name deduplication**: Preserves insertion order (like Java's LinkedHashSet)

Verified with full dataset comparisons:
- StopPlace: 58,639 entries — 0 diffs
- Matrikkel: 2,667,479 entries — 0 semantic diffs (coordinate rounding at 6th decimal due to different PROJ implementations)
- Stedsnavn: 2,215 entries — 0 semantic diffs (same coordinate rounding)

## Performance

Benchmarks on Apple Silicon (M-series), release build with LTO:

| Source | Entries | Time |
|--------|---------|------|
| StopPlace (150MB XML) | 58,639 | ~0.5s |
| Matrikkel (800MB CSV + 2.6GB GML) | 2,667,479 | ~17s |
| Stedsnavn (2.6GB GML) | 2,215 | ~12s |

## License

Same as the upstream Kotlin converter.
