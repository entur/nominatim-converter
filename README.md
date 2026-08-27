# nominatim-converter

Turns national geodata - stop places, address registries, place names, points of interest
and OpenStreetMap - into a single [Nominatim](https://nominatim.org/)-compatible NDJSON dump
that [Photon](https://github.com/komoot/photon) can index. It is the data half of Entur's
geocoder: [entur/geocoder](https://github.com/entur/geocoder) runs the search API and the
nightly import pipeline, and this tool produces what that pipeline imports. One Rust binary,
no database, no Nominatim install.

```mermaid
flowchart LR
    A["NeTEx, CSV/GML, PBF, GeoPackage"] --> B[nominatim-converter]
    B --> C[nominatim.ndjson]
    C --> D[Photon index]
    D --> E[Geocoder API]
```

## Data sources

| Source | Input | Description |
|--------|-------|-------------|
| **stopplace** | NeTEx XML | NSR/SAM stop places and groups of stop places, with fare zones |
| **matrikkel** | CSV + GML | Kartverket address registry (vegadresser), with street aggregation |
| **stedsnavn** | GML | Kartverket place names (SSR) |
| **poi** | NeTEx XML | Points of interest |
| **osm** | PBF | OpenStreetMap nodes, ways and relations |
| **belagenhet** | GeoPackage | Lantmäteriet belägenhetsadresser (Swedish addresses) |

## Build

Requires Rust 2024 edition (1.85+); PROJ is statically linked via `bundled_proj`.

```bash
cargo build --release      # -> target/release/nominatim-converter
```

## Usage

Everything is driven by a `converter.json` - which sources to import, where their data comes
from, and how they are ranked. See [`converter.example.json`](converter.example.json) for the
full schema.

```bash
nominatim-converter build -c converter.json -o output.ndjson -f
```

`build` converts every source that has a section in the config and appends them into one
NDJSON, in a fixed order (matrikkel, stedsnavn, poi, stopplace, osm, belagenhet). Each
section's `input` says where the data comes from:

```json
"matrikkel":  { "input": { "region": "all" }, ... },                  // Geonorge download
"stopPlace":  { "input": { "url": "https://.../Current_latest.zip" }, ... },
"osm":        { "input": { "file": "norway-latest.osm.pbf" }, ... },  // local file
"belagenhet": { "input": { "municipality": "all" }, ... }             // Lantmäteriet download
```

To skip a source, omit its section entirely; a section without an `input` is a hard error.
`region` and `municipality` trigger downloads (`regions` and `municipalities` list the valid
codes); `municipality` needs Lantmäteriet Geotorget credentials in `LANTMATERIET_USER` /
`LANTMATERIET_PASS`, from the environment or a `.env` file.

### Single-source subcommands

For debugging and ad-hoc runs, one **local** file at a time. These never download.

```bash
nominatim-converter stopplace  -i stop_places.xml -o out.ndjson -c converter.json --fare-zones fare-zones.xml
nominatim-converter matrikkel  -i adresse.csv -o out.ndjson -c converter.json -g stedsnavn.gml
nominatim-converter stedsnavn  -i stedsnavn.gml -o out.ndjson -c converter.json
nominatim-converter poi        -i poi.xml -o out.ndjson -c converter.json
nominatim-converter osm        -i planet.osm.pbf -o out.ndjson -c converter.json
nominatim-converter belagenhet -i belagenhetsadresser_kn0180.gpkg -o out.ndjson -c converter.json
```

### Flags

| Flag | Description |
|------|-------------|
| `-i` | Input file (single-source subcommands; required) |
| `-o` | Output file (required) |
| `-c` | Config file (defaults to `converter.json` in CWD; required for `build`) |
| `-f` / `-a` | Force overwrite / append to existing output |
| `-d` | Cache directory for downloads; also via `NOMINATIM_CACHE_DIR` |
| `--refresh-cache` | Ignore cache hits and re-download |
| `-u` | Local `id;name;usage` CSV that boosts popular entities |
| `--min-lines <N>` | Abort if fewer than N entries are written |
| `--warn-if-stale[=HOURS]` | Warn when a resolved source is older than HOURS (bare = 24) |

## Features

### Fare zones

Stop place fare zones come from their own NeTEx export rather than from the stop place file:

```json
"stopPlace": {
  "input":     { "url": "https://.../Current_latest.zip" },
  "fareZones": { "input": { "url": "https://api.entur.io/distance/netex/fare-zones" } }
}
```

`fareZones` is optional. Left out, the converter falls back to the `:FareZone:` refs NSR mirrors
into each stop and the authorities in the same input's `<FareFrame>`, which on a full run
reproduces the export exactly. That mirror is due to disappear, so set `fareZones` for anything
long-lived. The two never mix.

### Usage boost

A semicolon-separated `id;name;usage` CSV nudges popular entities up the importance ranking.
`build` resolves it from the `usage` section's `input`; the subcommands take a local path via `-u`.

```
id;name;usage
NSR:StopPlace:59872;Oslo S;139608
```

IDs use each source's native format (`NSR:StopPlace:N`, `KVE:PostalAddress:N`, ...). Missing IDs,
and IDs at or below `usageFloor`, get factor 1.0 - no penalty. The boost is
`1 + alpha * log10(usage / usageFloor)`, applied to raw popularity before the log10 importance
normalization, and tuned in the config (`"usage": { "alpha": 0.5, "usageFloor": 100 }`). With the
defaults, a stop with ~1M boardings gets a ~3x nudge - meaningful but bounded, so airports keep
dominating the top by structural ranking.

### Entrance enrichment (OSM)

Large area features (military areas, parks, quarries, campuses) are emitted at their polygon
centroid, which can be a poor routing destination deep inside an inaccessible area. Set
`useEntrance` on a POI filter to emit an entrance or gate node instead:

```json
{ "key": "landuse", "value": "military", "priority": 1, "useEntrance": true }
```

Candidates are `entrance=*` (except `entrance=no`), `routing:entrance=*`, or a passable
`barrier=*`. When there are several, one is picked by priority: an explicit `*=main` marker >
a pedestrian entrance > a gate > a routable gate > the gate on the most major road, ties broken
by node id. Only features at least 150 m across their longer bounding-box side are considered;
selection is per-feature, so every co-named parcel containing the gate is enriched.

### Safety nets

**`minLines`** guards against silently broken imports - an empty download, a changed upstream
format. Set it per source in the config and `build` fails instead of shipping a degraded index;
the subcommands can override it for one run with `--min-lines <N>` (`0` disables). It counts the
entries *this run* emits, so it is correct in append mode too.

**`--warn-if-stale`** catches frozen upstreams: a rolling URL that stopped updating, a local file
nobody refreshed. Ages come from the server's `Last-Modified` (stamped onto the cached file, so a
warm-cache run still reports the true upstream date) or the file's mtime. Purely advisory - it
never changes the exit code.

**`-d <DIR>`** caches downloaded files - including entries extracted from ZIPs - and reuses them
on later runs; with a warm cache a `municipality` build needs no credentials. Rolling URLs like
`Current_latest.zip` will happily serve stale data from the cache, so pass `--refresh-cache` when
that matters.

## Output

NDJSON. The first line is a header, the rest are place entries. All floats are serialized with
exactly six decimals.

```json
{"type":"NominatimDumpFile","content":{"version":"0.1.0","generator":"geocoder",...}}
{"type":"Place","content":[{"place_id":"KVE-PostalAddress-225678815","object_type":"N","categories":[...],...}]}
```

## Architecture

```
src/
├── main.rs        # CLI entry point (clap)
├── config.rs      # converter.json schema
├── common/        # coordinates and projection (UTM33/SWEREF99 TM -> WGS84), country
│                  # detection, importance scoring, translations, text helpers
├── source/        # one module per source: stopplace, matrikkel, stedsnavn, poi,
│                  # belagenhet, and osm (a 4-pass PBF reader)
└── target/        # NDJSON writer, place_id sanitization, Nominatim schema (serde)
```

Country boundaries ship inside the binary as `data/boundaries60x30.ser`, derived from
[JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm)
via the [countryboundaries](https://github.com/westnordost/countryboundaries) generator.

## License

[EUPL-1.2](LICENSE.md)
